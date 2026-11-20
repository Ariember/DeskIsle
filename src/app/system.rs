//! The status bar's system controls: keeping their state current, opening their
//! flyouts and carrying out the changes chosen there.

use super::Bars;
use crate::platform::{audio, display, media, network, power};
use crate::system::Display;
use crate::ui::layout::SystemItem;
use crate::ui::menu::{Command, Menu};
use crate::ui::system::{self, SystemCommand, SystemStatus};
use windows::core::Result;

/// Queries what the status bar icons show. A feature whose query fails shows as
/// unavailable until the next update rather than stopping DeskIsle.
pub(super) fn status() -> SystemStatus {
    SystemStatus {
        audio: audio::state().ok(),
        battery: power::battery().ok().flatten(),
        network: network::state(),
    }
}

impl Bars {
    /// Shows `status` on the bar when it differs from what is drawn.
    fn show_status(&mut self, status: SystemStatus) -> Result<()> {
        if self.system == status {
            return Ok(());
        }
        let relayout = self.system.items() != status.items();
        self.system = status;
        if relayout {
            self.restatus();
        }
        self.draw()
    }

    pub(super) fn refresh_system(&mut self) -> Result<()> {
        self.show_status(status())
    }

    /// After a volume notification from Windows.
    pub(super) fn refresh_audio(&mut self) -> Result<()> {
        let status = SystemStatus {
            audio: audio::state().ok(),
            ..self.system.clone()
        };
        self.show_status(status)
    }

    /// After Windows reports a change of power source or charge.
    pub(super) fn refresh_power(&mut self) -> Result<()> {
        let status = SystemStatus {
            battery: power::battery().ok().flatten(),
            ..self.system.clone()
        };
        self.show_status(status)
    }

    /// Follows the current default output, which the user may have switched since.
    pub(super) fn watch_audio(&mut self) {
        // Dropping the old watch first unregisters it from the previous device.
        self.audio_watch = None;
        // Without an output device the icon shows muted until the next update.
        self.audio_watch = audio::AudioWatch::new(self.top.0).ok();
    }

    /// The flyout of `item`, built from freshly queried state.
    pub(super) fn flyout_menu(&mut self, item: SystemItem) -> Menu<Command> {
        let mut status = self.system.clone();
        status.audio = audio::state().ok();
        if item == SystemItem::Battery {
            status.battery = power::battery().ok().flatten();
        }
        if matches!(item, SystemItem::Network | SystemItem::Control) {
            status.network = network::state();
        }
        let menu = match item {
            SystemItem::Sound => system::sound_flyout(status.audio.as_ref()),
            // A failed query lists no plans; the settings links still work.
            SystemItem::Battery => {
                system::battery_flyout(status.battery, &power::plans().unwrap_or_default())
            }
            SystemItem::Display => system::display_flyout(&display::state()),
            SystemItem::Network => system::network_flyout(&status.network),
            SystemItem::Control => {
                let display = Display {
                    brightness: display::brightness(),
                    dark: display::system_dark().unwrap_or(false),
                    hdr: None,
                };
                system::control_flyout(&status, &display, media::state().as_ref())
            }
        };
        self.system = status;
        menu
    }

    /// Opens the flyout of a status bar control, with the control highlighted meanwhile.
    pub(super) fn flyout(&mut self, item: SystemItem) -> Result<bool> {
        if matches!(item, SystemItem::Sound | SystemItem::Control) {
            self.watch_audio();
        }
        let menu = self.flyout_menu(item);
        let anchor = match self.status.system.iter().find(|(i, _)| *i == item) {
            Some((_, r)) => self.top_anchor(r.x + r.width / 2),
            None => self.cursor_anchor()?,
        };
        self.open_flyout = Some(item);
        self.draw()?;
        let choice = self.popup(menu, anchor, Some(item));
        self.open_flyout = None;
        // The flyout may have changed what the icons show.
        self.refresh_system()?;
        self.draw()?;
        match choice? {
            Some(command) => self.run_command(command),
            None => Ok(false),
        }
    }

    /// Applies a change chosen in a flyout.
    pub(super) fn system_command(&mut self, command: SystemCommand) -> Result<()> {
        match command {
            SystemCommand::SetMuted(muted) => audio::set_muted(muted),
            SystemCommand::SetOutput(id) => {
                let result = audio::set_default_output(&id);
                self.watch_audio();
                result
            }
            SystemCommand::SetPlan(id) => power::set_plan(id),
            // Windows broadcasts the change; the bars switch palette when it arrives.
            SystemCommand::SetDark(dark) => display::set_system_dark(dark),
            SystemCommand::SetHdr(on) => display::set_hdr(on),
            SystemCommand::SetRadio(kind, on) => network::set_radio(kind, on),
            SystemCommand::Media(action) => media::control(action),
        }
    }
}
