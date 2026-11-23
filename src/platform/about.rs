//! The About card's window. Like the settings panel it keeps a caption style, so DWM gives
//! it a shadow, rounded corners and a place in Alt+Tab, while the client area covers the
//! frame and DeskIsle draws everything. It has no system menu, so DWM draws no caption
//! buttons over the acrylic backdrop that fills the whole card.

use super::{EVENTS, Event, queue, to_rect, track_leave};
use crate::geometry::Rect;
use std::cell::Cell;
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmExtendFrameIntoClientArea,
            DwmSetWindowAttribute,
        },
        Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
            ScreenToClient, ValidateRect,
        },
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{MARGINS, WM_MOUSELEAVE},
        Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_ESCAPE, VK_F4, VK_RETURN, VK_SPACE},
        WindowsAndMessaging::*,
    },
};
use windows::core::{BOOL, Error, PCWSTR, Result, w};

const CLASS: PCWSTR = w!("DeskIsle.About");

pub(super) fn register_class(instance: HINSTANCE, cursor: HCURSOR) -> Result<()> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: CLASS,
        hCursor: cursor,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(Error::from_thread());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AboutKey {
    Escape,
    Enter,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AboutEvent {
    Move(i32, i32),
    Leave,
    Click(i32, i32),
    Key(AboutKey),
    /// The card moved to a monitor with this DPI and already took the suggested size.
    Dpi(u32),
    Close,
}

thread_local! {
    /// The close light and the button in client pixels. Everything else drags the card.
    static CONTROLS: Cell<[(i32, i32, i32, i32); 2]> = const { Cell::new([(0, 0, 0, 0); 2]) };
}

/// The card's window. The second field records whether DWM accepted the backdrop.
pub struct AboutWindow(pub HWND, bool);

impl AboutWindow {
    /// Creates the hidden window at `r`, in screen pixels. `controls` are the clickable
    /// areas, in client pixels. When `dwm_acrylic` is false, DeskIsle's live Liquid Glass
    /// compositor supplies the backdrop while DWM keeps the rounded frame and shadow.
    pub fn create(
        r: Rect,
        controls: [(i32, i32, i32, i32); 2],
        dark: bool,
        dwm_acrylic: bool,
    ) -> Result<Self> {
        CONTROLS.set(controls);
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                CLASS,
                w!("关于本机"),
                WS_POPUP | WS_CAPTION,
                r.x,
                r.y,
                r.width,
                r.height,
                None,
                None,
                Some(GetModuleHandleW(None)?.into()),
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
            // DWM draws the backdrop only where the frame extends into the client area.
            let edge = if glass { -1 } else { 1 };
            let margins = MARGINS {
                cxLeftWidth: edge,
                cxRightWidth: edge,
                cyTopHeight: edge,
                cyBottomHeight: edge,
            };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            Ok(window)
        }
    }

    /// Whether DWM draws the acrylic backdrop behind the card.
    pub fn backdrop(&self) -> bool {
        self.1
    }

    /// Matches the frame DWM draws around the card, and its backdrop's tint, to the palette.
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

    /// Shows the card, or brings it back to the front when it is already open.
    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.0, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(self.0);
        }
    }

    /// Resizes the card around its current center, kept inside its monitor's work area.
    pub fn resize(
        &self,
        width: i32,
        height: i32,
        controls: [(i32, i32, i32, i32); 2],
    ) -> Result<()> {
        CONTROLS.set(controls);
        let mut r = RECT::default();
        unsafe {
            GetWindowRect(self.0, &mut r)?;
            let monitor = MonitorFromWindow(self.0, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(monitor, &mut info).ok()?;
            let work = to_rect(info.rcWork);
            let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
            let x = (cx - width / 2).clamp(work.x, (work.x + work.width - width).max(work.x));
            let y = (cy - height / 2).clamp(work.y, (work.y + work.height - height).max(work.y));
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
}

impl Drop for AboutWindow {
    fn drop(&mut self) {
        // Created and destroyed on the UI thread; drop input the window still queued.
        let _ = unsafe { DestroyWindow(self.0) };
        EVENTS.with_borrow_mut(|events| events.retain(|e| !matches!(e, Event::About(_))));
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
    let event = match msg {
        // The client area takes the whole window; DeskIsle draws the card itself.
        WM_NCCALCSIZE => return LRESULT(0),
        WM_NCHITTEST => {
            let mut point = POINT { x, y };
            let _ = unsafe { ScreenToClient(hwnd, &mut point) };
            let on_control = CONTROLS
                .get()
                .iter()
                .any(|&(l, t, r, b)| (l..r).contains(&point.x) && (t..b).contains(&point.y));
            let hit = if on_control { HTCLIENT } else { HTCAPTION };
            return LRESULT(hit as isize);
        }
        // The card has a fixed size, so a double click on it must not zoom.
        WM_NCLBUTTONDBLCLK => return LRESULT(0),
        WM_PAINT => {
            // DirectComposition presents the content; nothing to paint here.
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let _ = track_leave(hwnd);
            AboutEvent::Move(x, y)
        }
        WM_MOUSELEAVE => AboutEvent::Leave,
        WM_LBUTTONUP => AboutEvent::Click(x, y),
        WM_KEYDOWN => match VIRTUAL_KEY(wp.0 as u16) {
            VK_ESCAPE => AboutEvent::Key(AboutKey::Escape),
            VK_RETURN | VK_SPACE => AboutEvent::Key(AboutKey::Enter),
            _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        },
        // Without a system menu Windows does not turn Alt+F4 into a close request.
        WM_SYSKEYDOWN if VIRTUAL_KEY(wp.0 as u16) == VK_F4 => AboutEvent::Close,
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
            AboutEvent::Dpi((wp.0 & 0xffff) as u32)
        }
        WM_CLOSE => AboutEvent::Close,
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    };
    queue(hwnd, Event::About(event));
    LRESULT(0)
}
