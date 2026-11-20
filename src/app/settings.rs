//! The settings panel: opening it, routing its input, and applying each change to the
//! bars, the system and the settings file.

use super::Bars;
use crate::platform::{self, Event, PanelKey, PanelWindow, display, startup};
use crate::render::is_device_lost;
use crate::settings::{Key, SCHEMA, SECTIONS, Settings, Store, Theme};
use crate::storage;
use crate::ui::layout::DockStyle;
use crate::ui::panel::{Action, Nav, PanelLayout, PanelState};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::core::{Error, Result};

pub(super) struct Panel {
    window: PanelWindow,
    state: PanelState,
    layout: PanelLayout,
    dpi: u32,
    /// Whether the sidebar is painted translucent over the acrylic backdrop.
    glass: bool,
}

/// The sidebar shows its backdrop only where DWM draws acrylic: the build supports it and
/// transparency effects are on. Otherwise the opaque sidebar looks better than the flat
/// fill DWM substitutes. An unreadable preference counts as off.
fn glass(window: &PanelWindow) -> bool {
    window.backdrop() && display::transparency().unwrap_or(false)
}

/// Settings saved by the user, falling back to defaults with a message when the file
/// cannot be read or holds invalid values. The file is rewritten only on the next change.
pub(super) fn load() -> Settings {
    let mut settings = match storage::load_settings() {
        Ok(Some((settings, problems))) => {
            if !problems.is_empty() {
                platform::report_error(None, &Error::new(E_FAIL, problems.join("\n")));
            }
            settings
        }
        Ok(None) => Settings::default(),
        Err(error) => {
            platform::report_error(None, &error);
            Settings::default()
        }
    };
    // The Run key is the source of truth, since other tools can change it.
    match startup::enabled() {
        Ok(enabled) => {
            settings.set(Key::Autostart, i32::from(enabled));
        }
        Err(error) => platform::report_error(None, &error),
    }
    settings
}

/// Whether to draw with the dark palette. An unreadable system preference counts as
/// light, the mode Windows itself assumes without one.
pub(super) fn dark(settings: &Settings) -> bool {
    match settings.theme() {
        Theme::System => display::system_dark().unwrap_or(false),
        Theme::Light => false,
        Theme::Dark => true,
    }
}

fn nav(key: PanelKey) -> Option<Nav> {
    Some(match key {
        PanelKey::Tab { back: false } | PanelKey::Down => Nav::Next,
        PanelKey::Tab { back: true } | PanelKey::Up => Nav::Previous,
        PanelKey::Left => Nav::Decrease,
        PanelKey::Right => Nav::Increase,
        PanelKey::Select => Nav::Activate,
        PanelKey::Page { back: false } => Nav::NextPage,
        PanelKey::Page { back: true } => Nav::PreviousPage,
        PanelKey::Escape => Nav::Close,
        PanelKey::Other => return None,
    })
}

impl Bars {
    pub(super) fn dock_style(&self) -> DockStyle {
        DockStyle {
            icon: self.settings.icon_size(),
            gap: self.settings.dock_gap(),
        }
    }

    pub(super) fn dark(&self) -> bool {
        dark(&self.settings)
    }

    /// Switches the palette after the theme setting or Windows' app mode changed, and the
    /// panel's sidebar after transparency effects were switched; Windows announces both
    /// as a setting change. The bars repaint on their next draw; the panel repaints here.
    pub(super) fn apply_theme(&mut self) -> Result<()> {
        let dark = self.dark();
        let Some(renderer) = self.renderer.as_mut() else {
            return Ok(());
        };
        let repalette = renderer.set_dark(dark);
        let Some(panel) = self.panel.as_mut() else {
            return Ok(());
        };
        let glass = glass(&panel.window);
        if !repalette && glass == panel.glass {
            return Ok(());
        }
        panel.glass = glass;
        if repalette {
            panel.window.set_dark(dark);
        }
        self.draw_panel()
    }

    /// Message boxes belong to the panel while it is open.
    fn settings_owner(&self) -> HWND {
        self.panel
            .as_ref()
            .map_or(self.bottom.0, |panel| panel.window.0)
    }

    /// Opens the panel centered on the primary monitor, or brings the open one forward.
    pub(super) fn open_settings(&mut self) -> Result<()> {
        if let Some(panel) = &self.panel {
            panel.window.show();
            return Ok(());
        }
        let dpi = self.monitor.dpi;
        let mut state = PanelState::default();
        let layout = PanelLayout::new(&mut state, &self.settings, dpi, self.monitor.work);
        let window = PanelWindow::create(
            layout.place(self.monitor.work),
            layout.caption_pixels(),
            layout.sidebar_pixels(),
            self.dark(),
        )?;
        self.panel = Some(Panel {
            glass: glass(&window),
            window,
            state,
            layout,
            dpi,
        });
        // Paint before showing so the window never appears empty.
        if let Err(error) = self.draw_panel() {
            self.close_settings();
            return Err(error);
        }
        if let Some(panel) = &self.panel {
            panel.window.show();
        }
        Ok(())
    }

    fn close_settings(&mut self) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.close_panel();
        }
        self.panel = None;
    }

    /// Paints the open panel without recovering from a lost device.
    pub(super) fn paint_panel(&mut self) -> Result<()> {
        let (Some(panel), Some(renderer)) = (&self.panel, self.renderer.as_mut()) else {
            return Ok(());
        };
        renderer.draw_panel(panel.window.0, &panel.layout, panel.dpi, panel.glass)
    }

    fn draw_panel(&mut self) -> Result<()> {
        match self.paint_panel() {
            // Rebuilding the device for the bars repaints the panel too.
            Err(error) if is_device_lost(&error) => self.draw(),
            result => result,
        }
    }

    /// Lays the panel out again after its state, the settings or its monitor changed,
    /// and repaints only when something visible moved.
    pub(super) fn relayout_panel(&mut self) -> Result<()> {
        let Some(panel) = self.panel.as_mut() else {
            return Ok(());
        };
        let work = panel.window.work_area()?;
        let layout = PanelLayout::new(&mut panel.state, &self.settings, panel.dpi, work);
        if layout == panel.layout {
            return Ok(());
        }
        if layout.pixels() != panel.layout.pixels() || layout.scale != panel.layout.scale {
            let (width, height) = layout.pixels();
            panel.window.resize(
                width,
                height,
                work,
                layout.caption_pixels(),
                layout.sidebar_pixels(),
            )?;
        }
        panel.layout = layout;
        self.draw_panel()
    }

    pub(super) fn panel_event(&mut self, event: Event) -> Result<()> {
        let Some(panel) = self.panel.as_mut() else {
            return Ok(());
        };
        let action = match event {
            Event::PanelMove(x, y) => {
                panel.state.hover = panel.layout.hit(x, y);
                None
            }
            Event::PanelLeave => {
                panel.state.hover = None;
                None
            }
            Event::PanelClick(x, y) => panel.layout.hit(x, y),
            Event::PanelScroll(delta) => {
                panel.state.scroll_by(delta, &panel.layout);
                None
            }
            Event::PanelKey(key) => nav(key).and_then(|nav| panel.state.key(nav, &panel.layout)),
            Event::PanelDpi(dpi) => {
                panel.dpi = dpi.max(96);
                None
            }
            Event::PanelClose => Some(Action::Close),
            _ => None,
        };
        match action {
            Some(Action::Close) => {
                self.close_settings();
                return Ok(());
            }
            Some(Action::Page(page)) => panel.state.open_page(page),
            Some(Action::Reset) => {
                let section = SECTIONS[panel.state.page];
                let mut next = self.settings.clone();
                for spec in SCHEMA.iter().filter(|spec| spec.section == section) {
                    next.set(spec.key, spec.default);
                }
                self.apply_settings(next)?;
            }
            Some(Action::Change(key, adjust)) => {
                let mut next = self.settings.clone();
                if next.adjust(key, adjust) {
                    self.apply_settings(next)?;
                }
            }
            None => {}
        }
        self.relayout_panel()
    }

    /// Makes `next` the current settings: applies every changed value, reverts those the
    /// system refused, then saves the file-backed values. Failures are reported and the
    /// remaining changes still apply.
    fn apply_settings(&mut self, next: Settings) -> Result<()> {
        let previous = std::mem::replace(&mut self.settings, next);
        let changed: Vec<_> = self.settings.changed(&previous).collect();
        let (mut relayout, mut redisplay, mut save, mut retheme) = (false, false, false, false);
        for spec in changed {
            let enabled = self.settings.enabled(spec.key);
            let result = match spec.key {
                Key::Theme => {
                    retheme = true;
                    Ok(())
                }
                Key::IconSize | Key::DockGap => {
                    relayout = true;
                    Ok(())
                }
                // The next hover animates to the new scale.
                Key::HoverZoom => Ok(()),
                Key::HideTaskbar => {
                    redisplay = true;
                    if enabled {
                        self.native_taskbar.hide()
                    } else {
                        self.native_taskbar.show()
                    }
                }
                Key::Autostart => startup::set(enabled),
                Key::Preview(_) => Ok(()),
            };
            if let Err(error) = result {
                platform::report_error(Some(self.settings_owner()), &error);
                self.settings.set(spec.key, previous.get(spec.key));
            } else if spec.store == Store::File {
                save = true;
            }
        }
        if save && let Err(error) = storage::save_settings(&self.settings) {
            // The change still applies for this session.
            platform::report_error(Some(self.settings_owner()), &error);
        }
        if retheme {
            self.apply_theme()?;
        }
        if redisplay {
            // Showing or hiding the taskbar changes the work area the Dock sits in.
            self.display(false)
        } else if relayout {
            self.relayout(self.layout.first)?;
            self.draw()
        } else if retheme {
            self.draw()
        } else {
            Ok(())
        }
    }
}
