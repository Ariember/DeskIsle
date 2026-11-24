//! The settings panel: opening it, routing its input, and applying each change to the
//! bars, the system and the settings file.

use super::Bars;
use crate::platform::{self, Event, PanelKey, PanelWindow, display, startup};
use crate::render::is_device_lost;
use crate::settings::{
    Adjust, AppearanceKey as A, Key, SCHEMA, SECTIONS, Settings, Store, Theme, TopBarBackground,
};
use crate::storage;
use crate::ui::layout::DockStyle;
use crate::ui::panel::{Action, Nav, PanelLayout, PanelState, panel_dpi};
use crate::visibility::DockDisplayMode;
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::core::{Error, Result};

pub(super) struct Panel {
    window: PanelWindow,
    state: PanelState,
    layout: PanelLayout,
    dpi: u32,
    /// Whether the sidebar is painted translucent over the acrylic backdrop.
    glass: bool,
    /// Another window may have changed the scene while it had focus.
    was_inactive: bool,
    pressed: Option<Action>,
}

/// The sidebar shows its backdrop when transparency effects are on and either DeskIsle's
/// live Liquid Glass compositor or DWM acrylic is available.
fn glass(window: &PanelWindow, live_glass: bool) -> bool {
    display::transparency().unwrap_or(false) && (live_glass || window.backdrop())
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
    if settings.theme() == Theme::Automatic
        && settings.enabled(Key::SyncSystemTheme)
        && let Err(error) = display::set_system_dark(dark(&settings))
    {
        platform::report_error(None, &error);
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
        Theme::Automatic => !(6..18).contains(&platform::local_hour()),
    }
}

pub(super) fn primary_monitor(settings: &Settings) -> Result<platform::Monitor> {
    let mut monitor = platform::monitor()?;
    monitor.dpi = settings.ui_dpi(monitor.dpi);
    Ok(monitor)
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
            spacing: self.settings.get(Key::IconSpacing),
        }
    }

    pub(super) fn dark(&self) -> bool {
        dark(&self.settings)
    }

    pub(super) fn sync_bar_backdrops(&mut self) {
        let level = |key| self.settings.get(Key::Appearance(key));
        let custom = level(A::TopCustom) != 0;
        self.wallpaper_tint = if custom && level(A::TopTint) > 0 {
            platform::backdrop::wallpaper_tint(self.monitor.bounds).ok()
        } else {
            None
        };

        let transparency = display::transparency().unwrap_or(false);
        let mut live_glass = false;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_transparency(transparency);
            live_glass = renderer.has_live_glass();
        }

        let frosted = if custom {
            level(A::TopOpacity) < 100
        } else {
            self.settings.top_bar_background() == TopBarBackground::Frosted
        };
        if live_glass {
            // Live Liquid Glass runs via Windows.UI.Composition underneath DComp; keep
            // legacy DWM system backdrops off so inactive windows never gray out or bleed.
            self.dock_glass_requested = None;
            self.sync_dock_backdrop();
            if self.top_glass_theme.take().is_some() {
                self.top.set_top_backdrop(false, false);
            }
            self.top_glass = transparency && frosted;
            return;
        }

        let dock_frosted = level(A::DockCustom) == 0 || level(A::DockOpacity) < 100;
        let blur = self.settings.get(Key::Backdrop) > 0;
        let glass = (frosted || dock_frosted) && transparency && blur;
        let dark = glass.then(|| self.dark());
        let theme = dark.filter(|_| frosted);
        self.dock_glass_requested = dark.filter(|_| dock_frosted);
        self.sync_dock_backdrop();
        if theme != self.top_glass_theme {
            self.top_glass = self
                .top
                .set_top_backdrop(theme.is_some(), theme.unwrap_or(false));
            self.top_glass_theme = theme;
        }
        if theme.is_none() {
            // At zero blur a translucent bar can show the desktop directly.
            self.top_glass = frosted && transparency && !blur;
        }
    }

    pub(super) fn update_theme_from_desktop(&mut self) -> Result<()> {
        if self.settings.theme() == Theme::Automatic
            && self.settings.enabled(Key::SyncSystemTheme)
            && self
                .renderer
                .as_ref()
                .is_some_and(|r| r.is_dark() != self.dark())
            && let Err(error) = display::set_system_dark(self.dark())
        {
            platform::report_error(Some(self.settings_owner()), &error);
        }
        self.apply_theme()
    }

    /// Switches the palette after the theme setting or Windows' app mode changed, and the
    /// panel's sidebar after transparency effects were switched; Windows announces both
    /// as a setting change. The bars repaint on their next draw; the panel repaints here.
    pub(super) fn apply_theme(&mut self) -> Result<()> {
        self.sync_bar_backdrops();
        let dark = self.dark();
        let Some(renderer) = self.renderer.as_mut() else {
            return Ok(());
        };
        let live_glass = renderer.has_live_glass();
        let repalette = renderer.set_appearance(dark, &self.settings);
        self.retheme_about(dark, repalette)?;
        if repalette {
            self.label = None;
            self.retheme_preview(dark)?;
            self.refresh_desktop_cards()?;
            self.draw_launchpad()?;
        }
        let Some(panel) = self.panel.as_mut() else {
            return Ok(());
        };
        let glass = glass(&panel.window, live_glass);
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
    pub(super) fn settings_owner(&self) -> HWND {
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
        let dpi = panel_dpi(self.monitor.dpi, self.monitor.work);
        let mut state = PanelState::default();
        let layout = PanelLayout::new(&mut state, &self.settings, dpi, self.monitor.work);
        let live_glass = self
            .renderer
            .as_ref()
            .is_some_and(crate::render::Renderer::has_live_glass);
        let window = PanelWindow::create(
            layout.place(self.monitor.work),
            layout.caption_pixels(),
            layout.sidebar_pixels(),
            self.dark(),
            !live_glass,
        )?;
        self.panel = Some(Panel {
            glass: glass(&window, live_glass),
            was_inactive: false,
            window,
            pressed: None,
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
            Err(error) if is_device_lost(&error) => self.rebuild_renderer(),
            result => result,
        }
    }

    /// Lays the panel out again after its state, the settings or its monitor changed,
    /// and repaints only when something visible moved.
    pub(super) fn relayout_panel(&mut self) -> Result<()> {
        let update_status = self.update_status_label();
        let Some(panel) = self.panel.as_mut() else {
            return Ok(());
        };
        let work = panel.window.work_area()?;
        panel.dpi = panel_dpi(
            self.settings.ui_dpi(platform::window_dpi(panel.window.0)),
            work,
        );
        let mut layout = PanelLayout::with_market(
            &mut panel.state,
            &self.settings,
            panel.dpi,
            work,
            &self.market.view,
        );
        if let (Some(about), Some(label)) = (&mut layout.about, update_status) {
            about.check_label = label;
        }
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

    /// Returns true when the user chooses to exit DeskIsle.
    pub(super) fn panel_event(&mut self, event: &Event) -> Result<bool> {
        let Some(panel) = self.panel.as_mut() else {
            return Ok(false);
        };
        let action = match *event {
            Event::PanelActivation(active) => {
                let reactivated = active && panel.was_inactive;
                panel.was_inactive = !active;
                if reactivated && panel.glass {
                    if let Some(renderer) = &mut self.renderer {
                        renderer.invalidate_panel_scene();
                    }
                    self.draw_panel()?;
                }
                return Ok(false);
            }
            Event::PanelMoved => {
                if panel.glass {
                    self.draw_panel()?;
                }
                return Ok(false);
            }
            Event::PanelPress(x, y) => {
                panel.pressed = panel.layout.hit(x, y);
                if let Some(key) = panel.layout.slider_at(x, y)
                    && let Some(Action::Change(_, Adjust::Set(value))) =
                        panel.layout.slider_action(key, x)
                {
                    panel.window.capture();
                    panel.state.slider_preview = Some((key, value));
                }
                None
            }
            Event::PanelCancel => {
                panel.state.slider_preview = None;
                panel.state.mode_highlight = None;
                panel.pressed = None;
                None
            }
            Event::PanelMove(x, y) => {
                if let Some((key, _)) = panel.state.slider_preview {
                    if let Some(Action::Change(_, Adjust::Set(value))) =
                        panel.layout.slider_action(key, x)
                    {
                        panel.state.slider_preview = Some((key, value));
                    }
                } else {
                    panel.state.hover = panel.layout.hit(x, y);
                    if panel.state.mode_highlight.is_some()
                        && let Some(Action::Change(Key::DockDisplayMode, Adjust::Pick(i))) =
                            panel.state.hover
                    {
                        panel.state.mode_highlight = Some(i);
                    }
                }
                None
            }
            Event::PanelLeave => {
                panel.state.hover = None;
                None
            }
            Event::PanelClick(x, y) => {
                let pressed = panel.pressed.take();
                if let Some((key, _)) = panel.state.slider_preview.take() {
                    panel.window.release();
                    panel.layout.slider_action(key, x)
                } else {
                    pressed.filter(|action| Some(*action) == panel.layout.hit(x, y))
                }
            }
            Event::PanelKey(PanelKey::Escape) if panel.state.slider_preview.is_some() => {
                panel.state.slider_preview = None;
                panel.pressed = None;
                panel.window.release();
                None
            }
            Event::PanelScroll(_) | Event::PanelKey(_) if panel.state.slider_preview.is_some() => {
                None
            }
            Event::PanelScroll(delta) if panel.state.mode_highlight.is_some() => {
                if delta != 0 {
                    let direction = if delta > 0 { Nav::Previous } else { Nav::Next };
                    panel.state.key(direction, &panel.layout);
                }
                None
            }
            Event::PanelScroll(delta) => {
                panel.state.scroll_by(delta, &panel.layout);
                None
            }
            Event::PanelKey(key) => nav(key).and_then(|nav| panel.state.key(nav, &panel.layout)),
            Event::PanelDpi => {
                panel.state.slider_preview = None;
                panel.state.mode_highlight = None;
                panel.pressed = None;
                panel.window.release();
                None
            }
            Event::PanelClose => Some(Action::Close),
            _ => None,
        };
        if action.is_some() && panel.state.mode_highlight.take().is_some() {
            panel.window.release();
            panel.state.hover = None;
        }
        match action {
            Some(Action::OpenModes) => {
                panel.state.focus = panel
                    .layout
                    .rows
                    .iter()
                    .position(|row| row.key == Key::DockDisplayMode);
                panel.state.mode_highlight = Some(self.settings.get(Key::DockDisplayMode) as usize);
                panel.window.capture();
            }
            Some(Action::DismissModes) => {}
            Some(Action::Close) => {
                self.close_settings();
                return Ok(false);
            }
            Some(Action::Quit) => return Ok(true),
            Some(Action::CheckUpdates) => self.check_for_updates(true),
            Some(action @ (Action::BrowseMarket | Action::InstallPlugin(_))) => {
                panel.pressed = None;
                self.start_market(action)
            }
            Some(Action::OpenPluginsDirectory) => {
                let owner = panel.window.0;
                if let Err(error) = storage::ensure_plugins_directory()
                    .and_then(|path| platform::shell::execute(owner, path))
                {
                    platform::report_error(Some(owner), &error);
                }
            }
            Some(Action::Open(uri)) => {
                let owner = panel.window.0;
                if let Err(error) = platform::shell::execute(owner, uri) {
                    platform::report_error(Some(owner), &error);
                }
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
        self.relayout_panel()?;
        Ok(false)
    }

    pub(super) fn toggle_taskbar(&mut self) -> Result<()> {
        let mut next = self.settings.clone();
        next.set(
            Key::HideTaskbar,
            i32::from(!self.settings.enabled(Key::HideTaskbar)),
        );
        self.apply_settings(next)
    }

    /// Makes `next` the current settings: applies every changed value, reverts those the
    /// system refused, then saves the file-backed values. Failures are reported and the
    /// remaining changes still apply.
    fn apply_settings(&mut self, next: Settings) -> Result<()> {
        let previous = std::mem::replace(&mut self.settings, next);
        let changed: Vec<_> = self.settings.changed(&previous).collect();
        let (mut relayout, mut redisplay, mut save, mut retheme) = (false, false, false, false);
        let mut monitor_changed = false;
        let mut dock_visibility_changed = false;
        let mut top_height_changed = false;
        let mut status_changed = false;
        let mut window_style_changed = false;
        for spec in changed {
            let enabled = self.settings.enabled(spec.key);
            let result = match spec.key {
                Key::InterfaceScale => {
                    redisplay = true;
                    Ok(())
                }
                Key::TopBarHeight => {
                    top_height_changed = true;
                    Ok(())
                }
                Key::Appearance(_)
                | Key::Backdrop
                | Key::TopBarBackground
                | Key::ColorTrayIcons => {
                    retheme = true;
                    Ok(())
                }
                Key::TopBarImmersive | Key::TopBarCloseMaximized => {
                    window_style_changed = true;
                    Ok(())
                }
                Key::ShowInputLanguageIcon => {
                    status_changed = true;
                    self.sync_input_indicator()
                }
                Key::ShowInputMethodIcon | Key::ShowSearchIcon => {
                    status_changed = true;
                    Ok(())
                }
                Key::LockTrayIcons => {
                    self.cancel_tray_drag();
                    Ok(())
                }
                Key::ConfirmPowerActions | Key::AutoCheckUpdates => Ok(()),
                Key::DesktopCards => self.refresh_desktop_cards(),
                Key::Theme | Key::SyncSystemTheme => {
                    retheme = true;
                    if self.settings.enabled(Key::SyncSystemTheme)
                        && self.settings.theme() != Theme::System
                    {
                        display::set_system_dark(self.dark())
                    } else {
                        Ok(())
                    }
                }
                Key::Accent | Key::Language | Key::SmoothText => {
                    retheme = true;
                    Ok(())
                }
                // The status bar sizes its clock and readings to the text.
                Key::FontFamily | Key::FontScale | Key::BoldFont => {
                    retheme = true;
                    status_changed = true;
                    Ok(())
                }
                Key::IconSize | Key::DockGap | Key::IconSpacing => {
                    relayout = true;
                    Ok(())
                }
                // The next hover animates to the new scale.
                Key::HoverZoom => Ok(()),
                Key::DockDisplayMode => {
                    let binding = if self.settings.dock_display_mode() == DockDisplayMode::Hotkey {
                        platform::Shortcut::dock(self.top.0).map(Some)
                    } else {
                        Ok(None)
                    };
                    binding.map(|binding| {
                        self.dock_shortcut = binding;
                        self.dock_hotkey_shown = false;
                        dock_visibility_changed = true;
                    })
                }
                Key::DockRevealDelay => {
                    dock_visibility_changed = true;
                    Ok(())
                }
                Key::HideTaskbar => {
                    redisplay = true;
                    if enabled {
                        self.native_taskbar.hide()
                    } else {
                        self.native_taskbar.show()
                    }
                }
                Key::Autostart => startup::set(enabled),
                Key::TaskbarShortcut => {
                    platform::Shortcut::taskbar(self.top.0, self.settings.get(spec.key))
                        .map(|registered| self.taskbar_shortcut = registered)
                }
                Key::Clock(_) | Key::RecentFiles => Ok(()),
                Key::Monitor(_) | Key::TemperatureUnit => {
                    monitor_changed = true;
                    Ok(())
                }
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
        if monitor_changed {
            self.apply_monitor_settings()?;
        }
        if retheme {
            self.apply_theme()?;
        }
        if self.settings.get(Key::InterfaceScale) != previous.get(Key::InterfaceScale) {
            self.refresh_about_appearance()?;
        }
        if dock_visibility_changed {
            self.sync_dock_visibility(true)?;
            if matches!(self.dock_exposure, super::DockExposure::Waiting(_)) {
                self.hide_idle_dock()?;
                self.hover_dock_edge(false)?;
            }
        }
        if window_style_changed {
            self.refresh_top_window(true);
        }
        if status_changed {
            self.restatus();
        }
        if redisplay || top_height_changed {
            // Showing or hiding the taskbar changes the work area the Dock sits in.
            self.display(top_height_changed)
        } else if relayout {
            self.relayout(self.layout.first)?;
            self.draw()?;
            self.resize_launchpad()
        } else if retheme || status_changed || window_style_changed {
            self.draw()
        } else {
            Ok(())
        }
    }
}
