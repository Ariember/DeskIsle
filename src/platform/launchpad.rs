use super::{EVENTS, Event, queue, track_leave};
use crate::geometry::Rect;
use std::cell::Cell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::ValidateRect,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::WM_MOUSELEAVE,
        Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_BACK, VK_ESCAPE, VK_NEXT, VK_PRIOR, VK_RETURN},
        Shell::{DragFinish, DragQueryFileW, HDROP},
        WindowsAndMessaging::*,
    },
};
use windows::core::{Error, HSTRING, PCWSTR, Result, w};

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
    Move(i32, i32),
    Leave,
    Click(i32, i32),
    Context(i32, i32),
    Scroll(i16),
    Key(LaunchpadKey),
    Text(char),
    DropFiles(Vec<PathBuf>),
    Close,
}

pub struct LaunchpadWindow(pub HWND);

impl LaunchpadWindow {
    pub fn create(r: Rect) -> Result<Self> {
        // A regular Alt+Tab window lets the user switch back to Explorer for a file.
        // DirectComposition supplies the entire translucent client area.
        unsafe {
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
            .map(Self)
        }
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

pub fn confirm_remove(owner: HWND, path: &Path) -> bool {
    let name = path
        .file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let prompt = HSTRING::from(format!("从启动台移除“{name}”？原文件将保留。"));
    unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(prompt.as_ptr()),
            w!("启动台"),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        ) == IDYES
    }
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
        let _ = unsafe { DestroyWindow(self.0) };
        EVENTS.with_borrow_mut(|events| events.retain(|e| !matches!(e, Event::Launchpad(_))));
        HIGH_SURROGATE.set(None);
    }
}

fn dropped_paths(drop: HDROP) -> Vec<PathBuf> {
    // WM_DROPFILES transfers this handle to the receiving window. Copy the paths
    // before freeing it so no Shell-owned pointer leaves the window procedure.
    unsafe {
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut paths = Vec::new();
        for index in 0..count {
            let length = DragQueryFileW(drop, index, None);
            let mut buffer = vec![0; length as usize + 1];
            if length > 0 && DragQueryFileW(drop, index, Some(&mut buffer)) == length {
                paths.push(PathBuf::from(OsString::from_wide(
                    &buffer[..length as usize],
                )));
            }
        }
        DragFinish(drop);
        paths
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
            LaunchpadEvent::Move(x, y)
        }
        WM_MOUSELEAVE => LaunchpadEvent::Leave,
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
        WM_DROPFILES => LaunchpadEvent::DropFiles(dropped_paths(HDROP(wp.0 as _))),
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
    use windows::Win32::{
        Foundation::HGLOBAL,
        System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream},
        UI::Shell::DROPFILES,
    };

    #[test]
    fn window_receives_a_unicode_multi_file_drop() -> Result<()> {
        let paths = [r"C:\我的应用\编辑器.lnk", r"D:\Apps\Tool 🔧.exe"];
        unsafe {
            let instance = GetModuleHandleW(None)?.into();
            register_class(instance, LoadCursorW(None, IDC_ARROW)?)?;
            let window = LaunchpadWindow::create(Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            })?;
            assert_ne!(
                GetWindowLongW(window.0, GWL_EXSTYLE) as u32 & WS_EX_ACCEPTFILES.0,
                0
            );

            // A Shell drop contains this header followed by a double-NUL terminated
            // list of paths. Hand the backing HGLOBAL to the window just as Shell does.
            let header = DROPFILES {
                pFiles: size_of::<DROPFILES>() as u32,
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
            SendMessageW(
                window.0,
                WM_DROPFILES,
                Some(WPARAM(handle.0 as usize)),
                None,
            );
            let received = EVENTS.with_borrow_mut(|events| {
                events.drain(..).find_map(|event| match event {
                    Event::Launchpad(LaunchpadEvent::DropFiles(paths)) => Some(paths),
                    _ => None,
                })
            });
            assert_eq!(received, Some(paths.map(PathBuf::from).to_vec()));
        }
        Ok(())
    }
}
