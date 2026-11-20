use std::collections::BTreeSet;
use std::path::PathBuf;

use super::Bars;
use crate::dock::path_id;
use crate::geometry::Rect;
use crate::platform::launchpad::{LaunchpadEvent, LaunchpadKey, LaunchpadWindow, confirm_remove};
use crate::platform::{self, shell};
use crate::render::is_device_lost;
use crate::storage;
use crate::ui::launchpad::{LaunchpadHit, LaunchpadLayout};
use windows::Win32::Foundation::E_INVALIDARG;
use windows::core::{Error, Result};

pub(super) struct Launchpad {
    window: LaunchpadWindow,
    rect: Rect,
    dpi: u32,
    paths: Vec<PathBuf>,
    query: String,
    layout: LaunchpadLayout,
    wheel: i32,
    hover: Option<usize>,
}

impl Launchpad {
    fn layout_page(&mut self, page: usize) {
        self.layout = LaunchpadLayout::new(
            self.rect.width,
            self.rect.height,
            self.dpi,
            &self.paths,
            &self.query,
            page,
        );
        self.hover = None;
    }

    fn save(&mut self, paths: Vec<PathBuf>) -> Result<()> {
        storage::save_launchpad(&paths)?;
        self.paths = paths;
        Ok(())
    }

    fn add_files(&mut self, files: Vec<PathBuf>) -> Result<bool> {
        let (paths, rejected) = dropped_entries(&self.paths, files);
        let added = paths.len() != self.paths.len();
        if added {
            self.save(paths)?;
            self.query.clear();
            self.layout_page(usize::MAX);
        }
        if rejected > 0 {
            platform::report_error(
                Some(self.window.0),
                &Error::new(
                    E_INVALIDARG,
                    format!(
                        "有 {rejected} 个项目无法添加。请拖入可访问的 .lnk、.exe 或 .url 文件。"
                    ),
                ),
            );
        }
        Ok(added)
    }
}

/// Keep the original dropped paths, including shortcut arguments stored by the Shell.
fn dropped_entries(existing: &[PathBuf], dropped: Vec<PathBuf>) -> (Vec<PathBuf>, usize) {
    let mut next = existing.to_vec();
    let mut known: BTreeSet<_> = existing.iter().map(|p| path_id(p)).collect();
    let mut rejected = 0;
    for path in dropped {
        if !crate::launchpad::supported(&path) || !path.is_file() {
            rejected += 1;
        } else if known.insert(path_id(&path)) {
            next.push(path);
        }
    }
    (next, rejected)
}

impl Bars {
    fn launchpad_rect(&self) -> Rect {
        let work = self.monitor.work;
        let top = work.y.max(self.top_rect.y + self.top_rect.height);
        Rect {
            y: top,
            height: (self.layout.bottom.y - top).max(1),
            ..work
        }
    }

    pub(super) fn open_launchpad(&mut self) -> Result<()> {
        if self.launchpad.is_some() {
            self.close_launchpad();
            return Ok(());
        }
        let paths = storage::load_launchpad()?;
        let rect = self.launchpad_rect();
        let dpi = self.monitor.dpi;
        let layout = LaunchpadLayout::new(rect.width, rect.height, dpi, &paths, "", 0);
        let window = LaunchpadWindow::create(rect)?;
        self.launchpad = Some(Launchpad {
            window,
            rect,
            dpi,
            paths,
            query: String::new(),
            layout,
            wheel: 0,
            hover: None,
        });
        if let Err(error) = self.draw_launchpad() {
            self.close_launchpad();
            return Err(error);
        }
        if let Some(pad) = &self.launchpad {
            pad.window.show();
        }
        Ok(())
    }

    fn close_launchpad(&mut self) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.close_launchpad();
        }
        self.launchpad = None;
    }

    pub(super) fn resize_launchpad(&mut self) -> Result<()> {
        let rect = self.launchpad_rect();
        let Some(pad) = &mut self.launchpad else {
            return Ok(());
        };
        if (pad.rect, pad.dpi) == (rect, self.monitor.dpi) {
            return Ok(());
        }
        pad.window.place(rect)?;
        pad.rect = rect;
        pad.dpi = self.monitor.dpi;
        pad.layout_page(pad.layout.page);
        self.draw_launchpad()
    }

    pub(super) fn paint_launchpad(&mut self) -> Result<()> {
        if let (Some(pad), Some(renderer)) = (&self.launchpad, self.renderer.as_mut()) {
            renderer.draw_launchpad(pad.window.0, &pad.layout, &pad.paths, &pad.query)?;
            if pad.hover.is_some() {
                renderer.hover_launchpad(&pad.layout, pad.hover)?;
            }
        }
        Ok(())
    }

    fn draw_launchpad(&mut self) -> Result<()> {
        match self.paint_launchpad() {
            Err(error) if is_device_lost(&error) => self.rebuild_renderer(),
            result => result,
        }
    }

    pub(super) fn launchpad_event(&mut self, input: LaunchpadEvent) -> Result<()> {
        let Some(pad) = &mut self.launchpad else {
            return Ok(());
        };
        let owner = pad.window.0;
        let page = pad.layout.page;
        let next_page = match input {
            LaunchpadEvent::Close | LaunchpadEvent::Key(LaunchpadKey::Escape) => {
                self.close_launchpad();
                return Ok(());
            }
            LaunchpadEvent::Click(x, y) => match pad.layout.hit(x, y) {
                LaunchpadHit::Item(index) => return self.start_launchpad_item(index),
                LaunchpadHit::Page(next) => next,
                LaunchpadHit::Search => return Ok(()),
                LaunchpadHit::Background => {
                    self.close_launchpad();
                    return Ok(());
                }
            },
            LaunchpadEvent::Key(LaunchpadKey::Enter) => {
                if let Some(index) = pad.layout.first_item() {
                    return self.start_launchpad_item(index);
                }
                return Ok(());
            }
            LaunchpadEvent::Context(x, y) => {
                let LaunchpadHit::Item(index) = pad.layout.hit(x, y) else {
                    return Ok(());
                };
                if !confirm_remove(owner, &pad.paths[index]) {
                    return Ok(());
                }
                let mut next = pad.paths.clone();
                next.remove(index);
                if let Err(error) = pad.save(next) {
                    platform::report_error(Some(owner), &error);
                    return Ok(());
                }
                page
            }
            LaunchpadEvent::DropFiles(files) => {
                match pad.add_files(files) {
                    Ok(true) => return self.draw_launchpad(),
                    Ok(false) => {}
                    Err(error) => platform::report_error(Some(owner), &error),
                }
                return Ok(());
            }
            LaunchpadEvent::Text(ch) => {
                pad.query.push(ch);
                0
            }
            LaunchpadEvent::Key(LaunchpadKey::Backspace) => {
                if pad.query.pop().is_none() {
                    return Ok(());
                }
                0
            }
            LaunchpadEvent::Key(LaunchpadKey::PageUp | LaunchpadKey::PageDown)
            | LaunchpadEvent::Scroll(_) => {
                let offset = match input {
                    LaunchpadEvent::Scroll(delta) => {
                        pad.wheel += i32::from(delta);
                        let steps = pad.wheel / 120;
                        pad.wheel %= 120;
                        -steps as isize
                    }
                    LaunchpadEvent::Key(LaunchpadKey::PageUp) => -1,
                    _ => 1,
                };
                let next = page.saturating_add_signed(offset).min(pad.layout.pages - 1);
                if next == page {
                    return Ok(());
                }
                next
            }
            LaunchpadEvent::Move(..) | LaunchpadEvent::Leave => {
                let hover = match input {
                    LaunchpadEvent::Move(x, y) => match pad.layout.hit(x, y) {
                        LaunchpadHit::Item(index) => Some(index),
                        _ => None,
                    },
                    _ => None,
                };
                if hover != pad.hover {
                    pad.hover = hover;
                    if let Some(renderer) = &mut self.renderer {
                        return match renderer.hover_launchpad(&pad.layout, hover) {
                            Err(error) if is_device_lost(&error) => self.rebuild_renderer(),
                            result => result,
                        };
                    }
                }
                return Ok(());
            }
        };
        pad.layout_page(next_page);
        self.draw_launchpad()
    }

    fn start_launchpad_item(&mut self, index: usize) -> Result<()> {
        let Some(pad) = &self.launchpad else {
            return Ok(());
        };
        match shell::execute(pad.window.0, &pad.paths[index]) {
            Ok(()) => self.close_launchpad(),
            Err(error) => platform::report_error(Some(pad.window.0), &error),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_drop_adds_existing_program_once_and_keeps_shortcuts_in_order() {
        let saved = vec![PathBuf::from(r"C:\我的应用\快捷方式.lnk")];
        let exe = std::env::current_exe().unwrap();
        let files = vec![exe.clone(), PathBuf::from("relative.url"), exe.clone()];
        let (entries, rejected) = dropped_entries(&saved, files);
        assert_eq!(entries, [saved[0].clone(), exe]);
        assert_eq!(rejected, 1);
    }
}
