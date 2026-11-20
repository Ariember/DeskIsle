use super::Bars;
use crate::dock::{self, Action, group_apps};
use crate::platform::{self, shell};
use crate::storage;
use crate::ui::menu::{self, Command};
use windows::core::Result;

impl Bars {
    /// Carries out a menu command. Failures are reported and leave DeskIsle running.
    pub(super) fn run_command(&mut self, command: Command) -> Result<bool> {
        let owner = self.bottom.0;
        let result = match command {
            Command::Quit => return Ok(true),
            Command::Select(id) => {
                if let Some(index) = self.apps.iter().position(|app| app.app_id == id) {
                    self.select(index)?;
                }
                return Ok(false);
            }
            Command::Window(id, window) => shell::activate(&id, window).map(drop),
            Command::NewInstance(path) => shell::execute(owner, &path),
            Command::Reveal(path) => shell::reveal(owner, &path),
            Command::Pin(path) => {
                dock::pin(&mut self.pins, &path);
                storage::save_pins(&self.pins)
            }
            Command::Unpin(id) => {
                self.pins.retain(|pin| pin.app_id != id);
                storage::save_pins(&self.pins)
            }
            Command::Close(id, windows) => shell::close_windows(&id, &windows).map(drop),
            Command::TaskManager => shell::execute(owner, "taskmgr.exe"),
            Command::ShowDesktop => self.tray.toggle_desktop(),
            Command::Settings => self.open_settings(),
            Command::DateTime => shell::execute(owner, "ms-settings:dateandtime"),
            Command::Notifications => shell::execute(owner, "ms-actioncenter:"),
            Command::TrayActivate(index) => {
                self.tray_activate(index, false, self.tray_overflow_anchor()?)?;
                return Ok(false);
            }
            Command::Open(uri) => shell::execute(owner, uri),
            Command::System(command) => self.system_command(command),
            Command::TrayToggle(index) => {
                self.tray_toggle(index)?;
                return Ok(false);
            }
        };
        if let Err(error) = result {
            platform::report_error(Some(owner), &error);
        }
        self.refresh()?;
        Ok(false)
    }

    pub(super) fn select(&mut self, index: usize) -> Result<()> {
        let target = &self.apps[index];
        let id = target.app_id.clone();
        let was_running = !target.windows.is_empty();
        if let Err(error) = self.act(&id, was_running) {
            platform::report_error(Some(self.bottom.0), &error);
        }
        self.refresh()
    }

    fn act(&mut self, id: &str, was_running: bool) -> Result<()> {
        let (snapshot, foreground) = shell::enumerate()?;
        let apps = group_apps(&self.pins, &snapshot, foreground.unwrap_or_default());
        let Some(app) = apps.iter().find(|app| app.app_id == id) else {
            return Ok(());
        };
        match app.action() {
            Action::Launch(path) => {
                if !was_running {
                    shell::launch(self.bottom.0, path, &snapshot)?;
                }
            }
            Action::Activate(window) => {
                shell::activate(id, window.id)?;
            }
            Action::Choose(_) => {
                let menu = menu::window_menu(app, foreground);
                let anchor = match self.apps.iter().position(|app| app.app_id == id) {
                    Some(index) => self.icon_anchor(index),
                    None => self.cursor_anchor()?,
                };
                if let Some(Command::Window(id, window)) = self.popup(menu, anchor, None)? {
                    shell::activate(&id, window)?;
                }
            }
        }
        Ok(())
    }
}
