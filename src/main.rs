#![windows_subsystem = "windows"]

mod dock;
mod platform;
mod render;

use std::collections::BTreeSet;

use dock::{
    Action, AppGroup, Layout, PinnedApp, Rect, STATUS_HEIGHT, StatusHit, StatusLayout, group_apps,
};
use platform::tray::{self, TrayChoice, TrayHost};
use platform::{AppBar, Bar, Event, Monitor, shell};
use render::{DockIcon, Renderer};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
use windows::core::{Error, Result};

struct Bars {
    // Unsubscribe first, then release composition targets, the appbar reservation and
    // the tray host before the bar HWNDs.
    events: shell::WindowEvents,
    renderer: Option<Renderer>,
    appbar: AppBar,
    tray: TrayHost,
    top: Bar,
    bottom: Bar,
    pins: Vec<PinnedApp>,
    apps: Vec<AppGroup>,
    monitor: Monitor,
    top_rect: Rect,
    /// A full-screen application covers the monitor; the status bar steps aside.
    fullscreen: bool,
    layout: Layout,
    status: StatusLayout,
    /// Indices into `tray.icons()` kept on the bar, in display order.
    shown: Vec<usize>,
    /// Keys of tray icons the user moved into the overflow menu.
    hidden: BTreeSet<String>,
    hover: Option<usize>,
    time: String,
}

fn applications(pins: &[PinnedApp]) -> Result<Vec<AppGroup>> {
    let (windows, foreground) = shell::enumerate()?;
    Ok(group_apps(pins, &windows, foreground.unwrap_or_default()))
}

fn status_height(dpi: u32) -> i32 {
    (STATUS_HEIGHT * dpi as i32 + 48) / 96
}

fn run() -> Result<()> {
    let _desktop = platform::Desktop::init()?;
    let (top, bottom) = platform::create_bars()?;
    let events = shell::WindowEvents::subscribe(bottom.0)?;
    let appbar = AppBar::new(top.0);
    let tray = TrayHost::new()?;
    let hidden = tray::load_hidden().unwrap_or_else(|error| {
        // Unreadable settings leave every icon on the bar; the file is rewritten only
        // when the user changes a choice.
        platform::report_error(None, &error);
        BTreeSet::new()
    });
    let pins = shell::pins()?;
    let apps = applications(&pins)?;
    let monitor = platform::monitor()?;
    let top_rect = appbar.place(monitor.bounds, status_height(monitor.dpi));
    let layout = Layout::new(monitor.work, monitor.dpi, apps.len(), 0);
    let time = platform::clock(top.0)?;
    let renderer = Some(Renderer::new(top.0, bottom.0)?);
    let mut bars = Bars {
        events,
        renderer,
        appbar,
        tray,
        top,
        bottom,
        pins,
        apps,
        status: StatusLayout::new(top_rect.width, top_rect.height, monitor.dpi, 0, 0),
        monitor,
        top_rect,
        fullscreen: false,
        layout,
        shown: Vec::new(),
        hidden,
        hover: None,
        time,
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
            Event::Display => bars.display(false)?,
            Event::AppBarMoved => bars.display(true)?,
            Event::ShellRestart => {
                bars.appbar.register();
                bars.tray.raise();
                bars.display(true)?;
            }
            Event::FullScreen(fullscreen) => {
                bars.fullscreen = fullscreen;
                bars.top.position(bars.top_rect, !fullscreen)?;
            }
            Event::Clock => {
                let time = platform::clock(bars.top.0)?;
                if bars.time != time {
                    bars.time = time;
                    bars.draw()?;
                }
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
            Event::Click(owner, x, y) if owner == bars.top.0 => match bars.status.hit(x, y) {
                StatusHit::Tray(slot) => bars.tray_click(slot, false)?,
                StatusHit::Overflow => bars.tray_menu()?,
                StatusHit::Bar if bars.menu(owner)? => return Ok(()),
                StatusHit::Bar => {}
            },
            Event::RightClick(owner, x, y) => {
                let hit = (owner == bars.top.0).then(|| bars.status.hit(x, y));
                if let Some(StatusHit::Tray(slot)) = hit {
                    bars.tray_click(slot, true)?;
                } else if bars.menu(owner)? {
                    return Ok(());
                }
            }
            Event::Menu(owner) if bars.menu(owner)? => return Ok(()),
            Event::Click(_, x, y) => {
                if let Some(index) = bars.layout.hit(x, y) {
                    bars.select(index)?;
                }
            }
            _ => {}
        }
    }
}

impl Bars {
    fn position(&self) -> Result<()> {
        self.top.position(self.top_rect, !self.fullscreen)?;
        self.bottom.position(self.layout.bottom, true)
    }

    /// Re-reads the monitor. The appbar strip is negotiated again only when the shell
    /// asks or the monitor changed, since each negotiation changes the work area again.
    fn display(&mut self, replace: bool) -> Result<()> {
        let monitor = platform::monitor()?;
        let replace =
            replace || monitor.bounds != self.monitor.bounds || monitor.dpi != self.monitor.dpi;
        self.monitor = monitor;
        if replace {
            self.top_rect = self
                .appbar
                .place(self.monitor.bounds, status_height(self.monitor.dpi));
        }
        self.apps = applications(&self.pins)?;
        self.restatus();
        self.relayout(self.layout.first)?;
        self.position()?;
        self.time = platform::clock(self.top.0)?;
        self.draw()
    }

    fn relayout(&mut self, first: usize) -> Result<bool> {
        let layout = Layout::new(self.monitor.work, self.monitor.dpi, self.apps.len(), first);
        if self.layout == layout {
            return Ok(false);
        }
        let moved = layout.bottom != self.layout.bottom;
        self.layout = layout;
        self.hover = None;
        if moved {
            self.bottom.position(self.layout.bottom, true)?;
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
        );
    }

    fn tray_click(&mut self, slot: usize, right: bool) -> Result<()> {
        let index = self.shown[slot];
        self.tray_activate(index, right)
    }

    fn tray_activate(&mut self, index: usize, right: bool) -> Result<()> {
        match self.tray.click(index, right, platform::cursor()?) {
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

    fn tray_menu(&mut self) -> Result<()> {
        let icons = self.tray.icons();
        let on_bar = &self.shown[..self.status.tray.len()];
        let mut listed = Vec::new();
        let mut all = Vec::new();
        for (i, icon) in icons.iter().enumerate().filter(|(_, icon)| icon.visible()) {
            if !on_bar.contains(&i) {
                listed.push(i);
            }
            all.push((i, !self.hidden.contains(&icon.key)));
        }
        let point = platform::cursor()?;
        let choice = self
            .tray
            .menu(self.top.0, point, self.monitor.dpi, &listed, &all)?;
        match choice {
            Some(TrayChoice::Activate(index)) => self.tray_activate(index, false)?,
            Some(TrayChoice::Toggle(index)) => {
                let key = self.tray.icons()[index].key.clone();
                if !self.hidden.remove(&key) {
                    self.hidden.insert(key);
                }
                if let Err(error) = tray::save_hidden(&self.hidden) {
                    // The choice still applies for this session.
                    platform::report_error(Some(self.top.0), &error);
                }
                self.restatus();
                self.draw()?;
            }
            None => {}
        }
        Ok(())
    }

    fn refresh(&mut self) -> Result<()> {
        let apps = applications(&self.pins)?;
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
            Err(e)
                if matches!(
                    e.code(),
                    DXGI_ERROR_DEVICE_REMOVED | DXGI_ERROR_DEVICE_RESET | D2DERR_RECREATE_TARGET
                ) =>
            {
                // Release the previous targets before binding a new device to these HWNDs.
                self.renderer.take();
                self.renderer = Some(
                    Renderer::new(self.top.0, self.bottom.0)
                        .map_err(|e| Error::new(e.code(), format!("重建双栏图形设备失败：{e}")))?,
                );
                self.draw_frame()
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
        renderer.draw_top(&self.status, dpi, active, &self.time, &tray)?;
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
                    size: (34 * dpi as i32 / 96).min(r.width).min(r.height).max(1),
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
        renderer.draw_dock(bottom.width, bottom.height, dpi, &icons, hover)
    }

    fn menu(&mut self, owner: HWND) -> Result<bool> {
        let mut labels: Vec<_> = self.apps.iter().map(|app| app.label.clone()).collect();
        labels.push("退出 DeskIsle".into());
        match platform::popup_menu(owner, platform::cursor()?, &labels)? {
            Some(i) if i == self.apps.len() => return Ok(true),
            Some(i) => self.select(i)?,
            None => {}
        }
        Ok(false)
    }

    fn select(&mut self, index: usize) -> Result<()> {
        let target = &self.apps[index];
        let id = target.app_id.clone();
        let was_running = !target.windows.is_empty();
        let result = self.act(&id, was_running);
        if let Err(error) = result {
            platform::report_error(Some(self.bottom.0), &error);
        }
        self.refresh()
    }

    fn act(&self, id: &str, was_running: bool) -> Result<()> {
        let (snapshot, foreground) = shell::enumerate()?;
        let apps = group_apps(&self.pins, &snapshot, foreground.unwrap_or_default());
        let Some(app) = apps.iter().find(|app| app.app_id == id) else {
            return Ok(());
        };
        let window = match app.action() {
            Action::Launch(path) => {
                if !was_running {
                    shell::launch(self.bottom.0, path, &snapshot)?;
                }
                None
            }
            Action::Activate(window) => Some(window.id),
            Action::Choose(windows) => {
                shell::choose_window(self.bottom.0, windows, platform::cursor()?)?
            }
        };
        if let Some(window) = window {
            shell::activate(id, window)?;
        }
        Ok(())
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            platform::report_error(None, &error);
            std::process::ExitCode::FAILURE
        }
    }
}
