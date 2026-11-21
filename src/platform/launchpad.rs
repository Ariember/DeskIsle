use super::{EVENTS, Event, MK_LBUTTON, drop_files, queue, track_leave};
use crate::geometry::Rect;
use std::cell::Cell;
use std::path::PathBuf;
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::ValidateRect,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::WM_MOUSELEAVE,
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, VIRTUAL_KEY, VK_BACK, VK_ESCAPE, VK_LBUTTON, VK_NEXT, VK_PRIOR,
            VK_RBUTTON, VK_RETURN,
        },
        Shell::HDROP,
        WindowsAndMessaging::*,
    },
};
use windows::core::{Error, PCWSTR, Result, w};

const CLASS: PCWSTR = w!("DeskIsle.Launchpad");

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
pub enum LaunchpadKey {
    Backspace,
    Enter,
    Escape,
    PageUp,
    PageDown,
}

#[derive(Clone, PartialEq, Eq)]
pub enum LaunchpadEvent {
    Move { x: i32, y: i32, left_down: bool },
    Leave,
    Press,
    Click(i32, i32),
    Context(i32, i32),
    Scroll(i16),
    Key(LaunchpadKey),
    Text(char),
    FileHover(bool),
    DropFiles(Vec<PathBuf>),
    Close,
}

pub struct LaunchpadWindow(pub HWND, Option<drop_files::Registration>);

impl LaunchpadWindow {
    pub fn contains_screen_point(&self, x: i32, y: i32) -> bool {
        unsafe { WindowFromPoint(POINT { x, y }) == self.0 }
    }

    pub fn create(r: Rect) -> Result<Self> {
        // A regular Alt+Tab window lets the user switch back to Explorer for a file.
        // DirectComposition supplies the entire translucent client area.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_ACCEPTFILES | WS_EX_NOREDIRECTIONBITMAP,
                CLASS,
                w!("DeskIsle 启动台"),
                WS_POPUP,
                r.x,
                r.y,
                r.width,
                r.height,
                None,
                None,
                Some(GetModuleHandleW(None)?.into()),
                None,
            )
        }?;
        let mut window = Self(hwnd, None);
        window.1 = Some(drop_files::Target::Launchpad.register(hwnd)?);
        Ok(window)
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.0, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(self.0);
        }
    }

    pub fn place(&self, r: Rect) -> Result<()> {
        unsafe {
            SetWindowPos(
                self.0,
                None,
                r.x,
                r.y,
                r.width,
                r.height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        }
    }
}

/// Whether the primary mouse button is held right now, as the hardware reports it.
/// A menu dismissed by this press has not yet seen its button go up.
pub fn primary_button_down() -> bool {
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    let button = if swapped { VK_RBUTTON } else { VK_LBUTTON };
    let state = unsafe { GetAsyncKeyState(i32::from(button.0)) };
    state < 0
}

// WM_CHAR delivers supplementary characters in two UTF-16 messages. There is one
// launchpad window on the UI thread, and focus loss discards any incomplete pair.
thread_local! {
    static HIGH_SURROGATE: Cell<Option<u16>> = const { Cell::new(None) };
}

fn input_char(unit: u16) -> Option<char> {
    let high = HIGH_SURROGATE.take();
    match unit {
        0xd800..=0xdbff => {
            HIGH_SURROGATE.set(Some(unit));
            None
        }
        0xdc00..=0xdfff => char::decode_utf16([high?, unit]).next()?.ok(),
        _ => char::from_u32(u32::from(unit)).filter(|c| !c.is_control()),
    }
}

impl Drop for LaunchpadWindow {
    fn drop(&mut self) {
        self.1.take();
        let _ = unsafe { DestroyWindow(self.0) };
        EVENTS.with_borrow_mut(|events| events.retain(|e| !matches!(e, Event::Launchpad(_))));
        HIGH_SURROGATE.set(None);
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
    let event = match msg {
        WM_PAINT => {
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let _ = track_leave(hwnd);
            LaunchpadEvent::Move {
                x,
                y,
                left_down: wp.0 & MK_LBUTTON != 0,
            }
        }
        WM_MOUSELEAVE => LaunchpadEvent::Leave,
        WM_LBUTTONDOWN => {
            queue(hwnd, Event::Launchpad(LaunchpadEvent::Press));
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        WM_LBUTTONUP => LaunchpadEvent::Click(x, y),
        WM_RBUTTONUP => LaunchpadEvent::Context(x, y),
        WM_MOUSEWHEEL => LaunchpadEvent::Scroll((wp.0 >> 16) as i16),
        WM_KEYDOWN => LaunchpadEvent::Key(match VIRTUAL_KEY(wp.0 as u16) {
            VK_BACK => LaunchpadKey::Backspace,
            VK_RETURN => LaunchpadKey::Enter,
            VK_ESCAPE => LaunchpadKey::Escape,
            VK_PRIOR => LaunchpadKey::PageUp,
            VK_NEXT => LaunchpadKey::PageDown,
            _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }),
        WM_CHAR => match input_char(wp.0 as u16) {
            Some(c) => LaunchpadEvent::Text(c),
            None => return LRESULT(0),
        },
        WM_KILLFOCUS => {
            HIGH_SURROGATE.set(None);
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        WM_DROPFILES => {
            let (_, paths) = drop_files::read(HDROP(wp.0 as _));
            LaunchpadEvent::DropFiles(paths)
        }
        WM_DPICHANGED => {
            // The app recomputes this window's bounds along with the primary bars.
            queue(hwnd, Event::Display);
            return LRESULT(0);
        }
        WM_CLOSE => LaunchpadEvent::Close,
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    };
    queue(hwnd, Event::Launchpad(event));
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::create_bars;
    use windows::Win32::{
        Foundation::{HGLOBAL, POINT},
        System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream},
        UI::Shell::DROPFILES,
    };

    fn send_drop(hwnd: HWND, point: POINT, paths: &[&str]) -> Result<()> {
        unsafe {
            // A Shell drop contains this header followed by a double-NUL terminated
            // list of paths. Hand the backing HGLOBAL to the window just as Shell does.
            let header = DROPFILES {
                pFiles: size_of::<DROPFILES>() as u32,
                pt: point,
                fWide: true.into(),
                ..Default::default()
            };
            let names: Vec<u16> = paths
                .iter()
                .flat_map(|path| path.encode_utf16().chain([0]))
                .chain([0])
                .collect();
            let stream = CreateStreamOnHGlobal(HGLOBAL::default(), false)?;
            stream
                .Write(
                    (&raw const header).cast(),
                    size_of_val(&header) as u32,
                    None,
                )
                .ok()?;
            stream
                .Write(
                    names.as_ptr().cast(),
                    size_of_val(names.as_slice()) as u32,
                    None,
                )
                .ok()?;
            let handle = GetHGlobalFromStream(&stream)?;
            drop(stream);
            SendMessageW(hwnd, WM_DROPFILES, Some(WPARAM(handle.0 as usize)), None);
        }
        Ok(())
    }

    #[test]
    fn launchpad_and_dock_receive_unicode_multi_file_drops() -> Result<()> {
        let paths = [r"C:\我的应用\编辑器.lnk", r"D:\Apps\Tool 🔧.exe"];
        let point = POINT { x: 67, y: 19 };
        let (top, dock) = create_bars()?;
        let pad = LaunchpadWindow::create(Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        })?;
        let accepts =
            |hwnd| unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_ACCEPTFILES.0 != 0 };
        assert!(!accepts(top.0));
        assert!(accepts(dock.0));
        assert!(accepts(pad.0));
        send_drop(pad.0, point, &paths)?;
        send_drop(dock.0, point, &paths)?;
        let paths = paths.map(PathBuf::from).to_vec();
        EVENTS.with_borrow_mut(|events| {
            let mut drops = events.drain(..).filter(|event| {
                matches!(
                    event,
                    Event::DropFiles(..) | Event::Launchpad(LaunchpadEvent::DropFiles(_))
                )
            });
            assert!(
                drops.next() == Some(Event::Launchpad(LaunchpadEvent::DropFiles(paths.clone())))
            );
            assert!(drops.next() == Some(Event::DropFiles(dock.0, point.x, point.y, paths)));
        });
        Ok(())
    }
}
