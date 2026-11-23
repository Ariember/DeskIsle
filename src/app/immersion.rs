use super::Bars;
use crate::geometry::Rect;
use crate::platform;
use crate::settings::Key;
use crate::visibility::{DockContext, DockDisplayMode, DockPolicy};
use std::time::{Duration, Instant};
use windows::core::Result;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DockExposure {
    Shown,
    Edge,
    Waiting(Instant),
}

impl DockExposure {
    pub(super) fn collapsed(self) -> bool {
        self != Self::Shown
    }

    fn enter_edge(self, now: Instant, delay: Duration) -> Self {
        match self {
            Self::Edge if delay.is_zero() => Self::Shown,
            Self::Edge => Self::Waiting(now + delay),
            Self::Waiting(deadline) if now >= deadline => Self::Shown,
            _ => self,
        }
    }
}

impl Bars {
    pub(super) fn dock_shown(&self) -> bool {
        self.dock_policy != DockPolicy::Hidden && !self.dock_exposure.collapsed()
    }

    fn dock_window_rect(&self) -> Rect {
        match (self.dock_policy, self.dock_exposure) {
            (DockPolicy::EdgeReveal, DockExposure::Shown) => self.layout.revealed_window(),
            (DockPolicy::EdgeReveal, _) => {
                self.layout.reveal_edge(self.settings.dock_display_mode())
            }
            _ => self.layout.bottom,
        }
    }

    pub(super) fn sync_dock_backdrop(&mut self) {
        let region = self
            .dock_glass_requested
            .filter(|_| self.dock_shown())
            .map(|dark| (dark, self.layout.bottom.height));
        if region != self.dock_glass_region {
            // Keep the transparent reveal strip below the drawn bar free of acrylic.
            let (dark, height) = region.unwrap_or_default();
            platform::set_backdrop_region(self.bottom.0, region.is_some(), dark, height);
            self.dock_glass_region = region;
        }
    }

    pub(super) fn position_dock(&mut self) -> Result<()> {
        self.sync_dock_backdrop();
        self.bottom.position_layer(
            self.dock_window_rect(),
            self.dock_policy != DockPolicy::Hidden,
            self.settings.dock_display_mode().topmost(),
        )
    }

    fn set_dock_exposure(&mut self, next: DockExposure, reposition: bool) -> Result<()> {
        let previous = self.dock_exposure;
        if let DockExposure::Waiting(_) = previous {
            self.bottom.cancel_dock_edge_timer();
        }
        if let DockExposure::Waiting(deadline) = next {
            // A cancelled timer's message can arrive after a new dwell has begun.
            // Rearm it for the remainder of the current dwell.
            let remaining = deadline.saturating_duration_since(Instant::now());
            self.bottom
                .time_dock_edge(remaining.as_millis().max(1) as u32)?;
        }
        self.dock_exposure = next;
        if reposition || previous.collapsed() != next.collapsed() {
            self.position_dock()?;
            if !self.dock_shown() {
                self.cancel_dock_press()?;
                self.set_hover(None)?;
                self.set_dock_file_target(None)?;
            } else if self.dock_policy == DockPolicy::EdgeReveal {
                platform::track_leave(self.bottom.0)?;
            }
        }
        Ok(())
    }

    pub(super) fn sync_dock_visibility(&mut self, reposition: bool) -> Result<()> {
        let mode = self.settings.dock_display_mode();
        let smart = mode == DockDisplayMode::SmartHide;
        let overlaps = self
            .events
            .watch_dock_overlap(smart.then_some(self.layout.bottom));
        let policy = mode.policy(DockContext {
            fullscreen: self.fullscreen,
            desktop: mode == DockDisplayMode::Desktop && platform::foreground_is_desktop(),
            overlaps,
            hotkey_shown: self.dock_hotkey_shown,
        });
        if self.dock_policy == policy {
            if reposition {
                self.position_dock()?;
            }
            return Ok(());
        }
        // Keep an active drag or a pointer already over the Dock or its preview when a
        // smart-hide window moves across it. Leaving them then collapses the Dock as usual.
        let keep_open = policy == DockPolicy::EdgeReveal
            && self.dock_shown()
            && (self.dock_press.is_some()
                || self.preview_pressed()
                || self.dock_file_target.is_some()
                || platform::cursor().is_ok_and(|p| {
                    self.layout.bottom.contains(p.x, p.y) || self.preview_holds_dock(p)
                }));
        self.dock_policy = policy;
        let exposure = if policy == DockPolicy::EdgeReveal && !keep_open {
            DockExposure::Edge
        } else {
            DockExposure::Shown
        };
        self.set_dock_exposure(exposure, true)
    }

    pub(super) fn hover_dock_edge(&mut self, timer: bool) -> Result<()> {
        if self.dock_policy != DockPolicy::EdgeReveal || !self.dock_exposure.collapsed() {
            return Ok(());
        }
        let point = platform::cursor()?;
        if !self.dock_window_rect().contains(point.x, point.y) {
            return self.set_dock_exposure(DockExposure::Edge, false);
        }
        let delay = Duration::from_millis(self.settings.get(Key::DockRevealDelay) as u64);
        let next = self.dock_exposure.enter_edge(Instant::now(), delay);
        if next != self.dock_exposure || timer {
            self.set_dock_exposure(next, false)?;
        }
        Ok(())
    }

    pub(super) fn hide_idle_dock(&mut self) -> Result<()> {
        if self.dock_policy == DockPolicy::EdgeReveal
            && self.dock_press.is_none()
            && !self.preview_pressed()
        {
            self.set_dock_exposure(DockExposure::Edge, false)?;
        }
        Ok(())
    }

    pub(super) fn check_dock_pointer(&mut self, window_leave: bool) -> Result<()> {
        if self.dock_policy != DockPolicy::EdgeReveal
            || self.dock_press.is_some()
            || self.preview_pressed()
        {
            return Ok(());
        }
        if self.dock_exposure.collapsed() && window_leave {
            return self.hide_idle_dock();
        }
        let point = platform::cursor()?;
        if self.preview_holds_dock(point) {
            return Ok(());
        }
        let inside = self.dock_window_rect().contains(point.x, point.y);
        if !inside
            && self
                .layout
                .reveal_edge(self.settings.dock_display_mode())
                .contains(point.x, point.y)
        {
            // After a corner reveal the pointer is outside the resized HWND. Check
            // for departure only while it stays on this uncovered part of the edge.
            self.bottom.time_dock_edge(80)?;
        } else if !inside || window_leave {
            self.hide_idle_dock()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revealing_requires_one_continuous_dwell() {
        let start = Instant::now();
        let delay = Duration::from_millis(300);
        let waiting = DockExposure::Edge.enter_edge(start, delay);
        assert!(waiting.collapsed());
        let middle = start + delay / 2;
        assert!(waiting.enter_edge(middle, delay) == waiting);
        assert!(!waiting.enter_edge(start + delay, delay).collapsed());
        // Leaving resets to Edge. An old timer arriving after re-entry cannot reveal it.
        let reentered = DockExposure::Edge.enter_edge(middle, delay);
        assert!(reentered.enter_edge(start + delay, delay).collapsed());
        assert!(
            !DockExposure::Edge
                .enter_edge(start, Duration::ZERO)
                .collapsed()
        );
    }
}
