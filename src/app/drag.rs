use std::path::PathBuf;

use super::Bars;
use crate::dock::{AppGroup, PinnedApp, insert_pins, move_pin, moved_pin_index, pinned};
use crate::platform::{self, shell};
use crate::ui::layout::{DockDrop, DockHit};
use windows::Win32::Foundation::{E_INVALIDARG, HWND};
use windows::core::{Error, Result};

pub(super) struct DockPress {
    /// None identifies the launchpad button. App indices may change during a press.
    app_id: Option<String>,
    origin: (i32, i32),
    threshold: (i32, i32),
    dragging: bool,
    target: Option<DockDrop>,
}

impl DockPress {
    fn source(&self, apps: &[AppGroup]) -> Option<usize> {
        let id = self.app_id.as_ref()?;
        apps.iter().position(|app| &app.app_id == id)
    }

    fn moved(&mut self, x: i32, y: i32) -> bool {
        self.dragging |= self.origin.0.abs_diff(x) >= self.threshold.0 as u32
            || self.origin.1.abs_diff(y) >= self.threshold.1 as u32;
        self.dragging
    }
}

fn accessible_files(files: Vec<PathBuf>) -> (Vec<PathBuf>, usize) {
    let count = files.len();
    let files: Vec<_> = files
        .into_iter()
        .filter(|p| crate::launchpad::supported(p) && p.is_file())
        .collect();
    let rejected = count - files.len();
    (files, rejected)
}

impl Bars {
    /// Use the visible gap for both hovering and release. New pins go before running
    /// apps even if the pointer is farther along the Dock.
    pub(super) fn file_target(&self, x: i32, y: i32) -> Option<DockDrop> {
        self.layout
            .file_drop_target(x, y, self.dock_file_target, self.pins.len())
    }

    pub(super) fn set_dock_file_target(&mut self, target: Option<DockDrop>) -> Result<()> {
        if self.dock_file_target != target {
            self.dock_file_target = target;
            if target.is_some() {
                self.hover = None;
                self.preview = None;
            }
            self.draw()?;
        }
        Ok(())
    }

    fn apply_drop_pins(&mut self, pins: Vec<PinnedApp>) -> Result<()> {
        match self.save_pins(pins) {
            Ok(true) => self.refresh()?,
            Ok(false) => {}
            Err(error) => platform::report_error(Some(self.bottom.0), &error),
        }
        Ok(())
    }

    pub(super) fn import_files(
        &mut self,
        owner: HWND,
        target: DockDrop,
        files: Vec<PathBuf>,
    ) -> Result<()> {
        let (files, rejected) = accessible_files(files);
        if !files.is_empty() {
            match target {
                DockDrop::Launchpad => {
                    if let Err(error) = self.add_launchpad_files(files) {
                        platform::report_error(Some(owner), &error);
                    }
                }
                DockDrop::Insert(at) => {
                    let mut pins = self.pins.clone();
                    insert_pins(&mut pins, at, files.into_iter().map(shell::pin_from_path));
                    self.apply_drop_pins(pins)?;
                }
            }
        }
        if rejected > 0 {
            let error = Error::new(
                E_INVALIDARG,
                format!("有 {rejected} 个项目无法添加。请拖入可访问的 .lnk、.exe 或 .url 文件。"),
            );
            platform::report_error(Some(owner), &error);
        }
        Ok(())
    }

    pub(super) fn press_dock(&mut self, x: i32, y: i32) {
        let app_id = match self.layout.hit(x, y) {
            Some(DockHit::Launchpad) => None,
            Some(DockHit::App(i)) => Some(self.apps[i].app_id.clone()),
            None => return,
        };
        self.bottom.capture();
        self.dock_press = Some(DockPress {
            app_id,
            origin: (x, y),
            threshold: platform::drag_threshold(self.monitor.dpi),
            dragging: false,
            target: None,
        });
    }

    pub(super) fn cancel_dock_press(&mut self) -> Result<()> {
        if let Some(press) = self.dock_press.take() {
            self.bottom.release();
            if press.target.is_some() {
                self.draw()?;
            }
            self.finish_dock_gesture()?;
        }
        Ok(())
    }

    pub(super) fn leave_dock(&mut self) -> Result<()> {
        if let Some(press) = self.dock_press.as_mut()
            && press.target.take().is_some()
        {
            self.draw()?;
        }
        self.set_hover(None)
    }

    fn drag_target(&self, source: usize, x: i32, y: i32) -> Option<DockDrop> {
        self.layout.drop_target(x, y, Some(source)).or_else(|| {
            let dock = self.layout.bottom;
            self.launchpad
                .as_ref()
                .filter(|pad| pad.contains_screen_point(dock.x + x, dock.y + y))
                .map(|_| DockDrop::Launchpad)
        })
    }

    pub(super) fn move_dock_pointer(&mut self, x: i32, y: i32) -> Result<()> {
        if self.dock_file_target.is_some() {
            return Ok(());
        }
        let Some(press) = self.dock_press.as_mut() else {
            return self.set_hover(self.layout.hit(x, y));
        };
        if !press.moved(x, y) {
            return Ok(());
        }
        let source = press
            .source(&self.apps)
            .filter(|&i| self.apps[i].executable().is_some());
        let previous = press.target;
        let target = source.and_then(|i| self.drag_target(i, x, y));
        if target != previous {
            self.dock_press.as_mut().unwrap().target = target;
            self.draw()?;
        }
        Ok(())
    }

    /// Whether the app being dragged would go into the launchpad if released now.
    pub(super) fn dropping_on_launchpad(&self) -> bool {
        self.launchpad_file_hover()
            || self.dock_file_target == Some(DockDrop::Launchpad)
            || self
                .dock_press
                .as_ref()
                .is_some_and(|press| press.target == Some(DockDrop::Launchpad))
    }

    /// Preview the order without changing the window groups used for hit testing.
    pub(super) fn dock_preview(&self) -> (Option<Vec<usize>>, Option<DockHit>) {
        if self.dock_file_target.is_some() {
            return (None, None);
        }
        let Some(press) = &self.dock_press else {
            return (None, self.hover);
        };
        match press.target {
            Some(DockDrop::Launchpad) => (None, Some(DockHit::Launchpad)),
            Some(DockDrop::Insert(at)) => {
                let Some(from) = press.source(&self.apps) else {
                    return (None, None);
                };
                let to = moved_pin_index(self.pins.len(), from, at);
                let mut order: Vec<_> = (0..self.apps.len()).collect();
                order.remove(from);
                order.insert(to, from);
                (Some(order), Some(DockHit::App(to)))
            }
            None => (None, None),
        }
    }

    pub(super) fn release_dock(&mut self, x: i32, y: i32) -> Result<()> {
        let Some(mut press) = self.dock_press.take() else {
            return Ok(());
        };
        self.bottom.release();
        if press.moved(x, y) {
            if let Some(source) = press.source(&self.apps)
                && let Some(target) = self.drag_target(source, x, y)
                && let Some(path) = self.apps[source].executable().map(|p| p.to_owned())
            {
                match target {
                    DockDrop::Launchpad => self.import_files(self.bottom.0, target, vec![path])?,
                    DockDrop::Insert(at) => {
                        let mut pins = self.pins.clone();
                        let app = self
                            .pins
                            .get(source)
                            .cloned()
                            .unwrap_or_else(|| pinned(path));
                        move_pin(&mut pins, at, app);
                        self.apply_drop_pins(pins)?;
                    }
                }
            }
            // Clear the preview even when the drop was cancelled or its save failed.
            if press.target.is_some() {
                self.draw()?;
            }
        } else {
            match self.layout.hit(x, y) {
                Some(DockHit::Launchpad) if press.app_id.is_none() => {
                    if let Err(error) = self.open_launchpad() {
                        platform::report_error(Some(self.bottom.0), &error);
                    }
                }
                Some(DockHit::App(i)) if Some(&self.apps[i].app_id) == press.app_id.as_ref() => {
                    self.select(i)?;
                }
                _ => {}
            }
        }
        self.finish_dock_gesture()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returning_to_origin_after_drag_does_not_click() {
        let mut press = DockPress {
            app_id: Some("editor".into()),
            origin: (20, 40),
            threshold: (4, 6),
            dragging: false,
            target: None,
        };
        assert!(!press.moved(23, 35));
        assert!(press.moved(16, 40));
        assert!(press.moved(20, 40));
    }

    #[test]
    fn mixed_drop_keeps_accessible_programs() {
        let exe = std::env::current_exe().unwrap();
        let (files, rejected) = accessible_files(vec![exe.clone(), PathBuf::from("relative.url")]);
        assert_eq!(files, [exe]);
        assert_eq!(rejected, 1);
    }
}
