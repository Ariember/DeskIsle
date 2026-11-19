#![windows_subsystem = "windows"]

mod dock;
mod menu;
mod platform;
mod render;

use std::collections::BTreeSet;

use dock::{
    Action, AppGroup, Layout, PinnedApp, Rect, STATUS_HEIGHT, StatusHit, StatusLayout, group_apps,
};
use menu::{Anchor, Command, Hit, Menu, MenuLayout, TrayEntry};
use platform::tray::{self, TrayHost};
use platform::{AppBar, Bar, Event, MenuWindow, Monitor, shell};
use render::{DockIcon, Renderer};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_MENU, VK_RETURN, VK_SPACE, VK_UP,
};
use windows::core::{Error, HSTRING, Result};

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
    foreground: Option<isize>,
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
    /// A left press on a bar dismissed the open menu; its release must not click.
    swallow_click: bool,
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

fn is_device_lost(error: &Error) -> bool {
    matches!(
        error.code(),
        DXGI_ERROR_DEVICE_REMOVED | DXGI_ERROR_DEVICE_RESET | D2DERR_RECREATE_TARGET
    )
}

fn load_pins() -> Result<Vec<PinnedApp>> {
    match shell::load_pins() {
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
    let pins = load_pins()?;
    let (apps, foreground) = applications(&pins)?;
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
        foreground,
        status: StatusLayout::new(top_rect.width, top_rect.height, monitor.dpi, 0, 0),
        monitor,
        top_rect,
        fullscreen: false,
        layout,
        shown: Vec::new(),
        hidden,
        hover: None,
        time,
        swallow_click: false,
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
            Event::Press(..) => bars.swallow_click = false,
            Event::Click(..) if bars.swallow_click => bars.swallow_click = false,
            Event::Click(owner, x, y) if owner == bars.top.0 => {
                let quit = match bars.status.hit(x, y) {
                    StatusHit::Tray(slot) => {
                        bars.tray_click(slot, false)?;
                        false
                    }
                    StatusHit::Overflow => bars.tray_menu()?,
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
        (self.apps, self.foreground) = applications(&self.pins)?;
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

    fn tray_toggle(&mut self, index: usize) -> Result<()> {
        let key = self.tray.icons()[index].key.clone();
        if !self.hidden.remove(&key) {
            self.hidden.insert(key);
        }
        if let Err(error) = tray::save_hidden(&self.hidden) {
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

    /// Opens the menu for whatever is under a right click. Returns whether to quit.
    fn context_menu(&mut self, owner: HWND, x: i32, y: i32) -> Result<bool> {
        if owner == self.top.0 {
            return match self.status.hit(x, y) {
                StatusHit::Tray(slot) => {
                    // The application owns its tray icon's menu.
                    self.tray_click(slot, true)?;
                    Ok(false)
                }
                StatusHit::Overflow => self.tray_menu(),
                StatusHit::Clock => self.clock_menu(),
                StatusHit::Title => match self.apps.iter().position(|app| app.active) {
                    Some(index) => {
                        let menu = menu::app_menu(&self.apps[index], self.foreground);
                        let anchor =
                            self.top_anchor(self.status.title.x + px(self.monitor.dpi, 60));
                        self.show_menu(menu, anchor)
                    }
                    None => self.bar_menu(),
                },
                StatusHit::Bar => self.bar_menu(),
            };
        }
        if owner != self.bottom.0 {
            return Ok(false);
        }
        match self.layout.hit(x, y) {
            Some(index) => {
                let menu = menu::app_menu(&self.apps[index], self.foreground);
                let anchor = self.icon_anchor(index);
                self.show_menu(menu, anchor)
            }
            None => self.bar_menu(),
        }
    }

    fn bar_menu(&mut self) -> Result<bool> {
        let menu = menu::bar_menu(&self.apps);
        let anchor = self.cursor_anchor()?;
        self.show_menu(menu, anchor)
    }

    fn clock_menu(&mut self) -> Result<bool> {
        let (date, year) = platform::date();
        let clock = self.status.clock;
        let anchor = self.top_anchor(clock.x + clock.width / 2);
        self.show_menu(menu::clock_menu(&date, &year), anchor)
    }

    fn tray_menu(&mut self) -> Result<bool> {
        let on_bar = &self.shown[..self.status.tray.len()];
        let entries = self
            .tray
            .icons()
            .iter()
            .enumerate()
            .filter(|(_, icon)| icon.visible())
            .map(|(index, icon)| TrayEntry {
                index,
                label: &icon.label,
                on_bar: on_bar.contains(&index),
                kept: !self.hidden.contains(&icon.key),
            })
            .collect::<Vec<_>>();
        let menu = menu::tray_menu(&entries);
        let anchor = match self.status.overflow {
            Some(r) => self.top_anchor(r.x + r.width / 2),
            None => self.cursor_anchor()?,
        };
        self.show_menu(menu, anchor)
    }

    /// Below the status bar at client `x`.
    fn top_anchor(&self, x: i32) -> Anchor {
        Anchor {
            x: self.top_rect.x + x,
            y: self.top_rect.y + self.top_rect.height,
            up: false,
        }
    }

    /// Above the Dock icon of application `index`, or at the pointer when it scrolled out.
    fn icon_anchor(&self, index: usize) -> Anchor {
        let bottom = self.layout.bottom;
        match self.layout.slots.iter().find(|slot| slot.index == index) {
            Some(slot) => Anchor {
                x: bottom.x + slot.rect.x + slot.rect.width / 2,
                y: bottom.y,
                up: true,
            },
            None => self.cursor_anchor().unwrap_or(Anchor {
                x: bottom.x + bottom.width / 2,
                y: bottom.y,
                up: true,
            }),
        }
    }

    /// At the pointer, clear of the bar it is over.
    fn cursor_anchor(&self) -> Result<Anchor> {
        let point = platform::cursor()?;
        let top = self.top_rect;
        let bottom = self.layout.bottom;
        Ok(if point.y < top.y + top.height {
            Anchor {
                x: point.x,
                y: top.y + top.height,
                up: false,
            }
        } else if point.y >= bottom.y {
            Anchor {
                x: point.x,
                y: bottom.y,
                up: true,
            }
        } else {
            let middle = self.monitor.bounds.y + self.monitor.bounds.height / 2;
            Anchor {
                x: point.x,
                y: point.y,
                up: point.y > middle,
            }
        })
    }

    /// Shows `menu`, then carries out the chosen command. Returns whether to quit.
    fn show_menu(&mut self, menu: Menu<Command>, anchor: Anchor) -> Result<bool> {
        match self.popup(menu, anchor)? {
            Some(command) => self.run_command(command),
            None => Ok(false),
        }
    }

    /// Runs the menu until the user chooses, dismisses or clicks elsewhere. Bar events
    /// that arrive meanwhile are held and handled afterwards in their original order.
    fn popup(&mut self, mut menu: Menu<Command>, anchor: Anchor) -> Result<Option<Command>> {
        let dpi = self.monitor.dpi;
        let renderer = self.renderer.as_mut().expect("renderer bound to bars");
        let layout = MenuLayout::new(&mut menu, dpi, |text, size, bold| {
            renderer.measure(text, size, bold)
        });
        let window = MenuWindow::create(layout.place(anchor, self.monitor.bounds))?;
        let tray = &self.tray;
        let resolve = |index: usize| tray.icons().get(index).and_then(|icon| icon.icon());
        if let Err(error) = renderer.open_menu(window.0, &menu, &layout, dpi, &resolve) {
            renderer.close_menu();
            drop(window);
            return self.recover(error).map(|()| None);
        }
        let opened_over = platform::foreground();
        let active = window.show();
        let mut hover: Option<usize> = None;
        let mut held = Vec::new();
        let mut failure = None;
        let choice = loop {
            let mut target = hover;
            match platform::next_event()? {
                // The shadow margin belongs to the window but not to the menu.
                Event::MenuPick(x, y) if layout.hit(&menu, x, y) == Hit::Outside => break None,
                Event::MenuMove(x, y) => {
                    target = match layout.hit(&menu, x, y) {
                        Hit::Item(index) => Some(index),
                        _ => None,
                    }
                }
                Event::MenuLeave => target = None,
                Event::MenuPick(x, y) => {
                    if let Hit::Item(index) = layout.hit(&menu, x, y) {
                        break menu.command(index).cloned();
                    }
                }
                Event::MenuKey(key) => match VIRTUAL_KEY(key) {
                    VK_DOWN => target = menu.step(hover, true),
                    VK_UP => target = menu.step(hover, false),
                    VK_HOME => target = menu.step(None, true),
                    VK_END => target = menu.step(None, false),
                    VK_RETURN | VK_SPACE => {
                        if let Some(index) = hover {
                            break menu.command(index).cloned();
                        }
                    }
                    VK_ESCAPE | VK_MENU => break None,
                    _ => {}
                },
                Event::MenuDismiss => break None,
                Event::Press(_, right) => {
                    // A right press goes on to open the menu of what it hit.
                    self.swallow_click = !right;
                    break None;
                }
                Event::Quit => {
                    held.push(Event::Quit);
                    break None;
                }
                Event::Refresh => {
                    held.push(Event::Refresh);
                    // Without activation the menu cannot see clicks elsewhere; another
                    // window coming to the front stands in for them.
                    if !active && !window.is_foreground() && platform::foreground() != opened_over {
                        break None;
                    }
                }
                event => held.push(event),
            }
            if target != hover {
                let danger = target.is_some_and(
                    |index| matches!(&menu.entries[index], menu::Entry::Item(item) if item.danger),
                );
                let renderer = self.renderer.as_mut().expect("renderer bound to bars");
                if let Err(error) = renderer.menu_hover(target, danger) {
                    failure = Some(error);
                    break None;
                }
                hover = target;
            }
        };
        self.renderer
            .as_mut()
            .expect("renderer bound to bars")
            .close_menu();
        drop(window);
        platform::requeue(held);
        if let Some(error) = failure {
            self.recover(error)?;
        }
        Ok(choice)
    }

    /// Rebuilds the graphics device after it was lost, or reports another menu failure.
    fn recover(&mut self, error: Error) -> Result<()> {
        if is_device_lost(&error) {
            self.draw()
        } else {
            platform::report_error(Some(self.bottom.0), &error);
            Ok(())
        }
    }

    /// Carries out a menu command. Failures are reported and leave DeskIsle running.
    fn run_command(&mut self, command: Command) -> Result<bool> {
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
            Command::NewInstance(path) => {
                shell::execute(owner, &HSTRING::from(path.as_os_str()), None)
            }
            Command::Reveal(path) => shell::reveal(owner, &path),
            Command::Pin(path) => {
                shell::pin(&mut self.pins, &path);
                self.save_pins()
            }
            Command::Unpin(id) => {
                self.pins.retain(|pin| pin.app_id != id);
                self.save_pins()
            }
            Command::Close(id, windows) => shell::close_windows(&id, &windows).map(drop),
            Command::TaskManager => shell::execute(owner, &HSTRING::from("taskmgr.exe"), None),
            Command::ShowDesktop => self.tray.toggle_desktop(),
            Command::DateTime => {
                shell::execute(owner, &HSTRING::from("ms-settings:dateandtime"), None)
            }
            Command::Notifications => {
                shell::execute(owner, &HSTRING::from("ms-actioncenter:"), None)
            }
            Command::TrayActivate(index) => {
                self.tray_activate(index, false)?;
                return Ok(false);
            }
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

    fn save_pins(&self) -> Result<()> {
        // The new pins apply for this session even when they cannot be saved.
        shell::save_pins(&self.pins)
    }

    fn select(&mut self, index: usize) -> Result<()> {
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
                if let Some(Command::Window(id, window)) = self.popup(menu, anchor)? {
                    shell::activate(&id, window)?;
                }
            }
        }
        Ok(())
    }
}

fn px(dpi: u32, dip: i32) -> i32 {
    (dip * dpi as i32 + 48) / 96
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
