//! Dock hover previews: keeping the preview and auto-hidden Dock open while the pointer
//! crosses into the preview window, activating a window when its card is clicked, and
//! closing a window from the card's top-right close button.

use std::time::{Duration, Instant};

use super::Bars;
use crate::geometry::Rect;
use crate::platform::backdrop::Backdrop;
use crate::platform::preview::{PreviewEvent, PreviewWindow};
use crate::platform::{self, shell};
use crate::render::is_device_lost;
use crate::settings::{AppearanceKey::*, Key::Appearance};
use crate::ui::layout::DockHit;
use crate::ui::preview::PreviewHit;
use windows::Win32::Foundation::POINT;
use windows::core::Result;

/// Poll interval while the pointer crosses between the Dock and its preview.
const BRIDGE_POLL: Duration = Duration::from_millis(60);
/// Maximum dwell inside the gap before the preview closes.
const BRIDGE_GRACE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LeaveGrace {
    /// Area the pointer may briefly occupy while moving to the preview or Dock.
    area: Rect,
    deadline: Instant,
}

impl LeaveGrace {
    /// Next poll interval in milliseconds, or `None` once the pointer left `area` or the
    /// grace period elapsed.
    fn next_wait(self, now: Instant, in_area: bool) -> Option<u32> {
        if !in_area || now >= self.deadline {
            return None;
        }
        let remaining = self
            .deadline
            .saturating_duration_since(now)
            .min(BRIDGE_POLL);
        Some(remaining.as_millis().max(1) as u32)
    }
}

pub(super) struct Preview {
    pub(super) app_id: String,
    source_ids: Vec<isize>,
    window: PreviewWindow,
    backdrop: Option<Backdrop>,
    blur: Option<i32>,
    anchor: (i32, i32),
    work: Rect,
    dpi: u32,
    hover: Option<PreviewHit>,
    /// Hit target and source window ID captured when the left button went down.
    pressed: Option<(PreviewHit, isize)>,
    grace: Option<LeaveGrace>,
}

impl Preview {
    pub(super) fn bounds(&self) -> Rect {
        self.window.bounds()
    }
}

impl Bars {
    pub(super) fn close_preview(&mut self) {
        self.bottom.cancel_preview_leave_timer();
        self.preview = None;
    }

    pub(super) fn preview_pressed(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|preview| preview.pressed.is_some())
    }

    /// Screen rectangle bridging the open preview and its Dock icon slot, covering both
    /// the air gap above the Dock and the Dock's top rim above the slot.
    pub(super) fn preview_bridge(&self) -> Option<Rect> {
        let preview = self.preview.as_ref()?;
        let bottom = self.layout.bottom;
        let slot_screen = self
            .apps
            .iter()
            .position(|app| app.app_id == preview.app_id)
            .and_then(|index| {
                self.layout
                    .slots
                    .iter()
                    .find(|slot| slot.target == DockHit::App(index))
            })
            .map(|slot| Rect {
                x: bottom.x + slot.rect.x,
                y: bottom.y + slot.rect.y,
                ..slot.rect
            });
        Some(match slot_screen {
            Some(slot) => preview.window.layout().bridge(slot),
            None => {
                let bounds = preview.window.bounds();
                Rect {
                    x: bounds.x,
                    y: bounds.y + bounds.height,
                    width: bounds.width,
                    height: (bottom.y - (bounds.y + bounds.height)).max(0),
                }
            }
        })
    }

    /// Whether the pointer is interacting with the preview (or crossing its bridge),
    /// which keeps an auto-hidden or smart-hidden Dock open.
    pub(super) fn preview_holds_dock(&self, point: POINT) -> bool {
        let Some(preview) = &self.preview else {
            return false;
        };
        if preview.pressed.is_some() || preview.window.contains_screen_point(point.x, point.y) {
            return true;
        }
        preview
            .grace
            .is_some_and(|grace| grace.area.contains(point.x, point.y))
    }

    /// The application whose preview currently owns the pointer, so a Dock relayout or
    /// window refresh keeps tracking it instead of clearing hover.
    pub(super) fn preview_pointer_app(&self) -> Option<String> {
        let preview = self.preview.as_ref()?;
        let active = preview.pressed.is_some()
            || platform::cursor().is_ok_and(|point| self.preview_holds_dock(point));
        active.then(|| preview.app_id.clone())
    }

    fn start_preview_grace(&mut self, area: Rect) -> Result<()> {
        let Some(preview) = self.preview.as_mut() else {
            return Ok(());
        };
        let now = Instant::now();
        // Keep the original deadline while the pointer stays inside the same grace area,
        // so moving inside the Dock's top rim cannot postpone dismissal indefinitely.
        let grace = match preview.grace {
            Some(existing) if existing.area == area => existing,
            _ => LeaveGrace {
                area,
                deadline: now + BRIDGE_GRACE,
            },
        };
        preview.grace = Some(grace);
        let wait = grace.next_wait(now, true).unwrap_or(1);
        self.bottom.time_preview_leave(wait)
    }

    fn cancel_preview_grace(&mut self) {
        if let Some(preview) = self.preview.as_mut()
            && preview.grace.take().is_some()
        {
            self.bottom.cancel_preview_leave_timer();
        }
    }

    /// Updates Dock hover for a pointer at client `(x, y)` on the bottom bar, keeping an
    /// open preview alive while crossing the Dock's top rim toward it.
    pub(super) fn hover_dock_at(&mut self, x: i32, y: i32) -> Result<()> {
        if let Some(hit) = self.layout.hit(x, y) {
            self.cancel_preview_grace();
            return self.set_hover(Some(hit));
        }
        if let Some(bridge) = self.preview_bridge() {
            let sx = self.layout.bottom.x + x;
            let sy = self.layout.bottom.y + y;
            if bridge.contains(sx, sy) {
                return self.start_preview_grace(bridge);
            }
        }
        self.set_hover(None)
    }

    /// Handles the pointer leaving the Dock window without an active Dock drag.
    /// Keeps the preview open while moving into it or across its bridge.
    pub(super) fn leave_dock_preview(&mut self) -> Result<()> {
        if let Some(preview) = &self.preview
            && let Ok(point) = platform::cursor()
        {
            if preview.window.contains_screen_point(point.x, point.y) {
                self.cancel_preview_grace();
                return Ok(());
            }
            if let Some(bridge) = self.preview_bridge()
                && bridge.contains(point.x, point.y)
            {
                return self.start_preview_grace(bridge);
            }
        }
        self.set_hover(None)
    }

    /// Repaints the open preview without recovering from a lost device.
    pub(super) fn paint_preview(&mut self) -> Result<()> {
        let (Some(preview), Some(renderer)) = (&mut self.preview, self.renderer.as_mut()) else {
            return Ok(());
        };
        let blur = if !platform::display::transparency().unwrap_or(false) {
            None
        } else if self.settings.enabled(Appearance(WindowCustom)) {
            (self.settings.get(Appearance(WindowOpacity)) < 100
                && self.settings.get(Appearance(WindowBlur)) > 0)
                .then(|| self.settings.get(Appearance(WindowBlur)))
        } else {
            Some(70)
        };
        if preview.blur != blur {
            // GDI presents the chrome as opaque pixels, so even clear glass needs
            // the pixels under the preview. A denied capture keeps the opaque fill.
            preview.backdrop = blur.and_then(|strength| {
                Backdrop::behind(
                    preview.window.hwnd(),
                    preview.window.bounds(),
                    strength,
                    false,
                )
                .ok()
            });
            preview.blur = blur;
        }
        let pixels = renderer.paint_preview(
            preview.window.layout(),
            preview.window.windows(),
            preview.dpi,
            preview.hover,
            preview.backdrop.as_ref(),
        )?;
        preview.window.set_pixels(pixels);
        Ok(())
    }

    fn draw_preview(&mut self) -> Result<()> {
        match self.paint_preview() {
            Err(error) if is_device_lost(&error) => self.rebuild_renderer(),
            result => result,
        }
    }

    pub(super) fn retheme_preview(&mut self, dark: bool) -> Result<()> {
        if let Some(preview) = &self.preview {
            preview.window.set_dark(dark);
            self.draw_preview()?;
        }
        Ok(())
    }

    pub(super) fn update_preview(&mut self) {
        if !self.dock_shown() {
            self.close_preview();
            return;
        }
        let Some(DockHit::App(index)) = self.hover else {
            self.close_preview();
            return;
        };
        let Some(app) = self.apps.get(index) else {
            self.close_preview();
            return;
        };
        if app.windows.is_empty() {
            self.close_preview();
            return;
        }
        let anchor = self.icon_anchor(index);
        let anchor_xy = (anchor.x, anchor.y);
        let source_ids: Vec<isize> = app.windows.iter().map(|w| w.id).collect();
        if let Some(existing) = self.preview.as_mut()
            && existing.app_id == app.app_id
            && existing.source_ids == source_ids
            && existing.anchor == anchor_xy
            && existing.work == self.monitor.work
            && existing.dpi == self.monitor.dpi
        {
            if existing.window.update_titles(&app.windows) {
                let _ = self.draw_preview();
            }
            return;
        }

        let cursor = platform::cursor().ok();
        let previous = self
            .preview
            .as_ref()
            .filter(|existing| existing.app_id == app.app_id)
            .and_then(|existing| {
                cursor
                    .is_some_and(|point| existing.window.contains_screen_point(point.x, point.y))
                    .then(|| existing.window.bounds())
            });
        let app_id = app.app_id.clone();
        let windows = app.windows.clone();
        self.close_preview();

        match PreviewWindow::create(
            &windows,
            anchor.x,
            anchor.y,
            self.monitor.work,
            self.monitor.dpi,
            self.dark(),
        ) {
            Ok(Some(window)) => {
                let bounds = window.bounds();
                let hover = cursor.and_then(|point| {
                    bounds
                        .contains(point.x, point.y)
                        .then(|| window.layout().hit(point.x - bounds.x, point.y - bounds.y))
                        .flatten()
                });
                self.preview = Some(Preview {
                    app_id,
                    source_ids,
                    window,
                    backdrop: None,
                    blur: None,
                    anchor: anchor_xy,
                    work: self.monitor.work,
                    dpi: self.monitor.dpi,
                    hover,
                    pressed: None,
                    grace: None,
                });
                if let Err(error) = self.draw_preview() {
                    self.close_preview();
                    self.tray_notice = Some(format!("窗口预览不可用（{}）", error.code()));
                    platform::time_tray_notice(self.top.0);
                    return;
                }
                if let Some(preview) = &self.preview {
                    if let Err(error) = preview.window.reveal() {
                        self.close_preview();
                        self.tray_notice = Some(format!("窗口预览不可用（{}）", error.code()));
                        platform::time_tray_notice(self.top.0);
                        return;
                    }
                    if let Some(point) = cursor {
                        if preview.window.contains_screen_point(point.x, point.y) {
                            let _ = preview.window.track_leave();
                        } else if let Some(old_bounds) = previous {
                            let _ = self.start_preview_grace(old_bounds);
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.tray_notice = Some(format!("窗口预览不可用（{}）", error.code()));
                platform::time_tray_notice(self.top.0);
            }
        }
    }

    pub(super) fn preview_leave_timer(&mut self) -> Result<()> {
        let Some(grace) = self.preview.as_ref().and_then(|preview| preview.grace) else {
            return Ok(());
        };
        let point = platform::cursor()?;
        if self.preview.as_ref().is_some_and(|preview| {
            preview.pressed.is_some() || preview.window.contains_screen_point(point.x, point.y)
        }) {
            self.cancel_preview_grace();
            return Ok(());
        }
        if self.bottom.contains_screen_point(point.x, point.y) {
            let (cx, cy) = (
                point.x - self.layout.bottom.x,
                point.y - self.layout.bottom.y,
            );
            if let Some(hit) = self.layout.hit(cx, cy) {
                self.cancel_preview_grace();
                return self.set_hover(Some(hit));
            }
        }
        if let Some(wait) = grace.next_wait(Instant::now(), grace.area.contains(point.x, point.y)) {
            return self.bottom.time_preview_leave(wait);
        }
        self.set_hover(None)?;
        self.check_dock_pointer(false)
    }

    fn recheck_preview_pointer(&mut self) -> Result<()> {
        let Ok(point) = platform::cursor() else {
            self.set_hover(None)?;
            return self.check_dock_pointer(false);
        };
        if let Some(preview) = self.preview.as_mut()
            && preview.window.contains_screen_point(point.x, point.y)
        {
            let bounds = preview.window.bounds();
            let hover = preview
                .window
                .layout()
                .hit(point.x - bounds.x, point.y - bounds.y);
            if preview.hover != hover {
                preview.hover = hover;
                self.draw_preview()?;
            }
            return Ok(());
        }
        if self.bottom.contains_screen_point(point.x, point.y) {
            let _ = platform::track_leave(self.bottom.0);
            return self.hover_dock_at(
                point.x - self.layout.bottom.x,
                point.y - self.layout.bottom.y,
            );
        }
        self.set_hover(None)?;
        self.check_dock_pointer(false)
    }

    pub(super) fn preview_event(&mut self, event: PreviewEvent) -> Result<()> {
        match event {
            PreviewEvent::Move(x, y) => {
                self.cancel_preview_grace();
                let Some(preview) = self.preview.as_mut() else {
                    return Ok(());
                };
                let hover = preview.window.layout().hit(x, y);
                if preview.hover != hover {
                    preview.hover = hover;
                    self.draw_preview()?;
                }
            }
            PreviewEvent::Leave => {
                let Some(preview) = self.preview.as_mut() else {
                    return Ok(());
                };
                if preview.hover.take().is_some() {
                    self.draw_preview()?;
                }
                if self.preview_pressed() {
                    return Ok(());
                }
                if let Ok(point) = platform::cursor() {
                    if self.bottom.contains_screen_point(point.x, point.y) {
                        let _ = platform::track_leave(self.bottom.0);
                        return self.hover_dock_at(
                            point.x - self.layout.bottom.x,
                            point.y - self.layout.bottom.y,
                        );
                    }
                    if let Some(bridge) = self.preview_bridge()
                        && bridge.contains(point.x, point.y)
                    {
                        return self.start_preview_grace(bridge);
                    }
                }
                self.set_hover(None)?;
                self.check_dock_pointer(true)?;
            }
            PreviewEvent::Press(x, y) => {
                self.cancel_preview_grace();
                self.cancel_dock_press()?;
                let Some(preview) = self.preview.as_mut() else {
                    return Ok(());
                };
                if let Some(hit) = preview.window.layout().hit(x, y)
                    && let Some(window) = preview.window.windows().get(hit.index())
                {
                    preview.pressed = Some((hit, window.id));
                    preview.window.capture();
                }
            }
            PreviewEvent::Cancel => {
                if let Some(preview) = self.preview.as_mut()
                    && preview.pressed.take().is_some()
                {
                    preview.window.release();
                    self.recheck_preview_pointer()?;
                }
            }
            PreviewEvent::Click(x, y) => {
                let Some(preview) = self.preview.as_mut() else {
                    return Ok(());
                };
                let pressed = preview.pressed.take();
                preview.window.release();
                let hit = preview.window.layout().hit(x, y);
                let action = pressed.and_then(|(pressed_hit, pressed_id)| {
                    (hit == Some(pressed_hit)
                        && preview
                            .window
                            .windows()
                            .get(pressed_hit.index())
                            .is_some_and(|window| window.id == pressed_id))
                    .then_some((preview.app_id.clone(), pressed_hit, pressed_id))
                });
                match action {
                    Some((app_id, PreviewHit::Window(_), window_id)) => {
                        self.set_hover(None)?;
                        if let Err(error) = shell::activate(&app_id, window_id) {
                            platform::report_error(Some(self.bottom.0), &error);
                        }
                        self.refresh()?;
                        self.check_dock_pointer(false)?;
                    }
                    Some((app_id, PreviewHit::Close(_), window_id)) => {
                        if let Err(error) = shell::close_windows(&app_id, &[window_id]) {
                            self.set_hover(None)?;
                            platform::report_error(Some(self.bottom.0), &error);
                            self.refresh()?;
                            self.check_dock_pointer(false)?;
                        } else {
                            self.refresh()?;
                        }
                    }
                    None => self.recheck_preview_pointer()?,
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_grace_polls_inside_area_and_stops_when_left_or_expired() {
        let start = Instant::now();
        let grace = LeaveGrace {
            area: Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 12,
            },
            deadline: start + Duration::from_millis(100),
        };
        assert_eq!(grace.next_wait(start, true), Some(60));
        assert_eq!(
            grace.next_wait(start + Duration::from_millis(75), true),
            Some(25)
        );
        assert_eq!(
            grace.next_wait(start + Duration::from_millis(100), true),
            None
        );
        assert_eq!(
            grace.next_wait(start + Duration::from_millis(20), false),
            None
        );
    }
}
