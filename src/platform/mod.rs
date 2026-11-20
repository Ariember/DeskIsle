pub mod audio;
pub mod display;
pub mod media;
pub mod network;
pub mod power;
pub mod shell;
pub mod startup;
pub mod taskbar;
pub mod tray;

use crate::geometry::Rect;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::mem::size_of;
use windows::Win32::{
    Foundation::*,
    Graphics::{
        Dwm::{
            DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        },
        Gdi::*,
    },
    System::{Com::*, LibraryLoader::GetModuleHandleW, SystemInformation::GetLocalTime},
    UI::{
        Controls::MARGINS,
        HiDpi::*,
        Input::KeyboardAndMouse::*,
        Shell::{
            ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABN_FULLSCREENAPP,
            ABN_POSCHANGED, APPBARDATA, SHAppBarMessage,
        },
        WindowsAndMessaging::*,
    },
};
use windows::core::{BOOL, Error, HSTRING, PCWSTR, Result, w};

pub struct Desktop;

impl Desktop {
    pub fn init() -> Result<Self> {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        Ok(Self)
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub struct Bar(pub HWND);

impl Drop for Bar {
    fn drop(&mut self) {
        // Both bars are created, used and destroyed on the same UI thread.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

const MENU_CLASS: PCWSTR = w!("DeskIsle.Menu");
const PANEL_CLASS: PCWSTR = w!("DeskIsle.Settings");

pub fn create_bars() -> Result<(Bar, Bar)> {
    unsafe {
        let instance = GetModuleHandleW(None)?.into();
        let class = w!("DeskIsle.Bar");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(Error::from_thread());
        }
        let restart = RegisterWindowMessageW(w!("TaskbarCreated"));
        if restart == 0 {
            return Err(Error::from_thread());
        }
        TASKBAR_CREATED.set(restart);
        let menu = WNDCLASSW {
            lpfnWndProc: Some(menu_proc),
            hInstance: instance,
            lpszClassName: MENU_CLASS,
            hCursor: wc.hCursor,
            ..Default::default()
        };
        if RegisterClassW(&menu) == 0 {
            return Err(Error::from_thread());
        }
        let panel = WNDCLASSW {
            lpfnWndProc: Some(panel_proc),
            lpszClassName: PANEL_CLASS,
            ..menu
        };
        if RegisterClassW(&panel) == 0 {
            return Err(Error::from_thread());
        }
        let create = |title| {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                class,
                title,
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )
            .map(Bar)
        };
        Ok((create(w!("DeskIsle Status"))?, create(w!("DeskIsle Dock"))?))
    }
}

pub struct Monitor {
    pub bounds: Rect,
    pub work: Rect,
    pub dpi: u32,
}

fn to_rect(r: RECT) -> Rect {
    Rect {
        x: r.left,
        y: r.top,
        width: r.right - r.left,
        height: r.bottom - r.top,
    }
}

pub fn monitor() -> Result<Monitor> {
    unsafe {
        let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(monitor, &mut info).ok()?;
        let mut dpi = 0;
        let mut dpi_y = 0;
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y)?;
        let (bounds, work) = (to_rect(info.rcMonitor), to_rect(info.rcWork));
        if work.width <= 0 || work.height <= 0 || bounds.width <= 0 || dpi == 0 {
            return Err(Error::new(E_FAIL, "显示器未提供有效的工作区或 DPI"));
        }
        Ok(Monitor { bounds, work, dpi })
    }
}

pub const APPBAR_CHANGED: u32 = WM_APP + 2;

/// Reserves the top edge of the primary monitor for the status bar, so maximized
/// windows keep their caption buttons below it.
pub struct AppBar {
    hwnd: HWND,
    explorer: Option<(HWND, u32)>,
}

impl AppBar {
    fn data(&self) -> APPBARDATA {
        APPBARDATA {
            cbSize: size_of::<APPBARDATA>() as u32,
            hWnd: self.hwnd,
            uCallbackMessage: APPBAR_CHANGED,
            uEdge: ABE_TOP,
            ..Default::default()
        }
    }

    /// `hwnd` is the status bar and outlives the registration.
    pub fn new(hwnd: HWND) -> Result<Self> {
        let mut bar = Self {
            hwnd,
            explorer: None,
        };
        bar.register()?;
        Ok(bar)
    }

    /// TaskbarCreated is also broadcast by our tray host to repopulate its icons.
    pub fn register(&mut self) -> Result<()> {
        let tray = taskbar::explorer_tray()
            .ok_or_else(|| Error::new(E_FAIL, "找不到 Explorer 主任务栏"))?;
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(tray, Some(&mut pid)) };
        if self.explorer == Some((tray, pid)) {
            return Ok(());
        }
        let mut data = self.data();
        if unsafe { SHAppBarMessage(ABM_NEW, &mut data) } == 0 {
            return Err(Error::new(E_FAIL, "Explorer 未注册顶部状态栏"));
        }
        self.explorer = Some((tray, pid));
        Ok(())
    }

    /// Returns the strip Explorer granted along the top edge.
    pub fn place(&self, bounds: Rect, height: i32) -> Result<Rect> {
        let requested = RECT {
            left: bounds.x,
            top: bounds.y,
            right: bounds.x + bounds.width,
            bottom: bounds.y + height,
        };
        let mut data = self.data();
        data.rc = requested;
        unsafe {
            if SHAppBarMessage(ABM_QUERYPOS, &mut data) == 0 {
                return Err(Error::new(E_FAIL, "无法查询顶部状态栏的可用位置"));
            }
            // QUERYPOS moves the strip below other top-edge appbars; keep our own height.
            data.rc.bottom = data.rc.top + height;
            if SHAppBarMessage(ABM_SETPOS, &mut data) == 0 {
                return Err(Error::new(E_FAIL, "Explorer 未预留顶部状态栏工作区"));
            }
        }
        let granted = to_rect(data.rc);
        if granted.width <= 0 || granted.height <= 0 {
            Err(Error::new(E_FAIL, "Explorer 返回了无效的顶部状态栏位置"))
        } else {
            Ok(granted)
        }
    }
}

impl Drop for AppBar {
    fn drop(&mut self) {
        if self.explorer.is_some() {
            let mut data = self.data();
            unsafe { SHAppBarMessage(ABM_REMOVE, &mut data) };
        }
    }
}

impl Bar {
    pub fn position(&self, r: Rect, visible: bool) -> Result<()> {
        let show = if visible {
            SWP_SHOWWINDOW
        } else {
            SWP_HIDEWINDOW
        };
        unsafe {
            SetWindowPos(
                self.0,
                Some(HWND_TOPMOST),
                r.x,
                r.y,
                r.width,
                r.height,
                SWP_NOACTIVATE | show,
            )
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Down,
    Up,
    Home,
    End,
    Select,
    Dismiss,
    Other,
}

/// Keys the settings panel responds to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanelKey {
    Tab {
        back: bool,
    },
    /// Ctrl+Tab, Page Up and Page Down switch pages.
    Page {
        back: bool,
    },
    Up,
    Down,
    Left,
    Right,
    Select,
    Escape,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Paint,
    Refresh,
    Display,
    Clock,
    Move(HWND, i32, i32),
    Leave,
    Click(HWND, i32, i32),
    RightClick(HWND, i32, i32),
    Menu(HWND),
    /// A mouse button went down on a bar (`true` for the right button).
    Press(HWND, bool),
    /// Pointer input in the open menu window, in its client pixels.
    MenuMove(i32, i32),
    /// The left button went down in the menu, which may start dragging a slider.
    MenuPress(i32, i32),
    MenuLeave,
    MenuPick(i32, i32),
    MenuKey(MenuKey),
    /// The menu lost activation or Windows cancelled modes.
    MenuDismiss,
    /// The shell may have moved other appbars or the work area.
    AppBarMoved,
    /// A full-screen application opened (`true`) or closed on the monitor.
    FullScreen(bool),
    /// Explorer started again and expects appbars and tray icons to register.
    ShellRestart,
    Tray,
    /// The default output's volume or mute changed.
    Audio,
    /// The power source or battery charge changed.
    Power,
    Scroll(i16),
    /// Pointer input in the settings panel, in its client pixels.
    PanelMove(i32, i32),
    PanelLeave,
    PanelClick(i32, i32),
    PanelScroll(i16),
    PanelKey(PanelKey),
    /// The panel moved to a monitor with this DPI and already took the suggested size.
    PanelDpi(u32),
    PanelClose,
    Quit,
}

impl Event {
    fn for_menu(&self) -> bool {
        matches!(
            self,
            Event::MenuMove(..)
                | Event::MenuPress(..)
                | Event::MenuLeave
                | Event::MenuPick(..)
                | Event::MenuKey(_)
                | Event::MenuDismiss
        )
    }

    fn for_panel(&self) -> bool {
        matches!(
            self,
            Event::PanelMove(..)
                | Event::PanelLeave
                | Event::PanelClick(..)
                | Event::PanelScroll(_)
                | Event::PanelKey(_)
                | Event::PanelDpi(_)
                | Event::PanelClose
        )
    }
}

thread_local! {
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static EVENTS: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    /// The settings panel's caption height and the left edge of its close button, in
    /// client pixels, for hit testing inside its window procedure.
    static PANEL_CAPTION: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Copy message values only. Shell menus and SetWindowPos can re-enter this callback.
    let (x, y) = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
    let event = match msg {
        WM_PAINT => {
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            Event::Paint
        }
        WM_CLOSE => Event::Quit,
        shell::WINDOWS_CHANGED => Event::Refresh,
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => Event::Display,
        WM_POWERBROADCAST if wp.0 == PBT_APMRESUMEAUTOMATIC as usize => Event::Display,
        WM_POWERBROADCAST if wp.0 == PBT_APMPOWERSTATUSCHANGE as usize => Event::Power,
        audio::AUDIO_CHANGED => Event::Audio,
        m if m != 0 && m == TASKBAR_CREATED.get() => Event::ShellRestart,
        APPBAR_CHANGED if wp.0 == ABN_POSCHANGED as usize => Event::AppBarMoved,
        APPBAR_CHANGED if wp.0 == ABN_FULLSCREENAPP as usize => Event::FullScreen(lp.0 != 0),
        APPBAR_CHANGED => return LRESULT(0),
        WM_TIMER | WM_TIMECHANGE => Event::Clock,
        WM_MOUSEMOVE => Event::Move(hwnd, x, y),
        windows::Win32::UI::Controls::WM_MOUSELEAVE => Event::Leave,
        WM_LBUTTONDOWN => Event::Press(hwnd, false),
        WM_RBUTTONDOWN => Event::Press(hwnd, true),
        WM_LBUTTONUP => Event::Click(hwnd, x, y),
        WM_RBUTTONUP => Event::RightClick(hwnd, x, y),
        WM_CONTEXTMENU => Event::Menu(hwnd),
        WM_MOUSEWHEEL => Event::Scroll((wp.0 >> 16) as i16),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    };
    queue(hwnd, event);
    LRESULT(0)
}

/// Queues `event` for the UI loop. `hwnd` belongs to the UI thread calling this.
fn queue(hwnd: HWND, event: Event) {
    let wake = EVENTS.with_borrow_mut(|events| {
        let wake = events.is_empty();
        if events.back() != Some(&event)
            || matches!(
                event,
                Event::Click(..)
                    | Event::RightClick(..)
                    | Event::Press(..)
                    | Event::Scroll(_)
                    | Event::MenuPick(..)
                    | Event::MenuPress(..)
                    | Event::MenuKey(_)
                    | Event::PanelClick(..)
                    | Event::PanelScroll(_)
                    | Event::PanelKey(_)
            )
        {
            events.push_back(event);
        }
        wake
    });
    if wake {
        // A sent message can arrive while GetMessageW is waiting for a posted message.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
    }
}

pub fn next_event() -> Result<Event> {
    loop {
        if let Some(event) = EVENTS.with_borrow_mut(VecDeque::pop_front) {
            return Ok(event);
        }
        let mut msg = MSG::default();
        unsafe {
            match GetMessageW(&mut msg, None, 0, 0).0 {
                -1 => return Err(Error::from_thread()),
                0 => return Ok(Event::Quit),
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

pub fn track_leave(hwnd: HWND) -> Result<()> {
    let mut tracking = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe { TrackMouseEvent(&mut tracking) }
}

pub fn cursor() -> Result<POINT> {
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt)? };
    Ok(pt)
}

pub fn clock(hwnd: HWND) -> Result<String> {
    unsafe {
        let now = GetLocalTime();
        let delay = 60_000 - u32::from(now.wSecond.min(59)) * 1000 - u32::from(now.wMilliseconds);
        if SetTimer(Some(hwnd), 1, delay, None) == 0 {
            return Err(Error::new(E_FAIL, "无法设置时钟定时器"));
        }
        Ok(format!("{:02}:{:02}", now.wHour, now.wMinute))
    }
}

/// DeskIsle's own popup menu window. It takes activation so that a click anywhere else
/// deactivates it; clicks on the bars, which never activate, arrive as [`Event::Press`].
pub struct MenuWindow(pub HWND);

impl MenuWindow {
    /// Creates the hidden window at `r`, in screen pixels, for the renderer to bind to.
    pub fn create(r: Rect) -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
                MENU_CLASS,
                w!("DeskIsle Menu"),
                WS_POPUP,
                r.x,
                r.y,
                r.width,
                r.height,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            Ok(Self(hwnd))
        }
    }

    /// Shows the menu above the bars and gives it the keyboard. Returns whether Windows
    /// made it the foreground window; it may refuse when another process holds the
    /// foreground lock, in which case the caller dismisses on the next foreground change.
    pub fn show(&self) -> bool {
        unsafe {
            let _ = SetWindowPos(
                self.0,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = SetForegroundWindow(self.0);
            let _ = SetFocus(Some(self.0));
            GetForegroundWindow() == self.0
        }
    }

    pub fn is_foreground(&self) -> bool {
        unsafe { GetForegroundWindow() == self.0 }
    }

    /// Keeps pointer input coming while a slider is dragged outside the menu.
    pub fn capture(&self) {
        unsafe { SetCapture(self.0) };
    }

    pub fn release(&self) {
        let _ = unsafe { ReleaseCapture() };
    }

    /// Moves and resizes the menu after its content changed, in screen pixels.
    pub fn place(&self, r: Rect) {
        // A failed move leaves the old size; the next rebuild tries again.
        let _ = unsafe {
            SetWindowPos(
                self.0,
                Some(HWND_TOPMOST),
                r.x,
                r.y,
                r.width,
                r.height,
                SWP_NOACTIVATE,
            )
        };
    }
}

impl Drop for MenuWindow {
    fn drop(&mut self) {
        // Created and destroyed on the UI thread. Destroying an active window sends it a
        // deactivation, which must not close the next menu.
        let _ = unsafe { DestroyWindow(self.0) };
        EVENTS.with_borrow_mut(|events| events.retain(|e| !e.for_menu()));
    }
}

unsafe extern "system" fn menu_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
    let event = match msg {
        WM_PAINT => {
            // DirectComposition presents the content; nothing to paint here.
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let _ = track_leave(hwnd);
            Event::MenuMove(x, y)
        }
        windows::Win32::UI::Controls::WM_MOUSELEAVE => Event::MenuLeave,
        WM_LBUTTONDOWN => Event::MenuPress(x, y),
        WM_LBUTTONUP | WM_RBUTTONUP => Event::MenuPick(x, y),
        WM_KEYDOWN | WM_SYSKEYDOWN => Event::MenuKey(match VIRTUAL_KEY(wp.0 as u16) {
            VK_DOWN => MenuKey::Down,
            VK_UP => MenuKey::Up,
            VK_HOME => MenuKey::Home,
            VK_END => MenuKey::End,
            VK_RETURN | VK_SPACE => MenuKey::Select,
            VK_ESCAPE | VK_MENU => MenuKey::Dismiss,
            _ => MenuKey::Other,
        }),
        WM_ACTIVATE if wp.0 & 0xffff == WA_INACTIVE as usize => Event::MenuDismiss,
        WM_CANCELMODE | WM_CLOSE => Event::MenuDismiss,
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    };
    queue(hwnd, event);
    LRESULT(0)
}

/// The settings window. It has a caption style so DWM gives it a shadow, rounded corners
/// and a place in Alt+Tab, while its client area covers the frame and DeskIsle draws it.
pub struct PanelWindow(pub HWND);

impl PanelWindow {
    /// Creates the hidden window at `r`, in screen pixels. `caption` is the draggable
    /// height and the left edge of the close button, in client pixels.
    pub fn create(r: Rect, caption: (i32, i32), dark: bool) -> Result<Self> {
        PANEL_CAPTION.set(caption);
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                PANEL_CLASS,
                w!("DeskIsle 设置"),
                WS_POPUP | WS_CAPTION | WS_SYSMENU,
                r.x,
                r.y,
                r.width,
                r.height,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let window = Self(hwnd);
            window.set_dark(dark);
            // Cosmetic only: builds without this attribute keep square corners.
            let round = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&raw const round).cast(),
                size_of_val(&round) as u32,
            );
            let margins = MARGINS {
                cxLeftWidth: 1,
                cxRightWidth: 1,
                cyTopHeight: 1,
                cyBottomHeight: 1,
            };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            Ok(window)
        }
    }

    /// Matches the frame DWM draws around the panel, such as its border, to the palette.
    pub fn set_dark(&self, dark: bool) {
        let dark = BOOL::from(dark);
        // Cosmetic only: older builds ignore the attribute.
        let _ = unsafe {
            DwmSetWindowAttribute(
                self.0,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark).cast(),
                size_of::<BOOL>() as u32,
            )
        };
    }

    /// Shows the panel, or brings it back to the front when it is already open.
    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.0, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(self.0);
        }
    }

    /// Resizes the window around its current center, kept inside `work`.
    pub fn resize(&self, width: i32, height: i32, work: Rect, caption: (i32, i32)) -> Result<()> {
        PANEL_CAPTION.set(caption);
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.0, &mut r)? };
        let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        let x = (cx - width / 2).clamp(work.x, (work.x + work.width - width).max(work.x));
        let y = (cy - height / 2).clamp(work.y, (work.y + work.height - height).max(work.y));
        unsafe {
            SetWindowPos(
                self.0,
                None,
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
    }

    /// The work area of the monitor showing the panel.
    pub fn work_area(&self) -> Result<Rect> {
        unsafe {
            let monitor = MonitorFromWindow(self.0, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(monitor, &mut info).ok()?;
            Ok(to_rect(info.rcWork))
        }
    }
}

impl Drop for PanelWindow {
    fn drop(&mut self) {
        // Created and destroyed on the UI thread; drop input the window still queued.
        let _ = unsafe { DestroyWindow(self.0) };
        EVENTS.with_borrow_mut(|events| events.retain(|e| !e.for_panel()));
    }
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
    let event = match msg {
        // The client area takes the whole window; DeskIsle draws the caption itself.
        WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
        WM_NCHITTEST => {
            let mut point = POINT { x, y };
            let _ = unsafe { ScreenToClient(hwnd, &mut point) };
            let (caption, close) = PANEL_CAPTION.get();
            let hit = if (0..caption).contains(&point.y) && point.x < close {
                HTCAPTION
            } else {
                HTCLIENT
            };
            return LRESULT(hit as isize);
        }
        WM_PAINT => {
            // DirectComposition presents the content; nothing to paint here.
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let _ = track_leave(hwnd);
            Event::PanelMove(x, y)
        }
        windows::Win32::UI::Controls::WM_MOUSELEAVE => Event::PanelLeave,
        WM_LBUTTONUP => Event::PanelClick(x, y),
        WM_MOUSEWHEEL => Event::PanelScroll((wp.0 >> 16) as i16),
        WM_KEYDOWN => {
            let held = |key: VIRTUAL_KEY| unsafe { GetKeyState(key.0 as i32) } < 0;
            let back = held(VK_SHIFT);
            Event::PanelKey(match VIRTUAL_KEY(wp.0 as u16) {
                VK_TAB if held(VK_CONTROL) => PanelKey::Page { back },
                VK_TAB => PanelKey::Tab { back },
                VK_PRIOR => PanelKey::Page { back: true },
                VK_NEXT => PanelKey::Page { back: false },
                VK_UP => PanelKey::Up,
                VK_DOWN => PanelKey::Down,
                VK_LEFT => PanelKey::Left,
                VK_RIGHT => PanelKey::Right,
                VK_RETURN | VK_SPACE => PanelKey::Select,
                VK_ESCAPE => PanelKey::Escape,
                _ => PanelKey::Other,
            })
        }
        WM_DPICHANGED => {
            // lParam points to the suggested rectangle for the new DPI during this call.
            let r = unsafe { *(lp.0 as *const RECT) };
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            Event::PanelDpi((wp.0 & 0xffff) as u32)
        }
        WM_CLOSE => Event::PanelClose,
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    };
    queue(hwnd, event);
    LRESULT(0)
}

/// The window that currently has the foreground, to tell when another one takes it.
pub fn foreground() -> HWND {
    unsafe { GetForegroundWindow() }
}

/// A maximized window's frame can extend outside the monitor while its client area
/// is smaller. WINDOWINFO gives us the client rectangle in screen coordinates.
pub fn window_covers_monitor(id: isize, bounds: Rect) -> bool {
    let mut info = WINDOWINFO {
        cbSize: size_of::<WINDOWINFO>() as u32,
        ..Default::default()
    };
    // The foreground window from the enumeration can close before this query.
    unsafe { GetWindowInfo(HWND(id as _), &mut info) }.is_ok()
        && info.rcClient.left <= bounds.x
        && info.rcClient.top <= bounds.y
        && info.rcClient.right >= bounds.x + bounds.width
        && info.rcClient.bottom >= bounds.y + bounds.height
}

/// Puts back events the menu loop held while it ran, ahead of anything newer.
pub fn requeue(held: Vec<Event>) {
    EVENTS.with_borrow_mut(|events| {
        for event in held.into_iter().rev() {
            events.push_front(event);
        }
    });
}

/// The local date for the clock menu, such as "11月19日 星期四".
pub fn date() -> (String, String) {
    let now = unsafe { GetLocalTime() };
    const WEEKDAYS: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
    let weekday = WEEKDAYS[usize::from(now.wDayOfWeek) % 7];
    (
        format!("{}月{}日 星期{weekday}", now.wMonth, now.wDay),
        format!("{} 年", now.wYear),
    )
}

pub fn report_error(owner: Option<HWND>, error: &Error) {
    let text = HSTRING::from(error.to_string());
    unsafe {
        MessageBoxW(
            owner,
            PCWSTR(text.as_ptr()),
            w!("DeskIsle"),
            MB_OK | MB_ICONERROR,
        )
    };
}
