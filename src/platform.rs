pub mod shell;

use crate::dock::Rect;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::mem::size_of;
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{Com::*, LibraryLoader::GetModuleHandleW, SystemInformation::GetLocalTime},
    UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};

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

pub fn work_area() -> Result<(Rect, u32)> {
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
        let r = info.rcWork;
        if r.right <= r.left || r.bottom <= r.top || dpi == 0 {
            return Err(Error::new(E_FAIL, "显示器未提供有效的工作区或 DPI"));
        }
        Ok((
            Rect {
                x: r.left,
                y: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
            },
            dpi,
        ))
    }
}

impl Bar {
    pub fn position(&self, r: Rect) -> Result<()> {
        unsafe {
            SetWindowPos(
                self.0,
                Some(HWND_TOPMOST),
                r.x,
                r.y,
                r.width,
                r.height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        }
    }
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
    Menu(HWND),
    Scroll(i16),
    Quit,
}

thread_local! {
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static EVENTS: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
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
        m if m != 0 && m == TASKBAR_CREATED.get() => Event::Display,
        WM_TIMER | WM_TIMECHANGE => Event::Clock,
        WM_MOUSEMOVE => Event::Move(hwnd, x, y),
        windows::Win32::UI::Controls::WM_MOUSELEAVE => Event::Leave,
        WM_LBUTTONUP => Event::Click(hwnd, x, y),
        WM_CONTEXTMENU => Event::Menu(hwnd),
        WM_MOUSEWHEEL => Event::Scroll((wp.0 >> 16) as i16),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    };
    let wake = EVENTS.with_borrow_mut(|events| {
        let wake = events.is_empty();
        if events.back() != Some(&event) || matches!(event, Event::Click(..) | Event::Scroll(_)) {
            events.push_back(event);
        }
        wake
    });
    if wake {
        // A sent message can arrive while GetMessageW is waiting for a posted message.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
    }
    LRESULT(0)
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

pub fn popup_menu(owner: HWND, point: POINT, labels: &[String]) -> Result<Option<usize>> {
    struct Menu(HMENU);
    impl Drop for Menu {
        fn drop(&mut self) {
            let _ = unsafe { DestroyMenu(self.0) };
        }
    }
    unsafe {
        let menu = Menu(CreatePopupMenu()?);
        for (i, label) in labels.iter().enumerate() {
            AppendMenuW(
                menu.0,
                MF_STRING,
                i + 1,
                &HSTRING::from(label.replace('&', "&&")),
            )?;
        }
        let _ = SetForegroundWindow(owner);
        SetLastError(ERROR_SUCCESS);
        let selected = TrackPopupMenu(
            menu.0,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            owner,
            None,
        )
        .0;
        if selected == 0 && GetLastError() != ERROR_SUCCESS {
            return Err(Error::from_thread());
        }
        Ok((selected as usize).checked_sub(1))
    }
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
