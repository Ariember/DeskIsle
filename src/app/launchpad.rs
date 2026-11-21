use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::Bars;
use crate::dock::path_id;
use crate::geometry::Rect;
use crate::platform::launchpad::{
    LaunchpadEvent, LaunchpadKey, LaunchpadWindow, primary_button_down,
};
use crate::platform::{self, shell};
use crate::render::is_device_lost;
use crate::storage;
use crate::ui::launchpad::{LaunchpadHit, LaunchpadLayout};
use crate::ui::layout::DockDrop;
use crate::ui::menu::{self, Anchor, Command};
use windows::core::Result;

pub(super) struct Launchpad {
    window: LaunchpadWindow,
    rect: Rect,
    /// Pixels from the top of the window that stay clear of the Dock.
    content: i32,
    dpi: u32,
    paths: Vec<PathBuf>,
    query: String,
    layout: LaunchpadLayout,
    wheel: i32,
    hover: Option<usize>,
    file_hover: bool,
    pressed: bool,
    /// The click that dismissed a tile's menu, which must not also close the launchpad.
    swallow_press: bool,
}

impl Launchpad {
    pub(super) fn contains_screen_point(&self, x: i32, y: i32) -> bool {
        self.rect.contains(x, y) && self.window.contains_screen_point(x, y)
    }

    fn layout_page(&mut self, page: usize) {
        self.layout = LaunchpadLayout::new(
            self.rect.width,
            self.rect.height,
            self.content,
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
}

/// Keep the original dropped paths, including shortcut arguments stored by the Shell.
fn append_entries(entries: &mut Vec<PathBuf>, dropped: Vec<PathBuf>) -> bool {
    let before = entries.len();
    let mut known: BTreeSet<_> = entries.iter().map(|p| path_id(p)).collect();
    entries.extend(dropped.into_iter().filter(|p| known.insert(path_id(p))));
    entries.len() != before
}

impl Bars {
    pub(super) fn add_launchpad_files(&mut self, files: Vec<PathBuf>) -> Result<()> {
        let mut entries = match &self.launchpad {
            Some(pad) => pad.paths.clone(),
            None => storage::load_launchpad()?,
        };
        if append_entries(&mut entries, files) {
            if let Some(pad) = &mut self.launchpad {
                pad.save(entries)?;
                pad.query.clear();
                pad.layout_page(usize::MAX);
                self.draw_launchpad()?;
            } else {
                storage::save_launchpad(&entries)?;
            }
            if let Some(renderer) = &self.renderer {
                // The entry is already saved even if the confirmation cannot animate.
                let _ = renderer.confirm_launchpad_drop();
            }
        }
        Ok(())
    }

    /// The window covers everything below the status bar down to the bottom of the
    /// screen, including the strip Explorer's hidden taskbar leaves behind. The Dock is
    /// topmost and stays above it; the second value is the height left above the Dock.
    fn launchpad_rect(&self) -> (Rect, i32) {
        let work = self.monitor.work;
        let bounds = self.monitor.bounds;
        let top = work.y.max(self.top_rect.y + self.top_rect.height);
        let bottom = (bounds.y + bounds.height).max(top + 1);
        let rect = Rect {
            y: top,
            height: bottom - top,
            ..work
        };
        let content = (self.layout.bottom.y - top).clamp(1, rect.height);
        (rect, content)
    }

    pub(super) fn open_launchpad(&mut self) -> Result<()> {
        if self.launchpad.is_some() {
            self.close_launchpad();
            return Ok(());
        }
        let paths = storage::load_launchpad()?;
        let (rect, content) = self.launchpad_rect();
        let dpi = self.monitor.dpi;
        let layout = LaunchpadLayout::new(rect.width, rect.height, content, dpi, &paths, "", 0);
        let window = LaunchpadWindow::create(rect)?;
        self.launchpad = Some(Launchpad {
            window,
            rect,
            content,
            dpi,
            paths,
            query: String::new(),
            layout,
            wheel: 0,
            hover: None,
            file_hover: false,
            pressed: false,
            swallow_press: false,
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
        let (rect, content) = self.launchpad_rect();
        let Some(pad) = &mut self.launchpad else {
            return Ok(());
        };
        if (pad.rect, pad.content, pad.dpi) == (rect, content, self.monitor.dpi) {
            return Ok(());
        }
        if pad.rect != rect {
            pad.window.place(rect)?;
        }
        pad.rect = rect;
        pad.content = content;
        pad.dpi = self.monitor.dpi;
        pad.layout_page(pad.layout.page);
        self.draw_launchpad()
    }

    pub(super) fn paint_launchpad(&mut self) -> Result<()> {
        let dropping = self.dropping_on_launchpad();
        if let (Some(pad), Some(renderer)) = (&self.launchpad, self.renderer.as_mut()) {
            renderer.draw_launchpad(pad.window.0, &pad.layout, &pad.paths, &pad.query)?;
            if pad.hover.is_some() {
                renderer.hover_launchpad(&pad.layout, pad.hover)?;
            }
            renderer.deepen_launchpad(dropping)?;
        }
        Ok(())
    }

    pub(super) fn draw_launchpad(&mut self) -> Result<()> {
        match self.paint_launchpad() {
            Err(error) if is_device_lost(&error) => self.rebuild_renderer(),
            result => result,
        }
    }

    pub(super) fn launchpad_file_hover(&self) -> bool {
        self.launchpad.as_ref().is_some_and(|pad| pad.file_hover)
    }

    pub(super) fn launchpad_event(&mut self, input: LaunchpadEvent) -> Result<()> {
        // A non-activating Dock may receive capture only within its own window.
        // A new press here or a move after release ends any stale Dock gesture.
        if matches!(
            input,
            LaunchpadEvent::Press
                | LaunchpadEvent::Move {
                    left_down: false,
                    ..
                }
        ) {
            self.cancel_dock_press()?;
        }
        let Some(pad) = &mut self.launchpad else {
            return Ok(());
        };
        let owner = pad.window.0;
        let page = pad.layout.page;
        let dock = self.layout.bottom;
        let dock_client = |x: i32, y: i32| (pad.rect.x + x - dock.x, pad.rect.y + y - dock.y);
        let next_page = match input {
            LaunchpadEvent::Press => {
                pad.pressed = !std::mem::take(&mut pad.swallow_press);
                return Ok(());
            }
            LaunchpadEvent::Move { x, y, .. } if self.dock_press.is_some() => {
                let (x, y) = dock_client(x, y);
                return self.move_dock_pointer(x, y);
            }
            LaunchpadEvent::Click(x, y) if self.dock_press.is_some() => {
                pad.pressed = false;
                let (x, y) = dock_client(x, y);
                return self.release_dock(x, y);
            }
            LaunchpadEvent::Click(..) if !std::mem::take(&mut pad.pressed) => {
                pad.swallow_press = false;
                return Ok(());
            }
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
                return self.launchpad_menu(index);
            }
            LaunchpadEvent::DropFiles(files) => {
                if std::mem::take(&mut pad.file_hover) {
                    self.draw()?;
                }
                return self.import_files(owner, DockDrop::Launchpad, files);
            }
            LaunchpadEvent::FileHover(hover) => {
                if pad.file_hover != hover {
                    pad.file_hover = hover;
                    return self.draw();
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
            LaunchpadEvent::Move { .. } | LaunchpadEvent::Leave => {
                let hover = match input {
                    LaunchpadEvent::Move { x, y, left_down } => {
                        pad.pressed &= left_down;
                        match pad.layout.hit(x, y) {
                            LaunchpadHit::Item(index) => Some(index),
                            _ => None,
                        }
                    }
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

    /// DeskIsle's own menu for a tile, opening below it the way the Dock's menus open
    /// above their icons.
    fn launchpad_menu(&mut self, index: usize) -> Result<()> {
        let Some(pad) = &self.launchpad else {
            return Ok(());
        };
        let Some(tile) = pad.layout.tiles.iter().find(|tile| tile.index == index) else {
            return Ok(());
        };
        let scale = pad.dpi as f32 / 96.;
        let anchor = Anchor {
            x: pad.rect.x + ((tile.icon.left + tile.icon.right) / 2. * scale).round() as i32,
            y: pad.rect.y + (tile.label.bottom * scale).round() as i32,
            up: false,
        };
        let path = pad.paths[index].clone();
        let choice = self.popup(menu::launchpad_menu(&path), anchor, None)?;
        let Some(pad) = &mut self.launchpad else {
            return Ok(());
        };
        let owner = pad.window.0;
        let Some(command) = choice else {
            // A press on the launchpad deactivated the menu; its release is not a click
            // on the background.
            if primary_button_down()
                && let Ok(point) = platform::cursor()
                && pad.contains_screen_point(point.x, point.y)
            {
                pad.swallow_press = true;
            }
            return Ok(());
        };
        // Starting something hands the screen to it, as a click on the tile does.
        let close = match &command {
            Command::NewInstance(path) => shell::execute(owner, path).map(|()| true),
            Command::RunAsAdmin(path) => shell::run_as_admin(owner, path),
            Command::Reveal(path) => shell::reveal(owner, path).map(|()| true),
            Command::Properties(path) => shell::properties(owner, path).map(|()| false),
            Command::LaunchpadRemove(path) => return self.remove_launchpad_entry(path),
            _ => return self.run_command(command).map(drop),
        };
        match close {
            Ok(true) => self.close_launchpad(),
            Ok(false) => {}
            Err(error) => platform::report_error(Some(owner), &error),
        }
        Ok(())
    }

    /// Takes `path` out of the launchpad. The file itself stays where it is.
    pub(super) fn remove_launchpad_entry(&mut self, path: &Path) -> Result<()> {
        let Some(pad) = &mut self.launchpad else {
            let mut entries = storage::load_launchpad()?;
            let before = entries.len();
            entries.retain(|entry| entry != path);
            if entries.len() != before {
                storage::save_launchpad(&entries)?;
            }
            return Ok(());
        };
        let mut next = pad.paths.clone();
        next.retain(|entry| entry != path);
        if next.len() == pad.paths.len() {
            return Ok(());
        }
        if let Err(error) = pad.save(next) {
            platform::report_error(Some(pad.window.0), &error);
            return Ok(());
        }
        pad.layout_page(pad.layout.page);
        self.draw_launchpad()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_import_preserves_original_shortcuts_and_order() {
        let saved = PathBuf::from(r"C:\我的应用\快捷方式.lnk");
        let exe = PathBuf::from(r"C:\Apps\Editor.exe");
        let mut entries = vec![saved.clone()];
        let files = vec![exe.clone(), saved.clone(), exe.clone()];
        assert!(append_entries(&mut entries, files.clone()));
        assert!(!append_entries(&mut entries, files));
        assert_eq!(entries, [saved, exe]);
    }
}
