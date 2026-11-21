use super::Bars;
use crate::platform;
use crate::settings::Key;
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
    pub(super) fn position_dock(&self) -> Result<()> {
        let rect = if self.settings.enabled(Key::DockImmersive) {
            self.layout.immersive_window(self.dock_exposure.collapsed())
        } else {
            self.layout.bottom
        };
        self.bottom.position(rect, !self.fullscreen)
    }

    fn set_dock_exposure(&mut self, next: DockExposure) -> Result<()> {
        let previous = self.dock_exposure;
        if let DockExposure::Waiting(_) = previous {
            self.bottom.cancel_dock_reveal();
        }
        if let DockExposure::Waiting(deadline) = next {
            // A cancelled timer's message can arrive after a new dwell has begun.
            // Rearm it for the remainder of the current dwell.
            let remaining = deadline.saturating_duration_since(Instant::now());
            self.bottom
                .time_dock_reveal(remaining.as_millis().max(1) as u32)?;
        }
        self.dock_exposure = next;
        if previous.collapsed() != next.collapsed() {
            self.position_dock()?;
            if next.collapsed() {
                self.set_hover(None)?;
                self.set_dock_file_target(None)?;
            } else {
                platform::track_leave(self.bottom.0)?;
            }
        }
        Ok(())
    }

    pub(super) fn sync_dock_immersion(&mut self) -> Result<()> {
        let next = if self.settings.enabled(Key::DockImmersive) {
            DockExposure::Edge
        } else {
            DockExposure::Shown
        };
        // Toggling off also removes the transparent gap under an already revealed Dock.
        if self.dock_exposure == DockExposure::Shown && next == DockExposure::Shown {
            return self.position_dock();
        }
        self.set_dock_exposure(next)
    }

    pub(super) fn hover_dock_edge(&mut self, timer: bool) -> Result<()> {
        if self.fullscreen || !self.dock_exposure.collapsed() {
            return Ok(());
        }
        let point = platform::cursor()?;
        if !self
            .layout
            .immersive_window(true)
            .contains(point.x, point.y)
        {
            return self.set_dock_exposure(DockExposure::Edge);
        }
        let delay = Duration::from_millis(self.settings.get(Key::DockRevealDelay) as u64);
        let next = self.dock_exposure.enter_edge(Instant::now(), delay);
        if next != self.dock_exposure || timer {
            self.set_dock_exposure(next)?;
        }
        Ok(())
    }

    pub(super) fn hide_idle_dock(&mut self) -> Result<()> {
        if self.settings.enabled(Key::DockImmersive) && self.dock_press.is_none() {
            self.set_dock_exposure(DockExposure::Edge)?;
        }
        Ok(())
    }

    pub(super) fn finish_dock_gesture(&mut self) -> Result<()> {
        if self.settings.enabled(Key::DockImmersive) {
            let point = platform::cursor()?;
            if !self
                .layout
                .immersive_window(self.dock_exposure.collapsed())
                .contains(point.x, point.y)
            {
                self.hide_idle_dock()?;
            }
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
