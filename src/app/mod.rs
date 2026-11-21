mod about;
mod commands;
mod drag;
mod immersion;
mod launchpad;
mod menus;
mod monitor;
mod settings;
mod system;

use std::collections::BTreeSet;

use crate::dock::{AppGroup, PinnedApp, group_apps};
use crate::geometry::Rect;
use crate::metrics::Indicator;
use crate::platform::preview::PreviewWindow;
use crate::platform::system_worker::SystemWorker;
use crate::platform::taskbar::{self, NativeTaskbar};
use crate::platform::tray::{TrayHost, TrayInput};
use crate::platform::{self, AppBar, Bar, Event, Monitor, audio, shell};
use crate::render::{DockIcon, Renderer, TrayImage, is_device_lost};
use crate::settings::{Key, Settings};
use crate::storage;
use crate::system::Media;
use crate::ui::layout::{
    DockDrop, DockHit, DockStyle, Layout, STATUS_HEIGHT, StatusHit, StatusLayout, SystemItem,
};
use crate::ui::system::SystemStatus;
use immersion::DockExposure;
use monitor::MetricsWorker;
use windows::Win32::Foundation::POINT;
use windows::core::{Error, Result};

struct Bars {
    // Destroy the tray host before removing the AppBar so Shell32 addresses Explorer.
    events: shell::WindowEvents,
    renderer: Option<Renderer>,
    /// Released after the renderer, which drops the panel's composition target first.
    panel: Option<settings::Panel>,
    /// Released after the renderer, like the panel.
    about: Option<about::About>,
    launchpad: Option<launchpad::Launchpad>,
    tray: TrayHost,
    appbar: AppBar,
    /// Both audio subscriptions must end before the status HWND is destroyed.
    audio_watch: Option<audio::AudioWatch>,
    /// Worker notifications stop before the status window goes away.
    worker: SystemWorker,
    metrics_worker: MetricsWorker,
    indicators: Vec<Indicator>,
    /// Unregister the global shortcut before destroying its owner window.
    taskbar_shortcut: Option<platform::TaskbarShortcut>,
    top: Bar,
    bottom: Bar,
    pins: Vec<PinnedApp>,
    apps: Vec<AppGroup>,
    foreground: Option<isize>,
    monitor: Monitor,
    top_rect: Rect,
    /// Both bars step aside while a full-screen application covers their monitor.
    fullscreen: bool,
    /// What the shell last reported. It also reports the desktop as a full-screen
    /// window, so the bars only step aside while an application is in front as well.
    shell_fullscreen: bool,
    layout: Layout,
    status: StatusLayout,
    /// Indices into `tray.icons()` kept on the bar, in display order.
    shown: Vec<usize>,
    /// Keys of tray icons the user moved into the overflow menu.
    hidden: BTreeSet<String>,
    hover: Option<DockHit>,
    dock_press: Option<drag::DockPress>,
    dock_file_target: Option<DockDrop>,
    dock_exposure: DockExposure,
    preview: Option<PreviewWindow>,
    /// The status bar target under the pointer, highlighted on the bar.
    top_hover: Option<StatusHit>,
    /// Wheel movement over the sound icon not yet turned into a volume step.
    wheel: i32,
    time: String,
    tray_notice: Option<String>,
    /// A left press on a bar dismissed the open menu; its release must not click.
    swallow_click: bool,
    settings: Settings,
    /// What the status bar's system controls show.
    system: SystemStatus,
    media: Option<Media>,
    /// The system control whose flyout is open, highlighted on the bar.
    open_flyout: Option<SystemItem>,
    /// The tray overflow menu is open; its button shows a collapse arrow meanwhile.
    overflow_open: bool,
    /// Restored after the bars are gone, on exit or when the user turns hiding off.
    native_taskbar: NativeTaskbar,
}

fn applications(pins: &[PinnedApp]) -> Result<(Vec<AppGroup>, Option<isize>)> {
    let (windows, foreground) = shell::enumerate()?;
    Ok((
        group_apps(pins, &windows, foreground.unwrap_or_default()),
        foreground,
    ))
}

fn status_height(dpi: u32) -> i32 {
    (STATUS_HEIGHT * dpi as i32 + 48) / 96
}

fn load_pins() -> Result<Vec<PinnedApp>> {
    match storage::load_pins(shell::pin_from_path) {
        Ok(Some(pins)) => Ok(pins),
        Ok(None) => shell::default_pins(),
        Err(error) => {
            // Unreadable pins fall back to the defaults; the file is rewritten only when
            // the user pins or unpins again.
            platform::report_error(None, &error);
            shell::default_pins()
        }
    }
}

pub(crate) fn run() -> Result<()> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new(taskbar::RESTORE_TASKBAR))
    {
        return taskbar::restore_on_disconnect();
    }
    let Some(_instance) = taskbar::claim_session()? else {
        return Ok(());
    };
    let _desktop = platform::Desktop::init()?;
    let (top, bottom) = platform::create_bars()?;
    let events = shell::WindowEvents::subscribe(bottom.0)?;
    let hidden = storage::load_hidden().unwrap_or_else(|error| {
        // Unreadable settings leave every icon on the bar; the file is rewritten only
        // when the user changes a choice.
        platform::report_error(None, &error);
        BTreeSet::new()
    });
    let mut settings = settings::load();
    let taskbar_shortcut =
        platform::TaskbarShortcut::register(top.0, settings.get(Key::TaskbarShortcut))
            .unwrap_or_else(|error| {
                platform::report_error(Some(top.0), &error);
                settings.set(Key::TaskbarShortcut, 0);
                None
            });
    let pins = load_pins()?;
    let (apps, foreground) = applications(&pins)?;
    let mut native_taskbar = NativeTaskbar::default();
    let monitor = settings::primary_monitor(&settings)?;
    let mut appbar = AppBar::new(top.0);
    let top_rect = appbar.place(monitor.bounds, status_height(monitor.dpi))?;
    if settings.enabled(Key::HideTaskbar) {
        native_taskbar.hide()?;
    }
    let monitor = settings::primary_monitor(&settings)?;
    let tray = TrayHost::new()?;
    // The shell may have announced an already full-screen app before registration.
    // Only an application from the window snapshot qualifies (the desktop also fills the screen).
    let fullscreen = apps.iter().any(|app| app.active)
        && foreground.is_some_and(|id| platform::window_covers_monitor(id, monitor.bounds));
    let style = DockStyle {
        icon: settings.icon_size(),
        spacing: settings.get(Key::IconSpacing),
        gap: settings.dock_gap(),
    };
    let layout = Layout::new(
        native_taskbar.dock_work_area(&monitor)?,
        monitor.dpi,
        apps.len(),
        0,
        style,
    );
    let time = platform::clock(top.0)?;
    let renderer = Some(Renderer::new(
        top.0,
        bottom.0,
        settings::dark(&settings),
        &settings,
    )?);
    let audio_watch = audio::AudioWatch::new(top.0).ok();
    let mut worker = SystemWorker::new(top.0)?;
    worker.refresh_network()?;
    let mut bars = Bars {
        metrics_worker: MetricsWorker::new(top.0)?,
        indicators: Vec::new(),
        taskbar_shortcut,
        worker,
        events,
        renderer,
        panel: None,
        about: None,
        launchpad: None,
        appbar,
        tray,
        top,
        bottom,
        pins,
        apps,
        foreground,
        status: StatusLayout::new(top_rect.width, top_rect.height, monitor.dpi, 0, 0, &[], &[]),
        monitor,
        top_rect,
        fullscreen,
        shell_fullscreen: fullscreen,
        layout,
        shown: Vec::new(),
        hidden,
        hover: None,
        dock_press: None,
        dock_file_target: None,
        dock_exposure: if settings.enabled(Key::DockImmersive) {
            DockExposure::Edge
        } else {
            DockExposure::Shown
        },
        preview: None,
        top_hover: None,
        wheel: 0,
        time,
        tray_notice: None,
        swallow_click: false,
        settings,
        system: system::status(Default::default()),
        media: None,
        audio_watch,
        open_flyout: None,
        overflow_open: false,
        native_taskbar,
    };
    bars.apply_monitor_settings()?;
    bars.position()?;
    // Applications answer at once; the loop below receives their icons.
    bars.tray.request_icons()?;
    loop {
        match platform::next_event()? {
            Event::Quit => return Ok(()),
            Event::Paint => bars.draw()?,
            Event::Refresh => {
                bars.events.acknowledge();
                bars.hide_taskbar()?;
                // Explorer raises its taskbar when activated; stay first for new icons.
                bars.tray.raise();
                if bars.tray.update(true) {
                    bars.restatus();
                }
                bars.refresh()?;
                // The shell reports nothing new when focus moves between the desktop
                // and a full-screen application, so check again on every switch.
                bars.sync_fullscreen()?;
            }
            Event::Tray if bars.tray.update(false) => {
                bars.restatus();
                bars.draw()?;
            }
            Event::Display => bars.display(false)?,
            Event::AppBarMoved => bars.display(true)?,
            Event::ShellRestart if bars.appbar.shell_changed() => {
                bars.hide_taskbar()?;
                bars.display(true)?;
                // Repeat Explorer's notification with our tray host back on top.
                bars.tray.request_icons()?;
            }
            Event::FullScreen(fullscreen) => {
                bars.shell_fullscreen = fullscreen;
                // The notification can arrive before the foreground change it follows.
                bars.refresh()?;
                bars.sync_fullscreen()?;
            }
            Event::Clock => {
                let time = platform::clock(bars.top.0)?;
                if bars.settings.theme() == crate::settings::Theme::Automatic {
                    bars.update_theme_from_desktop()?;
                }
                if bars.tray_notice.take().is_some() || bars.time != time {
                    bars.time = time;
                    bars.draw()?;
                }
                // Network changes have no notification here; the minute tick catches them.
                bars.refresh_system()?;
            }
            Event::TrayNoticeExpired => bars.clear_tray_notice()?,
            Event::TaskbarShortcut(id) if id == bars.settings.get(Key::TaskbarShortcut) => {
                bars.toggle_taskbar()?;
            }
            Event::DockReveal if matches!(bars.dock_exposure, DockExposure::Waiting(_)) => {
                bars.hover_dock_edge(true)?;
            }
            Event::Audio => bars.refresh_audio()?,
            Event::MetricsUpdate => match bars.draw_metrics() {
                Err(e) if is_device_lost(&e) => bars.rebuild_renderer()?,
                result => result?,
            },
            Event::DefaultOutput => {
                bars.watch_audio();
                bars.refresh_audio()?;
            }
            Event::Power => bars.refresh_power()?,
            Event::SystemUpdate => {
                bars.system_updated(bars.top.0)?;
            }
            Event::Move(hwnd, x, y) if hwnd == bars.bottom.0 => {
                platform::track_leave(hwnd)?;
                if bars.dock_exposure.collapsed() {
                    bars.hover_dock_edge(false)?;
                } else {
                    bars.move_dock_pointer(x, y)?;
                }
            }
            Event::CancelPress(hwnd) if hwnd == bars.bottom.0 => bars.cancel_dock_press()?,
            Event::DockFileHover(point) if !bars.fullscreen => {
                if bars.dock_exposure.collapsed() && point.is_some() {
                    bars.hover_dock_edge(false)?;
                } else {
                    let target = point.and_then(|(x, y)| bars.file_target(x, y));
                    bars.set_dock_file_target(target)?;
                    if point.is_none() {
                        bars.finish_dock_gesture()?;
                    }
                }
            }
            Event::DropFiles(hwnd, x, y, files)
                if hwnd == bars.bottom.0 && !bars.fullscreen && !bars.dock_exposure.collapsed() =>
            {
                let target = bars.file_target(x, y);
                bars.set_dock_file_target(None)?;
                if let Some(target) = target {
                    bars.import_files(hwnd, target, files)?;
                }
            }
            Event::Move(hwnd, x, y) if hwnd == bars.top.0 => {
                platform::track_leave(hwnd)?;
                bars.set_top_hover(Some(bars.status.hit(x, y)))?;
            }
            Event::Leave(hwnd) if hwnd == bars.bottom.0 => {
                bars.leave_dock()?;
                bars.hide_idle_dock()?;
            }
            Event::Leave(hwnd) if hwnd == bars.top.0 => bars.set_top_hover(None)?,
            Event::Scroll(owner, delta, x, y) if owner == bars.top.0 => {
                bars.scroll_status(delta, x, y)?;
            }
            Event::Scroll(_, delta, _, _) if delta != 0 && !bars.dock_exposure.collapsed() => {
                let first = bars
                    .layout
                    .first
                    .saturating_add_signed(-isize::from(delta.signum()));
                if bars.relayout(first)? {
                    bars.draw()?;
                }
            }
            Event::MiddleClick(owner, x, y)
                if owner != bars.bottom.0 || !bars.dock_exposure.collapsed() =>
            {
                bars.middle_click(owner, x, y)?
            }
            Event::Press {
                owner,
                x,
                y,
                right,
                double,
            } => {
                bars.swallow_click = false;
                bars.cancel_dock_press()?;
                if owner == bars.bottom.0 {
                    bars.set_hover(None)?;
                    if !right && !bars.fullscreen && !bars.dock_exposure.collapsed() {
                        bars.press_dock(x, y);
                    }
                }
                if owner == bars.top.0
                    && let StatusHit::Tray(slot) = bars.status.hit(x, y)
                {
                    bars.tray_pointer(slot, TrayInput::Press { right, double }, x, y)?;
                } else {
                    bars.tray.cancel_click();
                }
            }
            Event::Click(..) if bars.swallow_click => bars.swallow_click = false,
            Event::Click(owner, x, y) if owner == bars.top.0 => {
                let quit = match bars.status.hit(x, y) {
                    StatusHit::Tray(slot) => {
                        bars.tray_pointer(slot, TrayInput::Release { right: false }, x, y)?;
                        false
                    }
                    StatusHit::Overflow => bars.tray_menu()?,
                    StatusHit::System(item) => bars.flyout(item)?,
                    StatusHit::Clock => bars.clock_menu()?,
                    StatusHit::Mark => bars.mark_menu()?,
                    StatusHit::Title | StatusHit::Bar => bars.bar_menu()?,
                };
                if quit {
                    return Ok(());
                }
            }
            Event::Click(owner, x, y) if owner == bars.bottom.0 => bars.release_dock(x, y)?,
            Event::RightClick(owner, x, y)
                if (owner != bars.bottom.0 || !bars.dock_exposure.collapsed())
                    && bars.context_menu(owner, x, y)? =>
            {
                return Ok(());
            }
            Event::Menu(_) if bars.bar_menu()? => return Ok(()),
            Event::Launchpad(input) => bars.launchpad_event(input)?,
            Event::About(input) => bars.about_event(input)?,
            event @ (Event::PanelPress(..)
            | Event::PanelCancel
            | Event::PanelMove(..)
            | Event::PanelLeave
            | Event::PanelClick(..)
            | Event::PanelScroll(_)
            | Event::PanelKey(_)
            | Event::PanelDpi
            | Event::PanelClose) => bars.panel_event(event)?,
            _ => {}
        }
    }
}

impl Bars {
    /// Whether the foreground window is an application covering the bars' monitor.
    /// The desktop is not in the window snapshot, so it never qualifies.
    fn fullscreen_app(&self) -> bool {
        self.apps.iter().any(|app| app.active)
            && self
                .foreground
                .is_some_and(|id| platform::window_covers_monitor(id, self.monitor.bounds))
    }

    /// Hides both bars while the shell reports a full-screen window and that window is
    /// an application, and shows them again otherwise.
    fn sync_fullscreen(&mut self) -> Result<()> {
        let fullscreen = self.shell_fullscreen && self.fullscreen_app();
        if fullscreen == self.fullscreen {
            return Ok(());
        }
        self.fullscreen = fullscreen;
        self.sync_sampling();
        self.publish_tray_rects();
        if !fullscreen {
            self.draw()?;
        }
        self.position()?;
        if fullscreen {
            self.cancel_dock_press()?;
            self.hide_idle_dock()?;
            self.set_dock_file_target(None)?;
            self.tray.cancel_click();
            self.set_hover(None)?;
            self.set_top_hover(None)?;
        }
        Ok(())
    }

    /// Hides Explorer's taskbar again, unless the user keeps it.
    fn hide_taskbar(&mut self) -> Result<()> {
        if self.settings.enabled(Key::HideTaskbar) {
            self.native_taskbar.hide()?;
        }
        Ok(())
    }

    fn position(&self) -> Result<()> {
        self.tray.position(
            self.settings
                .enabled(Key::HideTaskbar)
                .then_some(self.top_rect),
        )?;
        self.top.position(self.top_rect, !self.fullscreen)?;
        self.position_dock()
    }

    /// Re-reads the monitor. The appbar strip is negotiated again only when the shell
    /// asks or the monitor changed, since each negotiation changes the work area again.
    fn display(&mut self, replace: bool) -> Result<()> {
        self.tray.cancel_click();
        let monitor = settings::primary_monitor(&self.settings)?;
        let replace =
            replace || monitor.bounds != self.monitor.bounds || monitor.dpi != self.monitor.dpi;
        if replace {
            self.top_rect = self.tray.with_explorer_taskbar(|| {
                self.appbar
                    .place(monitor.bounds, status_height(monitor.dpi))
            })?;
            // Only undo a bottom reservation that appeared during this negotiation.
            // A previously occupied strip can belong to another desktop utility.
            if monitor.work.y + monitor.work.height == monitor.bounds.y + monitor.bounds.height {
                self.native_taskbar.refresh_work_area()?;
            }
        }
        self.monitor = settings::primary_monitor(&self.settings)?;
        (self.apps, self.foreground) = applications(&self.pins)?;
        self.restatus();
        if !self.relayout(self.layout.first)? {
            self.update_preview();
        }
        self.position()?;
        self.time = platform::clock(self.top.0)?;
        // Windows announces a change of its app mode as a setting change.
        self.update_theme_from_desktop()?;
        self.draw()?;
        self.relayout_panel()?;
        self.resize_launchpad()
    }

    fn relayout(&mut self, first: usize) -> Result<bool> {
        let layout = Layout::new(
            self.native_taskbar.dock_work_area(&self.monitor)?,
            self.monitor.dpi,
            self.apps.len(),
            first,
            self.dock_style(),
        );
        if self.layout == layout {
            return Ok(false);
        }
        let moved = layout.bottom != self.layout.bottom
            || (self.settings.enabled(Key::DockImmersive)
                && layout.immersive_window(true) != self.layout.immersive_window(true));
        self.layout = layout;
        if moved {
            // A press started against the previous client origin.
            if self.dock_press.take().is_some() {
                self.bottom.release();
            }
            self.position_dock()?;
        }
        if self.hover.is_some() {
            self.hover = platform::cursor().ok().and_then(|point| {
                self.layout.hit(
                    point.x - self.layout.bottom.x,
                    point.y - self.layout.bottom.y,
                )
            });
            self.update_preview();
        }
        Ok(true)
    }

    /// Recomputes which tray icons sit on the status bar.
    fn restatus(&mut self) {
        let icons = self.tray.icons();
        let visible = icons.iter().filter(|icon| icon.visible()).count();
        self.shown = (0..icons.len())
            .filter(|&i| icons[i].visible() && !self.hidden.contains(&icons[i].key))
            .collect();
        self.status = StatusLayout::new(
            self.top_rect.width,
            self.top_rect.height,
            self.monitor.dpi,
            self.shown.len(),
            visible,
            &self.system.items(),
            &self.indicators,
        );
        self.publish_tray_rects();
        self.sync_sampling();
    }

    fn publish_tray_rects(&self) {
        if self.fullscreen || !self.settings.enabled(Key::HideTaskbar) {
            self.tray.set_icon_rects(std::iter::empty());
            return;
        }
        // Each visible icon is reachable at its own slot or at the overflow button.
        let on_bar = &self.shown[..self.status.tray.len()];
        let rects = self
            .tray
            .icons()
            .iter()
            .enumerate()
            .filter_map(|(index, icon)| {
                if !icon.visible() {
                    return None;
                }
                let rect = on_bar
                    .binary_search(&index)
                    .ok()
                    .map(|slot| self.status.tray[slot].rect)
                    .or(self.status.overflow)?;
                Some((
                    index,
                    Rect {
                        x: self.top_rect.x + rect.x,
                        y: self.top_rect.y + rect.y,
                        ..rect
                    },
                ))
            });
        self.tray.set_icon_rects(rects);
    }

    fn tray_pointer(&mut self, slot: usize, input: TrayInput, x: i32, y: i32) -> Result<()> {
        let index = self.shown[slot];
        if matches!(
            input,
            TrayInput::Press { right: true, .. } | TrayInput::Release { right: true }
        ) {
            self.tray_anchored_input(index, input)
        } else {
            self.tray_input(
                index,
                input,
                POINT {
                    x: self.top_rect.x + x,
                    y: self.top_rect.y + y,
                },
            )
        }
    }

    fn tray_anchored_input(&mut self, index: usize, input: TrayInput) -> Result<()> {
        let anchor = self.tray_anchor(index)?;
        self.tray_input(
            index,
            input,
            POINT {
                x: anchor.x,
                y: anchor.y,
            },
        )
    }

    fn tray_input(&mut self, index: usize, input: TrayInput, at: POINT) -> Result<()> {
        let result = self.tray.click(index, input, at);
        match result {
            Ok(false) => Ok(()),
            Ok(true) => {
                self.restatus();
                self.draw()
            }
            Err(error) => {
                let name = &self.tray.icons()[index].name;
                self.tray_notice = Some(
                    if error.code() == windows::Win32::Foundation::E_ACCESSDENIED {
                        format!("权限受限，请从系统托盘操作“{name}”")
                    } else {
                        format!("托盘点击投递失败（{}） · {name}", error.code())
                    },
                );
                platform::time_tray_notice(self.top.0);
                self.draw()
            }
        }
    }

    fn clear_tray_notice(&mut self) -> Result<()> {
        if self.tray_notice.take().is_some() {
            self.draw()?;
        }
        Ok(())
    }

    fn tray_toggle(&mut self, index: usize) -> Result<()> {
        let key = self.tray.icons()[index].key.clone();
        if !self.hidden.remove(&key) {
            self.hidden.insert(key);
        }
        if let Err(error) = storage::save_hidden(&self.hidden) {
            // The choice still applies for this session.
            platform::report_error(Some(self.top.0), &error);
        }
        self.restatus();
        self.draw()
    }

    fn refresh(&mut self) -> Result<()> {
        let (apps, foreground) = applications(&self.pins)?;
        self.foreground = foreground;
        if self.apps != apps {
            let sources_changed = if let Some(DockHit::App(i)) = self.hover {
                apps.get(i).is_none_or(|new| {
                    let old = &self.apps[i];
                    new.app_id != old.app_id
                        || new
                            .windows
                            .iter()
                            .map(|w| w.id)
                            .ne(old.windows.iter().map(|w| w.id))
                })
            } else {
                false
            };
            self.apps = apps;
            if !self.relayout(self.layout.first)? && sources_changed {
                self.update_preview();
            }
        }
        self.draw()
    }

    fn set_hover(&mut self, hit: Option<DockHit>) -> Result<()> {
        let hit = hit.filter(|_| !self.fullscreen);
        if self.hover != hit {
            self.hover = hit;
            self.update_preview();
            self.draw()?;
        }
        Ok(())
    }

    fn update_preview(&mut self) {
        self.preview = None;
        let Some(DockHit::App(index)) = self.hover else {
            return;
        };
        let windows = &self.apps[index].windows;
        if windows.is_empty() {
            return;
        }
        let anchor = self.icon_anchor(index);
        match PreviewWindow::show(
            windows,
            anchor.x,
            anchor.y,
            self.monitor.work,
            self.monitor.dpi,
        ) {
            Ok(preview) => self.preview = preview,
            Err(error) => {
                self.tray_notice = Some(format!("窗口预览不可用（{}）", error.code()));
                platform::time_tray_notice(self.top.0);
            }
        }
    }

    /// Highlights the status bar target under the pointer. The title and empty space
    /// have no highlight, so moving across them does not repaint.
    fn set_top_hover(&mut self, hit: Option<StatusHit>) -> Result<()> {
        let hit = hit.filter(|&hit| self.status.target(hit).is_some());
        if self.top_hover != hit {
            self.top_hover = hit;
            self.draw()?;
        }
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        match self.draw_frame() {
            Err(e) if is_device_lost(&e) => self.rebuild_renderer(),
            result => result,
        }
    }

    fn rebuild_renderer(&mut self) -> Result<()> {
        // Release composition targets before binding a new device to these HWNDs.
        self.renderer.take();
        self.renderer = Some(
            Renderer::new(self.top.0, self.bottom.0, self.dark(), &self.settings)
                .map_err(|e| Error::new(e.code(), format!("重建双栏图形设备失败：{e}")))?,
        );
        self.draw_frame()?;
        self.paint_panel()?;
        self.paint_about()?;
        self.paint_launchpad()
    }

    fn draw_frame(&mut self) -> Result<()> {
        let (preview_order, dock_hover) = self.dock_preview();
        let dropping = self.dropping_on_launchpad();
        let renderer = self.renderer.as_mut().expect("renderer bound to bars");
        renderer.check_device()?;
        let dpi = self.monitor.dpi;
        let active = self
            .apps
            .iter()
            .find(|app| app.active)
            .map_or("桌面", |app| app.label.as_str());
        let icons = self.tray.icons();
        let tray = self
            .status
            .tray
            .iter()
            .map(|slot| {
                let icon = &icons[self.shown[slot.index]];
                TrayImage {
                    icon: icon.icon(),
                    stamp: icon.stamp,
                    needs_attention: icon.needs_attention(),
                }
            })
            .collect::<Vec<_>>();
        let title = self.tray_notice.as_deref().unwrap_or_else(|| {
            if let Some(StatusHit::Tray(slot)) = self.top_hover
                && let Some(&index) = self.shown.get(slot)
                && !icons[index].can_interact()
            {
                "仅显示，请从系统托盘操作"
            } else {
                active
            }
        });
        let marks = self
            .status
            .system
            .iter()
            .map(|&(item, _)| self.system.mark(item))
            .collect::<Vec<_>>();
        renderer.draw_top(
            &self.status,
            dpi,
            title,
            &self.time,
            &tray,
            &marks,
            self.open_flyout
                .map(StatusHit::System)
                .or(self.overflow_open.then_some(StatusHit::Overflow)),
            self.top_hover,
            &self.indicators,
        )?;
        let insertion = match self.dock_file_target {
            Some(DockDrop::Insert(at)) => Some(self.layout.insertion_preview(at)),
            _ => None,
        };
        let (slots, scale) = insertion
            .as_ref()
            .map_or((&self.layout.slots, 1.), |(slots, scale)| (slots, *scale));
        let icons = slots
            .iter()
            .map(|slot| {
                let (path, label, running, active) = match slot.target {
                    DockHit::Launchpad => (None, "启动台", false, false),
                    DockHit::App(index) => {
                        let index = preview_order.as_ref().map_or(index, |order| order[index]);
                        let app = &self.apps[index];
                        (
                            Some(app.icon_path.as_path()),
                            app.label.as_str(),
                            !app.windows.is_empty(),
                            app.active,
                        )
                    }
                };
                let r = slot.rect;
                DockIcon {
                    path,
                    label,
                    center_x: r.x + r.width / 2,
                    center_y: r.y + r.height / 2,
                    size: self.layout.icon,
                    running,
                    active,
                }
            })
            .collect::<Vec<_>>();
        let hover = slots
            .iter()
            .position(|slot| Some(slot.target) == dock_hover);
        let deep = slots
            .iter()
            .position(|slot| slot.target == DockHit::Launchpad)
            .filter(|_| dropping);
        let bottom = self.layout.bottom;
        let lift = self.settings.hover_scale();
        renderer.draw_dock(
            bottom.width,
            bottom.height,
            dpi,
            &icons,
            hover,
            deep,
            lift,
            scale,
        )?;
        renderer.deepen_launchpad(dropping)
    }
}
