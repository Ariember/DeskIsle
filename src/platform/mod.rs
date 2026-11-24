pub mod about;
pub mod audio;
pub mod backdrop;
pub mod cards;
pub mod display;
mod drop_files;
pub mod input;
pub mod launchpad;
pub mod machine;
pub mod media;
pub mod metrics;
pub mod network;
pub mod power;
pub mod preview;
pub mod shell;
pub mod startup;
pub mod system_worker;
pub mod taskbar;
pub mod tray;
pub mod update;
pub mod wallpaper;

use crate::geometry::Rect;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::mem::size_of;
use std::path::PathBuf;
use windows::Win32::{
    Foundation::*,
    Graphics::{
        Dwm::{
            DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_EXTENDED_FRAME_BOUNDS,
            DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmExtendFrameIntoClientArea,
            DwmGetWindowAttribute, DwmSetWindowAttribute,
        },
        Gdi::*,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        SystemInformation::GetLocalTime,
        WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize},
    },
    UI::{
        Controls::MARGINS,
        HiDpi::*,
        Input::KeyboardAndMouse::*,
        Shell::{
            ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABN_FULLSCREENAPP,
            ABN_POSCHANGED, APPBARDATA, DragAcceptFiles, HDROP, SHAppBarMessage,
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
            RoInitialize(RO_INIT_SINGLETHREADED)?;
        }
        Ok(Self)
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

pub struct Bar(pub HWND, Option<drop_files::Registration>);

/// The registering UI thread owns the window and drops this binding before destroying it.
pub struct Shortcut {
    hwnd: HWND,
    id: i32,
}

const DOCK_SHORTCUT_ID: i32 = 3;

impl Shortcut {
    pub fn taskbar(hwnd: HWND, choice: i32) -> Result<Option<Self>> {
        let (modifiers, key, label) = match choice {
            0 => return Ok(None),
            1 => (MOD_CONTROL | MOD_ALT, VK_T, "Ctrl+Alt+T"),
            2 => (MOD_CONTROL | MOD_SHIFT, VK_F11, "Ctrl+Shift+F11"),
            _ => return Err(Error::new(E_INVALIDARG, "未知的任务栏快捷键")),
        };
        // A distinct ID per preset lets the old registration survive a conflict.
        Self::register(hwnd, choice, modifiers, key, label).map(Some)
    }

    pub fn dock(hwnd: HWND) -> Result<Self> {
        Self::register(
            hwnd,
            DOCK_SHORTCUT_ID,
            MOD_CONTROL | MOD_ALT,
            VK_D,
            "Ctrl+Alt+D",
        )
    }

    fn register(
        hwnd: HWND,
        id: i32,
        modifiers: HOT_KEY_MODIFIERS,
        key: VIRTUAL_KEY,
        label: &str,
    ) -> Result<Self> {
        unsafe { RegisterHotKey(Some(hwnd), id, modifiers | MOD_NOREPEAT, key.0 as u32) }.map_err(
            |error| Error::new(error.code(), format!("无法注册快捷键 {label}：{error}")),
        )?;
        Ok(Self { hwnd, id })
    }
}

impl Drop for Shortcut {
    fn drop(&mut self) {
        let _ = unsafe { UnregisterHotKey(Some(self.hwnd), self.id) };
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        // Both bars are created, used and destroyed on the same UI thread.
        self.1.take();
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
            style: CS_DBLCLKS,
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
            style: CS_DBLCLKS,
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
            style: WNDCLASS_STYLES::default(),
            lpfnWndProc: Some(panel_proc),
            lpszClassName: PANEL_CLASS,
            ..menu
        };
        if RegisterClassW(&panel) == 0 {
            return Err(Error::from_thread());
        }
        launchpad::register_class(instance, wc.hCursor)?;
        about::register_class(instance, wc.hCursor)?;
        preview::register_class(instance, wc.hCursor)?;
        cards::register_class(instance, wc.hCursor)?;
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
            .map(|hwnd| Bar(hwnd, None))
        };
        let top = create(w!("DeskIsle Status"))?;
        let mut bottom = create(w!("DeskIsle Dock"))?;
        bottom.1 = Some(drop_files::Target::Dock.register(bottom.0)?);
        DragAcceptFiles(bottom.0, true);
        Ok((top, bottom))
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
const METRICS_UPDATED: u32 = WM_APP + 6;
const CARDS_UPDATED: u32 = WM_APP + 7;
const TRAY_NOTICE_TIMER: usize = 2;
const DOCK_EDGE_TIMER: usize = 3;
const PREVIEW_LEAVE_TIMER: usize = 5;
const UPDATE_TIMER: usize = 6;
const MK_LBUTTON: usize = 0x0001;

/// Reset the one-shot status hint whenever another delivery fails.
pub fn time_tray_notice(hwnd: HWND) {
    // A clock tick also clears the notice if Windows cannot allocate a timer.
    let _ = unsafe { SetTimer(Some(hwnd), TRAY_NOTICE_TIMER, 5000, None) };
}

/// Wakes the UI thread. The worker holds the window's lifetime lock through this call.
pub fn notify_metrics(hwnd: HWND) -> Result<()> {
    unsafe { PostMessageW(Some(hwnd), METRICS_UPDATED, WPARAM(0), LPARAM(0)) }
}

pub fn notify_cards(hwnd: HWND) -> Result<()> {
    unsafe { PostMessageW(Some(hwnd), CARDS_UPDATED, WPARAM(0), LPARAM(0)) }
}

/// Poll only while a release check or a download is in flight.
pub fn update_timer(hwnd: HWND, enabled: bool) -> Result<()> {
    unsafe {
        if enabled {
            if SetTimer(Some(hwnd), UPDATE_TIMER, 300, None) == 0 {
                return Err(Error::from_thread());
            }
        } else {
            KillTimer(Some(hwnd), UPDATE_TIMER)?;
        }
    }
    Ok(())
}

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
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            explorer: None,
        }
    }

    fn shell_id(tray: HWND) -> (HWND, u32) {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(tray, Some(&mut pid)) };
        (tray, pid)
    }

    pub fn shell_changed(&self) -> bool {
        self.explorer != taskbar::explorer_tray().map(Self::shell_id)
    }

    fn register(&mut self, id: (HWND, u32)) -> Result<()> {
        if self.explorer == Some(id) {
            return Ok(());
        }
        let mut data = self.data();
        if unsafe { SHAppBarMessage(ABM_NEW, &mut data) } == 0 {
            return Err(Error::new(E_FAIL, "Explorer 未注册顶部状态栏"));
        }
        self.explorer = Some(id);
        Ok(())
    }

    /// Returns the strip Explorer granted along the top edge.
    pub fn place(&mut self, bounds: Rect, height: i32) -> Result<Rect> {
        let requested = RECT {
            left: bounds.x,
            top: bounds.y,
            right: bounds.x + bounds.width,
            bottom: bounds.y + height,
        };
        // Explorer may be between instances at logon or during a restart.
        // TaskbarCreated will negotiate the actual reservation once it is ready.
        let Some(tray) = taskbar::explorer_tray() else {
            self.explorer = None;
            return Ok(to_rect(requested));
        };
        let id = Self::shell_id(tray);
        match self.reserve(id, requested, height) {
            Err(_) if taskbar::explorer_tray().map(Self::shell_id) != Some(id) => {
                self.explorer = None;
                Ok(to_rect(requested))
            }
            result => result,
        }
    }

    fn reserve(&mut self, id: (HWND, u32), requested: RECT, height: i32) -> Result<Rect> {
        self.register(id)?;
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

/// Applies Windows' desktop acrylic to an owned UI window when the system supports it.
pub fn set_window_backdrop(hwnd: HWND, requested: bool, dark: bool) -> bool {
    set_backdrop_region(hwnd, requested, dark, -1)
}

pub fn set_backdrop_region(hwnd: HWND, requested: bool, dark: bool, height: i32) -> bool {
    let material = if requested {
        DWMSBT_TRANSIENTWINDOW
    } else {
        DWMSBT_NONE
    };
    let set_material = |material: windows::Win32::Graphics::Dwm::DWM_SYSTEMBACKDROP_TYPE| unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&raw const material).cast(),
            size_of_val(&material) as u32,
        )
    };
    if set_material(material).is_err() {
        return false;
    }
    let margins = MARGINS {
        cxLeftWidth: if requested && height < 0 { -1 } else { 0 },
        cyTopHeight: if requested { height.max(0) } else { 0 },
        ..Default::default()
    };
    if unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) }.is_err() {
        if requested {
            let _ = set_material(DWMSBT_NONE);
        }
        return false;
    }
    if requested {
        let dark = BOOL::from(dark);
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark).cast(),
                size_of_val(&dark) as u32,
            )
        };
    }
    requested
}

impl Bar {
    pub fn set_top_backdrop(&self, requested: bool, dark: bool) -> bool {
        set_window_backdrop(self.0, requested, dark)
    }

    pub fn capture(&self) {
        unsafe { SetCapture(self.0) };
    }

    pub fn release(&self) {
        // Cancellation can arrive after a different window took capture.
        if unsafe { GetCapture() } == self.0 {
            RELEASING_CAPTURE.set(true);
            let _ = unsafe { ReleaseCapture() };
            RELEASING_CAPTURE.set(false);
        }
    }

    pub fn position(&self, r: Rect, visible: bool) -> Result<()> {
        self.position_layer(r, visible, true)
    }

    pub fn position_layer(&self, r: Rect, visible: bool, topmost: bool) -> Result<()> {
        let show = if visible {
            SWP_SHOWWINDOW
        } else {
            SWP_HIDEWINDOW
        };
        let place = |after, order| unsafe {
            SetWindowPos(
                self.0,
                after,
                r.x,
                r.y,
                r.width,
                r.height,
                SWP_NOACTIVATE | order | show,
            )
        };
        if topmost {
            return place(Some(HWND_TOPMOST), SET_WINDOW_POS_FLAGS(0));
        }
        let is_topmost = |hwnd| {
            WINDOW_EX_STYLE(unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32)
                .contains(WS_EX_TOPMOST)
        };
        if !is_topmost(self.0) {
            return place(None, SWP_NOZORDER);
        }
        // Inserting after an ordinary foreground window demotes the Dock while
        // keeping it above Explorer's desktop. The borrowed HWND can disappear
        // or change bands between the query and SetWindowPos.
        let front = foreground();
        if !front.is_invalid() && !is_desktop(front) && !is_topmost(front) {
            match place(Some(front), SET_WINDOW_POS_FLAGS(0)) {
                Ok(()) if !is_topmost(self.0) => return Ok(()),
                Err(error) if unsafe { IsWindow(Some(front)) }.as_bool() => return Err(error),
                _ => {}
            }
        }
        place(Some(HWND_NOTOPMOST), SET_WINDOW_POS_FLAGS(0))
    }

    /// Schedule a one-shot pointer check at the reveal edge.
    pub fn time_dock_edge(&self, delay_ms: u32) -> Result<()> {
        if unsafe { SetTimer(Some(self.0), DOCK_EDGE_TIMER, delay_ms, None) } == 0 {
            return Err(Error::from_thread());
        }
        Ok(())
    }

    pub fn cancel_dock_edge_timer(&self) {
        let _ = unsafe { KillTimer(Some(self.0), DOCK_EDGE_TIMER) };
    }

    /// Schedule a one-shot pointer check while crossing between the Dock and its preview.
    pub fn time_preview_leave(&self, delay_ms: u32) -> Result<()> {
        if unsafe { SetTimer(Some(self.0), PREVIEW_LEAVE_TIMER, delay_ms, None) } == 0 {
            return Err(Error::from_thread());
        }
        Ok(())
    }

    pub fn cancel_preview_leave_timer(&self) {
        let _ = unsafe { KillTimer(Some(self.0), PREVIEW_LEAVE_TIMER) };
    }

    pub fn contains_screen_point(&self, x: i32, y: i32) -> bool {
        unsafe { WindowFromPoint(POINT { x, y }) == self.0 }
    }
}

pub fn drag_threshold(dpi: u32) -> (i32, i32) {
    unsafe {
        (
            GetSystemMetricsForDpi(SM_CXDRAG, dpi)
                .saturating_abs()
                .max(1),
            GetSystemMetricsForDpi(SM_CYDRAG, dpi)
                .saturating_abs()
                .max(1),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Down,
    Up,
    Left,
    Right,
    Home,
    End,
    /// Page Up and Page Down turn the calendar's month.
    PageUp,
    PageDown,
    Select,
    /// The menu key or Shift+F10 asks for the focused row's own menu.
    Context,
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

#[derive(Clone, PartialEq, Eq)]
pub enum Event {
    Paint,
    Refresh,
    Display,
    Clock,
    TrayNoticeExpired,
    InputLanguage,
    DockEdgeTimer,
    PreviewLeaveTimer,
    TaskbarShortcut(i32),
    DockShortcut,
    MetricsUpdate,
    CardsUpdated,
    UpdateProgress,
    Move(HWND, i32, i32),
    Leave(HWND),
    CancelPress(HWND),
    DockFileHover(Option<(i32, i32)>),
    /// Shell files and the actual drop point in client pixels.
    DropFiles(HWND, i32, i32, Vec<PathBuf>),
    Click(HWND, i32, i32),
    RightClick(HWND, i32, i32),
    /// The middle button was released over a bar, in its client pixels.
    MiddleClick(HWND, i32, i32),
    Menu(HWND),
    /// A mouse button went down on a bar (`true` for the right button).
    Press {
        owner: HWND,
        x: i32,
        y: i32,
        right: bool,
        double: bool,
    },
    /// Pointer input in the open menu window, in its client pixels.
    MenuMove(i32, i32),
    /// A button went down in the menu, in client pixels.
    MenuPress {
        x: i32,
        y: i32,
        right: bool,
        double: bool,
    },
    MenuLeave,
    /// The left button was released in the menu.
    MenuPick(i32, i32),
    /// The right button was released in the menu.
    MenuContext(i32, i32),
    /// A wheel turn of `delta` (120 per notch) over the menu.
    MenuScroll(i16),
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
    TrayRecovery,
    /// The default output's volume or mute changed.
    Audio,
    /// The worker completed a network or media request.
    SystemUpdate,
    /// Windows selected a different default playback endpoint (possibly none).
    DefaultOutput,
    /// The power source or battery charge changed.
    Power,
    /// A wheel turn of `delta` (120 per notch) over a bar, at its client pixels.
    Scroll(HWND, i16, i32, i32),
    /// Pointer input in the settings panel, in its client pixels.
    PanelMove(i32, i32),
    PanelLeave,
    PanelPress(i32, i32),
    PanelCancel,
    PanelClick(i32, i32),
    PanelScroll(i16),
    PanelKey(PanelKey),
    /// The panel moved to a monitor with this DPI and already took the suggested size.
    PanelDpi,
    PanelClose,
    Launchpad(launchpad::LaunchpadEvent),
    About(about::AboutEvent),
    Preview(preview::PreviewEvent),
    CardPage(i32),
    Quit,
}

impl Event {
    fn for_menu(&self) -> bool {
        matches!(
            self,
            Event::MenuMove(..)
                | Event::MenuPress { .. }
                | Event::MenuLeave
                | Event::MenuPick(..)
                | Event::MenuContext(..)
                | Event::MenuScroll(_)
                | Event::MenuKey(_)
                | Event::MenuDismiss
        )
    }

    fn for_panel(&self) -> bool {
        matches!(
            self,
            Event::PanelMove(..)
                | Event::PanelPress(..)
                | Event::PanelCancel
                | Event::PanelLeave
                | Event::PanelClick(..)
                | Event::PanelScroll(_)
                | Event::PanelKey(_)
                | Event::PanelDpi
                | Event::PanelClose
        )
    }
}

thread_local! {
    /// ReleaseCapture synchronously sends WM_CAPTURECHANGED. The application
    /// already finished that press and may start a new one before reading events.
    static RELEASING_CAPTURE: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static EVENTS: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    /// The settings panel's caption height and the left and right edges of its close
    /// button, in client pixels, for hit testing inside its window procedure.
    static PANEL_CAPTION: Cell<(i32, i32, i32)> = const { Cell::new((0, 0, 0)) };
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
        WM_HOTKEY if matches!(wp.0, 1 | 2) => Event::TaskbarShortcut(wp.0 as i32),
        WM_HOTKEY if wp.0 == DOCK_SHORTCUT_ID as usize => Event::DockShortcut,
        shell::WINDOWS_CHANGED => Event::Refresh,
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => Event::Display,
        WM_POWERBROADCAST if wp.0 == PBT_APMRESUMEAUTOMATIC as usize => Event::Display,
        WM_POWERBROADCAST if wp.0 == PBT_APMPOWERSTATUSCHANGE as usize => Event::Power,
        audio::AUDIO_CHANGED if wp.0 != 0 => Event::DefaultOutput,
        audio::AUDIO_CHANGED => Event::Audio,
        system_worker::UPDATED => Event::SystemUpdate,
        METRICS_UPDATED => Event::MetricsUpdate,
        CARDS_UPDATED => Event::CardsUpdated,
        m if m != 0 && m == TASKBAR_CREATED.get() => Event::ShellRestart,
        APPBAR_CHANGED if wp.0 == ABN_POSCHANGED as usize => Event::AppBarMoved,
        APPBAR_CHANGED if wp.0 == ABN_FULLSCREENAPP as usize => Event::FullScreen(lp.0 != 0),
        APPBAR_CHANGED => return LRESULT(0),
        WM_TIMER if wp.0 == TRAY_NOTICE_TIMER => {
            let _ = unsafe { KillTimer(Some(hwnd), TRAY_NOTICE_TIMER) };
            Event::TrayNoticeExpired
        }
        WM_TIMER if wp.0 == DOCK_EDGE_TIMER => {
            let _ = unsafe { KillTimer(Some(hwnd), DOCK_EDGE_TIMER) };
            Event::DockEdgeTimer
        }
        WM_TIMER if wp.0 == PREVIEW_LEAVE_TIMER => {
            let _ = unsafe { KillTimer(Some(hwnd), PREVIEW_LEAVE_TIMER) };
            Event::PreviewLeaveTimer
        }
        WM_TIMER if wp.0 == input::TIMER => Event::InputLanguage,
        WM_TIMER if wp.0 == UPDATE_TIMER => Event::UpdateProgress,
        WM_TIMER | WM_TIMECHANGE => Event::Clock,
        WM_MOUSEMOVE => {
            // A non-activating bar may miss a release outside its visible area.
            if wp.0 & MK_LBUTTON == 0 && unsafe { GetCapture() } == hwnd {
                queue(hwnd, Event::CancelPress(hwnd));
            }
            Event::Move(hwnd, x, y)
        }
        WM_CAPTURECHANGED if !RELEASING_CAPTURE.get() => Event::CancelPress(hwnd),
        WM_CANCELMODE => {
            queue(hwnd, Event::CancelPress(hwnd));
            // Default processing releases this window's capture.
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        WM_DROPFILES => {
            let (point, paths) = drop_files::read(HDROP(wp.0 as _));
            let Some(point) = point else {
                return LRESULT(0);
            };
            Event::DropFiles(hwnd, point.x, point.y, paths)
        }
        windows::Win32::UI::Controls::WM_MOUSELEAVE => Event::Leave(hwnd),
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK => Event::Press {
            owner: hwnd,
            x,
            y,
            right: matches!(msg, WM_RBUTTONDOWN | WM_RBUTTONDBLCLK),
            double: matches!(msg, WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK),
        },
        WM_LBUTTONUP => Event::Click(hwnd, x, y),
        WM_RBUTTONUP => Event::RightClick(hwnd, x, y),
        WM_MBUTTONUP => Event::MiddleClick(hwnd, x, y),
        WM_CONTEXTMENU => Event::Menu(hwnd),
        WM_MOUSEWHEEL => {
            // Wheel messages carry screen coordinates.
            let mut point = POINT { x, y };
            let _ = unsafe { ScreenToClient(hwnd, &mut point) };
            Event::Scroll(hwnd, (wp.0 >> 16) as i16, point.x, point.y)
        }
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
                    | Event::DropFiles(..)
                    | Event::SystemUpdate
                    | Event::RightClick(..)
                    | Event::MiddleClick(..)
                    | Event::Press { .. }
                    | Event::Scroll(..)
                    | Event::MenuPick(..)
                    | Event::MenuContext(..)
                    | Event::MenuPress { .. }
                    | Event::MenuScroll(_)
                    | Event::MenuKey(_)
                    | Event::PanelClick(..)
                    | Event::PanelScroll(_)
                    | Event::PanelKey(_)
                    | Event::Launchpad(_)
                    | Event::About(_)
                    | Event::Preview(_)
                    | Event::CardPage(_)
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

pub fn local_hour() -> u16 {
    unsafe { GetLocalTime() }.wHour
}

/// HWND belongs to a live DeskIsle window on the calling UI thread.
pub fn window_dpi(hwnd: HWND) -> u32 {
    unsafe { GetDpiForWindow(hwnd) }
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

/// A click-through label owned by the Dock. STATIC needs no additional window procedure.
pub struct LabelWindow(pub HWND);

impl LabelWindow {
    pub fn create(owner: HWND, rect: Rect) -> Result<Self> {
        unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOPMOST
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOREDIRECTIONBITMAP,
                w!("STATIC"),
                None,
                WS_POPUP,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                Some(owner),
                None,
                Some(GetModuleHandleW(None)?.into()),
                None,
            )
            .map(Self)
        }
    }
    pub fn show(&self) {
        let _ = unsafe { ShowWindow(self.0, SW_SHOWNOACTIVATE) };
    }
}
impl Drop for LabelWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

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
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK => Event::MenuPress {
            x,
            y,
            right: matches!(msg, WM_RBUTTONDOWN | WM_RBUTTONDBLCLK),
            double: matches!(msg, WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK),
        },
        WM_LBUTTONUP => Event::MenuPick(x, y),
        WM_RBUTTONUP => Event::MenuContext(x, y),
        WM_MOUSEWHEEL => Event::MenuScroll((wp.0 >> 16) as i16),
        WM_KEYDOWN | WM_SYSKEYDOWN => Event::MenuKey(match VIRTUAL_KEY(wp.0 as u16) {
            VK_DOWN => MenuKey::Down,
            VK_UP => MenuKey::Up,
            VK_TAB if (unsafe { GetKeyState(VK_SHIFT.0 as i32) }) < 0 => MenuKey::Up,
            VK_TAB => MenuKey::Down,
            VK_LEFT => MenuKey::Left,
            VK_RIGHT => MenuKey::Right,
            VK_HOME => MenuKey::Home,
            VK_END => MenuKey::End,
            VK_PRIOR => MenuKey::PageUp,
            VK_NEXT => MenuKey::PageDown,
            VK_RETURN | VK_SPACE => MenuKey::Select,
            VK_APPS => MenuKey::Context,
            VK_F10 if (unsafe { GetKeyState(VK_SHIFT.0 as i32) }) < 0 => MenuKey::Context,
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
/// The second field records whether DWM accepted the acrylic backdrop for the sidebar.
pub struct PanelWindow(pub HWND, bool);

impl PanelWindow {
    pub fn capture(&self) {
        unsafe { SetCapture(self.0) };
    }

    pub fn release(&self) {
        if unsafe { GetCapture() } == self.0 {
            RELEASING_CAPTURE.set(true);
            let _ = unsafe { ReleaseCapture() };
            RELEASING_CAPTURE.set(false);
        }
    }

    /// Creates the hidden window at `r`, in screen pixels. `caption` is the draggable
    /// height and the left and right edges of the close button, and `sidebar` the
    /// sidebar's width, in client pixels. When `dwm_acrylic` is false, DeskIsle's live
    /// Liquid Glass compositor supplies the sidebar backdrop and DWM only frames the window.
    pub fn create(
        r: Rect,
        caption: (i32, i32, i32),
        sidebar: i32,
        dark: bool,
        dwm_acrylic: bool,
    ) -> Result<Self> {
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
            let backdrop = DWMSBT_TRANSIENTWINDOW;
            let glass = dwm_acrylic
                && DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    (&raw const backdrop).cast(),
                    size_of_val(&backdrop) as u32,
                )
                .is_ok();
            let window = Self(hwnd, glass);
            window.set_dark(dark);
            // Cosmetic only: builds without this attribute keep square corners.
            let round = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&raw const round).cast(),
                size_of_val(&round) as u32,
            );
            window.extend_frame(sidebar);
            Ok(window)
        }
    }

    /// Whether DWM draws the acrylic backdrop under the sidebar. With transparency
    /// effects off, DWM shows a flat fill there instead.
    pub fn backdrop(&self) -> bool {
        self.1
    }

    /// DWM draws the backdrop only where the frame extends into the client area. It
    /// extends under the sidebar alone: across the whole window DWM would also draw its
    /// own caption buttons over the top right corner.
    fn extend_frame(&self, sidebar: i32) {
        let margins = MARGINS {
            cxLeftWidth: if self.1 { sidebar.max(1) } else { 1 },
            cxRightWidth: 1,
            cyTopHeight: 1,
            cyBottomHeight: 1,
        };
        // Cosmetic only: without it the window keeps the DWM frame's default look.
        let _ = unsafe { DwmExtendFrameIntoClientArea(self.0, &margins) };
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
    pub fn resize(
        &self,
        width: i32,
        height: i32,
        work: Rect,
        caption: (i32, i32, i32),
        sidebar: i32,
    ) -> Result<()> {
        PANEL_CAPTION.set(caption);
        self.extend_frame(sidebar);
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
        // Both forms must keep the window rectangle: CreateWindowEx sends the one without
        // `wParam`, and the system caption would stay until the next frame change.
        WM_NCCALCSIZE => return LRESULT(0),
        WM_NCHITTEST => {
            let mut point = POINT { x, y };
            let _ = unsafe { ScreenToClient(hwnd, &mut point) };
            let (caption, close_left, close_right) = PANEL_CAPTION.get();
            let on_close = (close_left..close_right).contains(&point.x);
            let hit = if (0..caption).contains(&point.y) && !on_close {
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
        WM_LBUTTONDOWN => Event::PanelPress(x, y),
        WM_LBUTTONUP => Event::PanelClick(x, y),
        WM_CAPTURECHANGED if !RELEASING_CAPTURE.get() => Event::PanelCancel,
        WM_CANCELMODE => {
            queue(hwnd, Event::PanelCancel);
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
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
            Event::PanelDpi
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

pub fn foreground_is_desktop() -> bool {
    is_desktop(foreground())
}

fn is_desktop(hwnd: HWND) -> bool {
    if hwnd.is_invalid() {
        return false;
    }
    let shell = unsafe { GetShellWindow() };
    if hwnd == shell || hwnd == unsafe { GetDesktopWindow() } {
        return true;
    }
    // Explorer can host the desktop view in a WorkerW instead of Progman.
    let mut class = [0u16; 32];
    let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
    class[..len].iter().copied().eq("WorkerW".encode_utf16())
        && !shell.is_invalid()
        && shell::window_process(hwnd) == shell::window_process(shell)
}

pub fn overlapping_window_frame(hwnd: HWND, dock: Rect) -> Option<Rect> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
            || WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32)
                .contains(WS_EX_TOOLWINDOW)
            || is_desktop(hwnd)
        {
            return None;
        }
        // GetWindowRect includes invisible resize borders. Prefer the frame that
        // the compositor actually draws, including for partially overlapping windows.
        let mut frame = RECT::default();
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut frame).cast(),
            size_of::<RECT>() as u32,
        )
        .is_err()
            && GetWindowRect(hwnd, &mut frame).is_err()
        {
            return None;
        }
        (frame.left < dock.x + dock.width
            && frame.right > dock.x
            && frame.top < dock.y + dock.height
            && frame.bottom > dock.y)
            .then_some(Rect {
                x: frame.left,
                y: frame.top,
                width: frame.right - frame.left,
                height: frame.bottom - frame.top,
            })
    }
}

pub fn maximized_on_primary(hwnd: HWND) -> bool {
    unsafe {
        IsZoomed(hwnd).as_bool()
            && MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL)
                == MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY)
    }
}

/// Reads one visible pixel just inside the application's top edge. Custom title bars
/// often paint their own color. A popup covering the sample must not supply its tint.
pub fn top_window_color(id: isize, bar: Rect, dpi: u32) -> Option<u32> {
    let window = HWND(id as _);
    let point = POINT {
        x: bar.x + bar.width / 2,
        y: bar.y + bar.height + (3 * dpi as i32 / 96).max(1),
    };
    unsafe {
        if GetAncestor(WindowFromPoint(point), GA_ROOT) != window {
            return None;
        }
        let dc = GetDC(None);
        if dc.is_invalid() {
            return None;
        }
        let color = GetPixel(dc, point.x, point.y);
        ReleaseDC(None, dc);
        (color.0 != u32::MAX).then_some(color.0)
    }
}

pub fn confirm_power(owner: HWND, restart: bool) -> Result<bool> {
    let message = if restart {
        w!("确定要重新启动计算机吗？")
    } else {
        w!("确定要关闭计算机吗？")
    };
    let answer = unsafe {
        MessageBoxW(
            Some(owner),
            message,
            w!("DeskIsle"),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        )
    };
    if answer.0 == 0 {
        Err(Error::from_thread())
    } else {
        Ok(answer == IDYES)
    }
}

pub fn confirm_update(owner: HWND, version: &str) -> Result<bool> {
    let text = HSTRING::from(format!(
        "发现 DeskIsle {version}（当前版本 {}）。现在下载并重启 DeskIsle 吗？",
        env!("CARGO_PKG_VERSION")
    ));
    let answer = unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(text.as_ptr()),
            w!("DeskIsle 更新"),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        )
    };
    if answer.0 == 0 {
        Err(Error::from_thread())
    } else {
        Ok(answer == IDYES)
    }
}

pub fn show_update_status(owner: HWND, message: &str, failed: bool) {
    let text = HSTRING::from(message);
    let icon = if failed {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(text.as_ptr()),
            w!("DeskIsle 更新"),
            MB_OK | icon,
        )
    };
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

/// Today's local date: year, month and day.
pub fn today() -> (i32, u8, u8) {
    let now = unsafe { GetLocalTime() };
    (i32::from(now.wYear), now.wMonth as u8, now.wDay as u8)
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
