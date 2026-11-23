use std::path::Path;

use super::Bars;
use crate::platform::shell;
use crate::settings::{AppearanceKey as A, Key};
use crate::ui::menu::{self, Anchor, Command};
use windows::core::Result;

impl Bars {
    pub(super) fn open_folder(&mut self, folder: &Path, anchor: Anchor) -> Result<()> {
        let paths = shell::folder_entries(folder)?;
        let title = folder
            .file_name()
            .unwrap_or(folder.as_os_str())
            .to_string_lossy();
        let size = if self.settings.enabled(Key::Appearance(A::FolderIconCustom)) {
            self.settings.get(Key::Appearance(A::FolderIconSize)) as f32
        } else {
            18.
        };
        let available = (anchor.y - self.monitor.work.y) as f32 * 96. / self.monitor.dpi as f32;
        let mut page = 0;
        loop {
            let list = menu::folder_menu(folder, &title, &paths, size, available, page);
            match self.popup(list, anchor, None)? {
                Some(Command::FolderPage(next)) => page = next,
                Some(Command::OpenDocument(path)) => return shell::execute(self.bottom.0, path),
                _ => return Ok(()),
            }
        }
    }
}
