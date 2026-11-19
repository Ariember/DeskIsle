#![windows_subsystem = "windows"]

mod dock;
mod platform;
mod render;

use dock::{Action, AppGroup, Layout, PinnedApp, Rect, group_apps};
use platform::{Bar, Event, shell};
use render::{DockIcon, Renderer};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
use windows::core::{Error, Result};

struct Bars {
    // Unsubscribe first, then release composition targets before their HWNDs.
    events: shell::WindowEvents,
    renderer: Option<Renderer>,
    top: Bar,
    bottom: Bar,
    pins: Vec<PinnedApp>,
    apps: Vec<AppGroup>,
    work: Rect,
    dpi: u32,
    layout: Layout,
    hover: Option<usize>,
    time: String,
}

fn applications(pins: &[PinnedApp]) -> Result<Vec<AppGroup>> {
    let (windows, foreground) = shell::enumerate()?;
    Ok(group_apps(pins, &windows, foreground.unwrap_or_default()))
}

fn run() -> Result<()> {
    let _desktop = platform::Desktop::init()?;
    let (top, bottom) = platform::create_bars()?;
    let events = shell::WindowEvents::subscribe(bottom.0)?;
    let pins = shell::pins()?;
    let apps = applications(&pins)?;
    let (work, dpi) = platform::work_area()?;
    let layout = Layout::new(work, dpi, apps.len(), 0);
    let time = platform::clock(top.0)?;
    let renderer = Some(Renderer::new(top.0, bottom.0)?);
    let mut bars = Bars {
        events,
        renderer,
        top,
        bottom,
        pins,
        apps,
        work,
        dpi,
        layout,
        hover: None,
        time,
    };
    bars.draw()?;
    bars.position()?;
    loop {
        let event = match platform::next_event()? {
            Event::Click(owner, _, _) if owner == bars.top.0 => Event::Menu(owner),
            event => event,
        };
        match event {
            Event::Quit => return Ok(()),
            Event::Paint => bars.draw()?,
            Event::Refresh => {
                bars.events.acknowledge();
                bars.refresh()?;
            }
            Event::Display => {
                (bars.work, bars.dpi) = platform::work_area()?;
                bars.apps = applications(&bars.pins)?;
                bars.relayout(bars.layout.first)?;
                bars.time = platform::clock(bars.top.0)?;
                bars.draw()?;
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
        self.top.position(self.layout.top)?;
        self.bottom.position(self.layout.bottom)
    }

    fn relayout(&mut self, first: usize) -> Result<bool> {
        let layout = Layout::new(self.work, self.dpi, self.apps.len(), first);
        if self.layout == layout {
            return Ok(false);
        }
        let moved = layout.top != self.layout.top || layout.bottom != self.layout.bottom;
        self.layout = layout;
        self.hover = None;
        if moved {
            self.position()?;
        }
        Ok(true)
    }

    fn refresh(&mut self) -> Result<()> {
        let apps = applications(&self.pins)?;
        if self.apps != apps {
            self.apps = apps;
            self.relayout(self.layout.first)?;
            self.draw()?;
        }
        Ok(())
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
        let active = self
            .apps
            .iter()
            .find(|app| app.active)
            .map_or("桌面", |app| app.label.as_str());
        let top = self.layout.top;
        renderer.draw_top(top.width, top.height, self.dpi, active, &self.time)?;
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
                    size: (34 * self.dpi as i32 / 96)
                        .min(r.width)
                        .min(r.height)
                        .max(1),
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
        renderer.draw_dock(bottom.width, bottom.height, self.dpi, &icons, hover)
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
