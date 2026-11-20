mod commands;
mod menus;
mod settings;
mod system;

use std::collections::BTreeSet;

use crate::dock::{AppGroup, PinnedApp, group_apps};
use crate::geometry::Rect;
use crate::platform::system_worker::SystemWorker;
use crate::platform::taskbar::{self, NativeTaskbar};
use crate::platform::tray::TrayHost;
use crate::platform::{self, AppBar, Bar, Event, Monitor, audio, shell};
use crate::render::{DockIcon, Renderer, is_device_lost};
use crate::settings::{Key, Settings};
use crate::storage;
use crate::system::Media;
use crate::ui::layout::{DockStyle, Layout, STATUS_HEIGHT, StatusHit, StatusLayout, SystemItem};
use crate::ui::menu::Anchor;
use crate::ui::system::SystemStatus;
use windows::Win32::Foundation::POINT;
use windows::core::{Error, Result};

struct Bars {
    // Destroy the tray host before removing the AppBar so Shell32 addresses Explorer.
    events: shell::WindowEvents,
    renderer: Option<Renderer>,
    /// Released after the renderer, which drops the panel's composition target first.
    panel: Option<settings::Panel>,
    tray: TrayHost,
    appbar: AppBar,
    /// Both audio subscriptions must end before the status HWND is destroyed.
    audio_watch: Option<audio::AudioWatch>,
    /// Worker notifications stop before the status window goes away.
    worker: SystemWorker,
    top: Bar,
    bottom: Bar,
    pins: Vec<PinnedApp>,
    apps: Vec<AppGroup>,
    foreground: Option<isize>,
    monitor: Monitor,
    top_rect: Rect,
    /// Both bars step aside while a full-screen application covers their monitor.
    fullscreen: bool,
    layout: Layout,
    status: StatusLayout,
    /// Indices into `tray.icons()` kept on the bar, in display order.
    shown: Vec<usize>,
    /// Keys of tray icons the user moved into the overflow menu.
    hidden: BTreeSet<String>,
    hover: Option<usize>,
    time: String,
    /// A left press on a bar dismissed the open menu; its release must not click.
    swallow_click: bool,
    settings: Settings,
    /// What the status bar's system controls show.
    system: SystemStatus,
    media: Option<Media>,
    /// The system control whose flyout is open, highlighted on the bar.
    open_flyout: Option<SystemItem>,
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
    match storage::load_pins() {
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
    let settings = settings::load();
    let pins = load_pins()?;
    let (apps, foreground) = applications(&pins)?;
    let mut native_taskbar = NativeTaskbar::default();
    let monitor = platform::monitor()?;
    let mut appbar = AppBar::new(top.0);
    let top_rect = appbar.place(monitor.bounds, status_height(monitor.dpi))?;
    if settings.enabled(Key::HideTaskbar) {
        native_taskbar.hide()?;
    }
    let monitor = platform::monitor()?;
    let tray = TrayHost::new()?;
    // The shell may have announced an already full-screen app before registration.
    // Only an application from the window snapshot qualifies (the desktop also fills the screen).
    let fullscreen = apps.iter().any(|app| app.active)
        && foreground.is_some_and(|id| platform::window_covers_monitor(id, monitor.bounds));
    let style = DockStyle {
        icon: settings.icon_size(),
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
    let renderer = Some(Renderer::new(top.0, bottom.0, settings::dark(&settings))?);
    let audio_watch = audio::AudioWatch::new(top.0).ok();
    let mut worker = SystemWorker::new(top.0)?;
    worker.refresh_network()?;
    let mut bars = Bars {
        worker,
        events,
        renderer,
        panel: None,
        appbar,
        tray,
        top,
        bottom,
        pins,
        apps,
        foreground,
        status: StatusLayout::new(top_rect.width, top_rect.height, monitor.dpi, 0, 0, &[]),
        monitor,
        top_rect,
        fullscreen,
        layout,
        shown: Vec::new(),
        hidden,
        hover: None,
        time,
        swallow_click: false,
        settings,
        system: system::status(Default::default()),
        media: None,
        audio_watch,
        open_flyout: None,
        native_taskbar,
    };
    bars.restatus();
    bars.draw()?;
    bars.position()?;
    loop {
        match platform::next_event()? {
            Event::Quit => return Ok(()),
            Event::Paint => bars.draw()?,
            Event::Refresh => {
                bars.events.acknowledge();
                bars.hide_taskbar()?;
                // Explorer raises its taskbar when activated; stay first for new icons.
                bars.tray.raise();
                if bars.tray.update() {
                    bars.restatus();
                }
                bars.refresh()?;
            }
            Event::Tray if bars.tray.update() => {
                bars.restatus();
                bars.draw()?;
            }
            Event::TrayActivation => {
                let result = bars.tray.finish_activation();
                bars.handle_tray_click(result)?;
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
                bars.fullscreen = fullscreen;
                bars.publish_tray_rects();
                bars.position()?;
                if fullscreen {
                    bars.tray.cancel_click();
                    bars.set_hover(None)?;
                }
            }
            Event::Clock => {
                let time = platform::clock(bars.top.0)?;
                if bars.time != time {
                    bars.time = time;
                    bars.draw()?;
                }
                // Network changes have no notification here; the minute tick catches them.
                bars.refresh_system()?;
            }
            Event::Audio => bars.refresh_audio()?,
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
                bars.set_hover(bars.layout.hit(x, y))?;
            }
            Event::Leave => bars.set_hover(None)?,
            Event::Scroll(delta) if delta != 0 => {
                let first = bars
                    .layout
                    .first
                    .saturating_add_signed(-isize::from(delta.signum()));
                if bars.relayout(first)? {
                    bars.draw()?;
                }
            }
            Event::Press(..) => {
                bars.tray.cancel_click();
                bars.swallow_click = false;
            }
            Event::Click(..) if bars.swallow_click => bars.swallow_click = false,
            Event::Click(owner, x, y) if owner == bars.top.0 => {
                let quit = match bars.status.hit(x, y) {
                    StatusHit::Tray(slot) => {
                        bars.tray_click(slot, false)?;
                        false
                    }
                    StatusHit::Overflow => bars.tray_menu()?,
                    StatusHit::System(item) => bars.flyout(item)?,
                    StatusHit::Clock => bars.clock_menu()?,
                    StatusHit::Title | StatusHit::Bar => bars.bar_menu()?,
                };
                if quit {
                    return Ok(());
                }
            }
            Event::Click(_, x, y) => {
                if let Some(index) = bars.layout.hit(x, y) {
                    bars.select(index)?;
                }
            }
            Event::RightClick(owner, x, y) if bars.context_menu(owner, x, y)? => return Ok(()),
            Event::Menu(_) if bars.bar_menu()? => return Ok(()),
            event @ (Event::PanelMove(..)
            | Event::PanelLeave
            | Event::PanelClick(..)
            | Event::PanelScroll(_)
            | Event::PanelKey(_)
            | Event::PanelDpi(_)
            | Event::PanelClose) => bars.panel_event(event)?,
            _ => {}
        }
    }
}

impl Bars {
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
        self.bottom.position(self.layout.bottom, !self.fullscreen)
    }

    /// Re-reads the monitor. The appbar strip is negotiated again only when the shell
    /// asks or the monitor changed, since each negotiation changes the work area again.
    fn display(&mut self, replace: bool) -> Result<()> {
        self.tray.cancel_click();
        let monitor = platform::monitor()?;
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
        self.monitor = platform::monitor()?;
        (self.apps, self.foreground) = applications(&self.pins)?;
        self.restatus();
        self.relayout(self.layout.first)?;
        self.position()?;
        self.time = platform::clock(self.top.0)?;
        // Windows announces a change of its app mode as a setting change.
        self.apply_theme()?;
        self.draw()?;
        self.relayout_panel()
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
        let moved = layout.bottom != self.layout.bottom;
        self.layout = layout;
        self.hover = None;
        if moved {
            self.bottom.position(self.layout.bottom, !self.fullscreen)?;
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
        );
        self.publish_tray_rects();
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

    fn tray_click(&mut self, slot: usize, right: bool) -> Result<()> {
        let rect = self.status.tray[slot].rect;
        let index = self.shown[slot];
        self.tray_activate(index, right, self.top_anchor(rect.x + rect.width / 2))
    }

    fn tray_activate(&mut self, index: usize, right: bool, anchor: Anchor) -> Result<()> {
        let result = self.tray.click(
            index,
            right,
            POINT {
                x: anchor.x,
                y: anchor.y,
            },
        );
        self.handle_tray_click(result)
    }

    fn handle_tray_click(&mut self, result: Result<bool>) -> Result<()> {
        match result {
            Ok(false) => Ok(()),
            Ok(true) => {
                self.restatus();
                self.draw()
            }
            Err(error) => {
                platform::report_error(Some(self.top.0), &error);
                Ok(())
            }
        }
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
            self.apps = apps;
            self.relayout(self.layout.first)?;
        }
        self.draw()
    }

    fn set_hover(&mut self, hit: Option<usize>) -> Result<()> {
        if self.hover != hit {
            self.hover = hit;
            self.draw()?;
        }
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        match self.draw_frame() {
            Err(e) if is_device_lost(&e) => {
                // Release the previous targets before binding a new device to these HWNDs.
                self.renderer.take();
                self.renderer = Some(
                    Renderer::new(self.top.0, self.bottom.0, self.dark())
                        .map_err(|e| Error::new(e.code(), format!("重建双栏图形设备失败：{e}")))?,
                );
                self.draw_frame()?;
                self.paint_panel()
            }
            result => result,
        }
    }

    fn draw_frame(&mut self) -> Result<()> {
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
                (icon.icon(), icon.stamp)
            })
            .collect::<Vec<_>>();
        let marks = self
            .status
            .system
            .iter()
            .map(|&(item, _)| self.system.mark(item))
            .collect::<Vec<_>>();
        renderer.draw_top(
            &self.status,
            dpi,
            active,
            &self.time,
            &tray,
            &marks,
            self.open_flyout,
        )?;
        let icons = self
            .layout
            .slots
            .iter()
            .map(|slot| {
                let app = &self.apps[slot.index];
                let r = slot.rect;
                DockIcon {
                    path: &app.icon_path,
                    label: &app.label,
                    center_x: r.x + r.width / 2,
                    center_y: r.y + r.height / 2,
                    size: self.layout.icon,
                    running: !app.windows.is_empty(),
                    active: app.active,
                }
            })
            .collect::<Vec<_>>();
        let hover = self
            .layout
            .slots
            .iter()
            .position(|slot| Some(slot.index) == self.hover);
        let bottom = self.layout.bottom;
        let lift = self.settings.hover_scale();
        renderer.draw_dock(bottom.width, bottom.height, dpi, &icons, hover, lift)
    }
}
