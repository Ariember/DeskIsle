//! The status bar's system controls: keeping their state current, opening their
//! flyouts and carrying out the changes chosen there.

use super::Bars;
use crate::platform::{audio, display, power, system_worker::Snapshot};
use crate::system::{Display, Network};
use crate::ui::layout::{StatusHit, SystemItem};
use crate::ui::menu::{Command, Menu};
use crate::ui::system::{self, SystemCommand, SystemStatus};
use windows::Win32::Foundation::HWND;
use windows::core::Result;

/// Wheel movement of one notch, as Windows reports it.
const NOTCH: i32 = 120;

/// Queries what the status bar icons show. A feature whose query fails shows as
/// unavailable until the next update rather than stopping DeskIsle.
pub(super) fn status(network: Network) -> SystemStatus {
    SystemStatus {
        audio: audio::state().ok(),
        battery: power::battery().ok().flatten(),
        network,
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
        if self.open_flyout.is_none() {
            self.draw()?;
        }
        // The flyout draws the bars after it releases its composition target.
        Ok(())
    }

    pub(super) fn refresh_system(&mut self) -> Result<()> {
        self.worker.refresh_network()?;
        self.show_status(status(self.system.network.clone()))
    }

    pub(super) fn system_updated(&mut self, owner: HWND) -> Result<Option<SystemItem>> {
        let Some(update) = self.worker.take_update() else {
            return Ok(None);
        };
        if let Err(error) = update.result {
            crate::platform::report_error(Some(owner), &error);
        }
        match update.snapshot {
            Snapshot::Network(network) if network != self.system.network => {
                self.show_status(SystemStatus {
                    network,
                    ..self.system.clone()
                })?;
                Ok(Some(SystemItem::Network))
            }
            Snapshot::Media(media) if media != self.media => {
                self.media = media;
                Ok(Some(SystemItem::Control))
            }
            _ => Ok(None),
        }
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
        if let Some(watch) = &mut self.audio_watch {
            watch.bind_default();
        } else {
            self.audio_watch = audio::AudioWatch::new(self.top.0).ok();
        }
    }

    /// Turns the default output's volume by wheel `delta` over the sound icon. Wheel
    /// turns elsewhere on the status bar do nothing.
    pub(super) fn scroll_status(&mut self, delta: i16, x: i32, y: i32) -> Result<()> {
        if self.status.hit(x, y) != StatusHit::System(SystemItem::Sound) {
            self.wheel = 0;
            return Ok(());
        }
        // Precision touchpads send fractions of a notch; collect them into whole steps.
        self.wheel += i32::from(delta);
        let notches = self.wheel / NOTCH;
        self.wheel %= NOTCH;
        let Ok(state) = audio::state() else {
            return Ok(());
        };
        let Some(volume) = system::wheel_volume(&state, notches) else {
            return Ok(());
        };
        if let Err(error) = audio::set_volume(volume) {
            crate::platform::report_error(Some(self.top.0), &error);
        }
        // Windows reports the new volume, which redraws the icon.
        Ok(())
    }

    /// Mutes or unmutes the default output, as a middle click on the sound icon does.
    pub(super) fn toggle_mute(&self) {
        let result = audio::state().and_then(|state| audio::set_muted(!state.muted));
        if let Err(error) = result {
            crate::platform::report_error(Some(self.top.0), &error);
        }
    }

    /// The flyout of `item`, built from freshly queried state.
    pub(super) fn flyout_menu(&mut self, item: SystemItem) -> Menu<Command> {
        let mut status = self.system.clone();
        status.audio = audio::state().ok();
        if item == SystemItem::Battery {
            status.battery = power::battery().ok().flatten();
        }
        match item {
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
                system::control_flyout(&status, &display, self.media.as_ref())
            }
        }
    }

    /// Opens the flyout of a status bar control, with the control highlighted meanwhile.
    pub(super) fn flyout(&mut self, item: SystemItem) -> Result<bool> {
        if matches!(item, SystemItem::Network | SystemItem::Control) {
            self.worker.refresh_network()?;
        }
        if item == SystemItem::Control {
            self.media = None;
            self.worker.media(None)?;
        }
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
            SystemCommand::SetOutput(id) => audio::set_default_output(&id),
            SystemCommand::SetPlan(id) => power::set_plan(id),
            // Windows broadcasts the change; the bars switch palette when it arrives.
            SystemCommand::SetDark(dark) => display::set_system_dark(dark),
            SystemCommand::SetHdr(on) => display::set_hdr(on),
            SystemCommand::SetRadio(kind, on) => self.worker.set_radio(kind, on),
            SystemCommand::Media(action) => self.worker.media(Some(action)),
        }
    }
}
