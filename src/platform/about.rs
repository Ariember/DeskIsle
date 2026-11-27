//! Self-drawn About and update cards. Each keeps a caption style, so DWM gives
//! it a shadow, rounded corners and a place in Alt+Tab, while the client area covers the
//! frame and DeskIsle draws everything. It has no system menu, so DWM draws no caption
//! buttons over the acrylic backdrop that fills the whole card.

use super::{EVENTS, Event, queue, track_leave, window_work_area};
use crate::geometry::Rect;
use std::cell::Cell;
use std::thread::LocalKey;
use windows::Win32::{
    Foundation::{
        ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
    },
    Graphics::{
        Dwm::{
            DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmExtendFrameIntoClientArea,
            DwmSetWindowAttribute,
        },
        Gdi::{ScreenToClient, ValidateRect},
    },
    System::{
        Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        LibraryLoader::GetModuleHandleW,
    },
    UI::{
        Accessibility::{
            CLSID_AccPropServices, IAccPropServices, NotifyWinEvent, PROPID_ACC_DESCRIPTION,
            PROPID_ACC_VALUE,
        },
        Controls::{MARGINS, WM_MOUSELEAVE},
        Input::KeyboardAndMouse::{
            GetCapture, GetKeyState, ReleaseCapture, SetCapture, VIRTUAL_KEY, VK_C, VK_CONTROL,
            VK_DOWN, VK_ESCAPE, VK_F4, VK_RETURN, VK_SPACE, VK_TAB, VK_UP,
        },
        WindowsAndMessaging::*,
    },
};
use windows::core::{BOOL, Error, HRESULT, HSTRING, PCWSTR, Result, w};

const CLASS: PCWSTR = w!("DeskIsle.About");
const UPDATE_CLASS: PCWSTR = w!("DeskIsle.Update");

pub(super) fn register_class(instance: HINSTANCE, cursor: HCURSOR) -> Result<()> {
    let classes: [(PCWSTR, WNDPROC); 2] = [
        (CLASS, Some(window_proc::<false>)),
        (UPDATE_CLASS, Some(window_proc::<true>)),
    ];
    for (name, proc) in classes {
        let class = WNDCLASSW {
            lpfnWndProc: proc,
            hInstance: instance,
            lpszClassName: name,
            hCursor: cursor,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            let error = Error::from_thread();
            let mut registered = WNDCLASSW::default();
            if error.code() != HRESULT::from_win32(ERROR_CLASS_ALREADY_EXISTS.0)
                || unsafe { GetClassInfoW(Some(instance), name, &mut registered) }.is_err()
                || !registered
                    .lpfnWndProc
                    .zip(proc)
                    .is_some_and(|(a, b)| std::ptr::fn_addr_eq(a, b))
            {
                return Err(error);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AboutKey {
    Escape,
    Enter,
    Tab,
    Up,
    Down,
    Copy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AboutEvent {
    Move(i32, i32),
    Leave,
    Click(i32, i32),
    Scroll(i32),
    Key(AboutKey),
    /// The card settled at a new screen position.
    Moved,
    /// The card gained or lost activation.
    Activation(bool),
    /// The card moved to a monitor with this DPI and already took the suggested size.
    Dpi(u32),
    Close,
}

type Control = (i32, i32, i32, i32);
const EMPTY: Control = (0, 0, 0, 0);

#[derive(Default)]
struct CardInput {
    controls: Cell<[Control; 3]>,
    moving: Cell<bool>,
    pressed: Cell<Option<usize>>,
    wheel_remainder: Cell<i32>,
}

impl CardInput {
    fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.controls
            .get()
            .iter()
            .position(|&(l, t, r, b)| (l..r).contains(&x) && (t..b).contains(&y))
    }
}

thread_local! {
    // Bars owns at most one window of each kind. Callbacks can run reentrantly on its UI thread.
    static ABOUT_INPUT: CardInput = CardInput::default();
    static UPDATE_INPUT: CardInput = CardInput::default();
}

#[derive(Clone, Copy)]
enum CardKind {
    About,
    Update,
}

impl CardKind {
    fn input(self) -> &'static LocalKey<CardInput> {
        match self {
            Self::About => &ABOUT_INPUT,
            Self::Update => &UPDATE_INPUT,
        }
    }
}

struct CardFrame {
    backdrop: bool,
    kind: CardKind,
    accessibility: Option<IAccPropServices>,
}

pub struct AboutWindow(pub HWND, CardFrame);

impl AboutWindow {
    /// Creates the hidden window at `r`, in screen pixels. `controls` are the clickable
    /// areas, in client pixels. When `dwm_acrylic` is false, DeskIsle's live Liquid Glass
    /// compositor supplies the backdrop while DWM keeps the rounded frame and shadow.
    pub fn create(r: Rect, controls: [Control; 2], dark: bool, dwm_acrylic: bool) -> Result<Self> {
        Self::create_card(
            r,
            [controls[0], controls[1], EMPTY],
            dark,
            dwm_acrylic,
            CardKind::About,
            w!("关于本机"),
        )
    }

    pub fn create_update(r: Rect, controls: [Control; 3], dark: bool, title: &str) -> Result<Self> {
        let title = HSTRING::from(title);
        Self::create_card(
            r,
            controls,
            dark,
            false,
            CardKind::Update,
            PCWSTR(title.as_ptr()),
        )
    }

    fn create_card(
        r: Rect,
        controls: [Control; 3],
        dark: bool,
        dwm_acrylic: bool,
        kind: CardKind,
        title: PCWSTR,
    ) -> Result<Self> {
        kind.input().with(|state| state.controls.set(controls));
        let class = match kind {
            CardKind::Update => UPDATE_CLASS,
            CardKind::About => CLASS,
        };
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                class,
                title,
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
            let accessibility = match kind {
                CardKind::Update => {
                    CoCreateInstance(&CLSID_AccPropServices, None, CLSCTX_INPROC_SERVER).ok()
                }
                CardKind::About => None,
            };
            let window = Self(
                hwnd,
                CardFrame {
                    backdrop: glass,
                    kind,
                    accessibility,
                },
            );
            window.set_dark(dark);
            // Cosmetic only: builds without this attribute keep square corners.
            let round = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&raw const round).cast(),
                size_of_val(&round) as u32,
            );
            // Extend the frame only for DWM acrylic. With composition glass, even a
            // one-pixel extension introduces an opaque frame fill underneath clear glass.
            if glass {
                let margins = MARGINS {
                    cxLeftWidth: -1,
                    cxRightWidth: -1,
                    cyTopHeight: -1,
                    cyBottomHeight: -1,
                };
                let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            }
            Ok(window)
        }
    }

    /// Whether DWM draws the acrylic backdrop behind the card.
    pub fn backdrop(&self) -> bool {
        self.1.backdrop
    }

    /// Exposes the card's diagnostic text and selected action on the MSAA client object.
    pub fn describe_update(&self, description: &str, focus: &str) {
        if let Some(service) = &self.1.accessibility {
            let text = HSTRING::from(description);
            unsafe {
                let _ = service.SetHwndPropStr(
                    self.0,
                    OBJID_CLIENT.0 as u32,
                    0,
                    PROPID_ACC_DESCRIPTION,
                    PCWSTR(text.as_ptr()),
                );
            }
            self.focus_update(focus);
        }
    }

    pub fn focus_update(&self, focus: &str) {
        if let Some(service) = &self.1.accessibility {
            let text = HSTRING::from(focus);
            unsafe {
                if service
                    .SetHwndPropStr(
                        self.0,
                        OBJID_CLIENT.0 as u32,
                        0,
                        PROPID_ACC_VALUE,
                        PCWSTR(text.as_ptr()),
                    )
                    .is_ok()
                {
                    NotifyWinEvent(EVENT_OBJECT_VALUECHANGE, self.0, OBJID_CLIENT.0, 0);
                }
            }
        }
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
    pub fn resize(&self, width: i32, height: i32, controls: [Control; 2]) -> Result<()> {
        self.resize_card(width, height, [controls[0], controls[1], EMPTY])
    }

    pub fn resize_update(&self, width: i32, height: i32, controls: [Control; 3]) -> Result<()> {
        self.resize_card(width, height, controls)
    }

    fn resize_card(&self, width: i32, height: i32, controls: [Control; 3]) -> Result<()> {
        let mut r = RECT::default();
        unsafe {
            GetWindowRect(self.0, &mut r)?;
            let work = window_work_area(self.0)?;
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
            )?;
        }
        self.1
            .kind
            .input()
            .with(|state| state.controls.set(controls));
        Ok(())
    }
}

impl Drop for AboutWindow {
    fn drop(&mut self) {
        // Created and destroyed on the UI thread; drop input the window still queued.
        if let Some(service) = &self.1.accessibility {
            // HWND values can be reused. Remove both annotations while the HWND is still ours.
            let _ = unsafe {
                service.ClearHwndProps(
                    self.0,
                    OBJID_CLIENT.0 as u32,
                    0,
                    &[PROPID_ACC_DESCRIPTION, PROPID_ACC_VALUE],
                )
            };
        }
        let _ = unsafe { DestroyWindow(self.0) };
        EVENTS.with_borrow_mut(|events| {
            events.retain(|e| {
                !matches!(
                    (self.1.kind, e),
                    (CardKind::Update, Event::UpdatePrompt(_)) | (CardKind::About, Event::About(_))
                )
            })
        });
        self.1.kind.input().with(|state| {
            state.moving.set(false);
            state.pressed.set(None);
            state.wheel_remainder.set(0);
        });
    }
}

unsafe extern "system" fn window_proc<const UPDATE: bool>(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    let input = if UPDATE {
        CardKind::Update
    } else {
        CardKind::About
    }
    .input();
    let emit = |event| {
        queue(
            hwnd,
            if UPDATE {
                Event::UpdatePrompt(event)
            } else {
                Event::About(event)
            },
        )
    };
    let (x, y) = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
    let event = match msg {
        // The client area takes the whole window; DeskIsle draws the card itself.
        WM_NCCALCSIZE => return LRESULT(0),
        WM_ENTERSIZEMOVE => {
            input.with(|state| state.moving.set(true));
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        WM_EXITSIZEMOVE => {
            input.with(|state| state.moving.set(false));
            if unsafe { IsWindowVisible(hwnd) }.as_bool() {
                emit(AboutEvent::Moved);
            }
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        WM_MOVE
            if !input.with(|state| state.moving.get())
                && unsafe { IsWindowVisible(hwnd) }.as_bool() =>
        {
            AboutEvent::Moved
        }
        WM_ACTIVATE => {
            emit(AboutEvent::Activation(
                wp.0 & 0xffff != WA_INACTIVE as usize,
            ));
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        WM_NCHITTEST => {
            let mut point = POINT { x, y };
            let _ = unsafe { ScreenToClient(hwnd, &mut point) };
            let on_control = input.with(|state| state.hit(point.x, point.y).is_some());
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
        WM_LBUTTONDOWN if UPDATE => {
            input.with(|state| state.pressed.set(state.hit(x, y)));
            unsafe { SetCapture(hwnd) };
            return LRESULT(0);
        }
        WM_CAPTURECHANGED if UPDATE => {
            input.with(|state| state.pressed.set(None));
            return LRESULT(0);
        }
        WM_LBUTTONUP if UPDATE => {
            let clicked = input.with(|state| {
                state
                    .pressed
                    .take()
                    .is_some_and(|pressed| Some(pressed) == state.hit(x, y))
            });
            if unsafe { GetCapture() } == hwnd {
                let _ = unsafe { ReleaseCapture() };
            }
            if !clicked {
                return LRESULT(0);
            }
            AboutEvent::Click(x, y)
        }
        WM_LBUTTONUP => AboutEvent::Click(x, y),
        WM_MOUSEWHEEL if UPDATE => {
            let steps = input.with(|state| {
                let delta = i32::from((wp.0 >> 16) as i16) + state.wheel_remainder.get();
                let unit = WHEEL_DELTA as i32;
                state.wheel_remainder.set(delta % unit);
                delta / unit
            });
            if steps == 0 {
                return LRESULT(0);
            }
            AboutEvent::Scroll(steps)
        }
        WM_KEYDOWN => match VIRTUAL_KEY(wp.0 as u16) {
            VK_ESCAPE => AboutEvent::Key(AboutKey::Escape),
            VK_RETURN | VK_SPACE if UPDATE && lp.0 & (1 << 30) != 0 => return LRESULT(0),
            VK_RETURN | VK_SPACE => AboutEvent::Key(AboutKey::Enter),
            VK_TAB if UPDATE => AboutEvent::Key(AboutKey::Tab),
            VK_UP if UPDATE => AboutEvent::Key(AboutKey::Up),
            VK_DOWN if UPDATE => AboutEvent::Key(AboutKey::Down),
            VK_C if UPDATE && unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 => {
                AboutEvent::Key(AboutKey::Copy)
            }
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
    emit(event);
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            Variant::VARIANT,
        },
        UI::Accessibility::{AccessibleObjectFromWindow, IAccessible},
    };
    use windows::core::Interface;

    #[test]
    fn update_controls_keep_their_input_when_the_about_card_closes() -> Result<()> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        let result = check_hidden_cards();
        unsafe { CoUninitialize() };
        result
    }

    fn check_hidden_cards() -> Result<()> {
        let instance = unsafe { GetModuleHandleW(None)? }.into();
        register_class(instance, unsafe { LoadCursorW(None, IDC_ARROW)? })?;
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 280,
            height: 160,
        };
        let about = AboutWindow::create(bounds, [EMPTY; 2], false, false)?;
        let update = AboutWindow::create_update(
            bounds,
            [EMPTY, EMPTY, (20, 20, 50, 50)],
            false,
            "DeskIsle 更新",
        )?;
        update.describe_update("已下载并校验", "稍后再说");
        let accessible = unsafe {
            let mut raw = std::ptr::null_mut();
            AccessibleObjectFromWindow(
                update.0,
                OBJID_CLIENT.0 as u32,
                &IAccessible::IID,
                &mut raw,
            )?;
            IAccessible::from_raw(raw)
        };
        let client = VARIANT::from(0i32);
        assert_eq!(
            unsafe { accessible.get_accDescription(&client)? }.to_string(),
            "已下载并校验"
        );
        update.focus_update("安装并重启");
        assert_eq!(
            unsafe { accessible.get_accValue(&client)? }.to_string(),
            "安装并重启"
        );
        EVENTS.with_borrow_mut(|events| events.clear());
        unsafe {
            SendMessageW(about.0, WM_KEYDOWN, Some(WPARAM(VK_ESCAPE.0.into())), None);
            SendMessageW(update.0, WM_KEYDOWN, Some(WPARAM(VK_TAB.0.into())), None);
            let enter = Some(WPARAM(VK_RETURN.0.into()));
            SendMessageW(update.0, WM_KEYDOWN, enter, None);
            SendMessageW(update.0, WM_KEYDOWN, enter, Some(LPARAM(1 << 30)));
            let half_step = Some(WPARAM((WHEEL_DELTA as usize / 2) << 16));
            SendMessageW(update.0, WM_MOUSEWHEEL, half_step, None);
            SendMessageW(update.0, WM_MOUSEWHEEL, half_step, None);
            let button = Some(LPARAM(30 | (30 << 16)));
            let outside = Some(LPARAM(70 | (30 << 16)));
            SendMessageW(update.0, WM_LBUTTONDOWN, None, button);
            SendMessageW(update.0, WM_LBUTTONUP, None, outside);
            SendMessageW(update.0, WM_LBUTTONUP, None, button);
            SendMessageW(update.0, WM_LBUTTONDOWN, None, button);
            SendMessageW(update.0, WM_LBUTTONUP, None, button);
        }
        drop(about);
        EVENTS.with_borrow(|events| {
            assert!(events.iter().eq(&[
                Event::UpdatePrompt(AboutEvent::Key(AboutKey::Tab)),
                Event::UpdatePrompt(AboutEvent::Key(AboutKey::Enter)),
                Event::UpdatePrompt(AboutEvent::Scroll(1)),
                Event::UpdatePrompt(AboutEvent::Click(30, 30)),
            ]));
        });
        Ok(())
    }
}
