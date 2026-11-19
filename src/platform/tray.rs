//! Tray host for the status bar.
//!
//! Windows 11 offers no API to read the notification area. `Shell_NotifyIcon` sends a
//! `WM_COPYDATA` to the first `Shell_TrayWnd` that `FindWindow` returns, so DeskIsle owns
//! a hidden window of that class above Explorer's taskbar in the Z order, records the
//! icons it receives and forwards every message to Explorer. The system tray keeps
//! working while DeskIsle runs and after it exits.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::path::PathBuf;

use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, GUID, Result, w};

use super::taskbar::explorer_tray;
use super::{Event, TASKBAR_CREATED, queue, shell};

// shellapi.h. Shell32 sends the 32-bit layout to the tray from 32- and 64-bit callers.
const TRAY_SIGNATURE: u32 = 0x3475_3423;
const COPYDATA_NOTIFY: usize = 1;
const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETVERSION: u32 = 4;
const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_STATE: u32 = 0x08;
const NIF_GUID: u32 = 0x20;
const NIS_HIDDEN: u32 = 0x01;
const NIN_SELECT: u32 = WM_USER;
const HWND_BROADCAST: HWND = HWND(0xffff as _);
const FORWARD_TIMEOUT_MS: u32 = 3000;

#[repr(C)]
#[derive(Clone, Copy)]
struct NotifyData32 {
    size: u32,
    hwnd: u32,
    id: u32,
    flags: u32,
    callback: u32,
    icon: u32,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: GUID,
    balloon_icon: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TrayMessage {
    signature: u32,
    message: u32,
    data: NotifyData32,
}

/// The fields every caller since Windows 2000 fills in.
const MIN_TRAY_MESSAGE: usize = offset_of!(TrayMessage, data) + offset_of!(NotifyData32, tip);

struct OwnedIcon(HICON);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // Copied with CopyIcon on this thread, so this process owns the handle.
        let _ = unsafe { DestroyIcon(self.0) };
    }
}

struct Notify {
    message: u32,
    owner: isize,
    id: u32,
    flags: u32,
    callback: u32,
    icon: Option<OwnedIcon>,
    tip: String,
    state: u32,
    state_mask: u32,
    version: u32,
    guid: GUID,
}

thread_local! {
    // Filled by the host window procedure and drained by the UI loop on the same thread.
    static COMMANDS: RefCell<Vec<Notify>> = const { RefCell::new(Vec::new()) };
}

pub struct TrayIcon {
    /// Stable across runs: executable path plus the icon's GUID or ID.
    pub key: String,
    pub label: String,
    /// Changes whenever the image or tooltip changes.
    pub stamp: u64,
    icon: Option<OwnedIcon>,
    owner: isize,
    id: u32,
    guid: Option<GUID>,
    callback: u32,
    version: u32,
    state: u32,
}

impl TrayIcon {
    /// Owned by the tray host; valid until the next [`TrayHost::update`].
    pub fn icon(&self) -> Option<HICON> {
        self.icon.as_ref().map(|icon| icon.0)
    }

    /// Applications hide an icon with `NIS_HIDDEN` while keeping it registered.
    pub fn visible(&self) -> bool {
        self.state & NIS_HIDDEN == 0
    }

    fn matches(&self, n: &Notify) -> bool {
        match (n.flags & NIF_GUID != 0, self.guid) {
            (true, Some(guid)) => guid == n.guid,
            (true, None) => false,
            (false, _) => self.guid.is_none() && self.owner == n.owner && self.id == n.id,
        }
    }
}

pub struct TrayHost {
    hwnd: HWND,
    icons: Vec<TrayIcon>,
    next_stamp: u64,
}

impl TrayHost {
    /// Call on the UI thread after `create_bars` registered the TaskbarCreated message.
    pub fn new() -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?.into();
            let class = w!("Shell_TrayWnd");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(host_proc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err(Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class,
                w!(""),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )?;
            let host = Self {
                hwnd,
                icons: Vec::new(),
                next_stamp: 0,
            };
            host.raise();
            // Applications add their icons again on TaskbarCreated, now through this host.
            // UIPI keeps the broadcast from elevated processes; their icons appear on their
            // next update.
            SendNotifyMessageW(HWND_BROADCAST, TASKBAR_CREATED.get(), WPARAM(0), LPARAM(0))?;
            Ok(host)
        }
    }

    /// Keeps this host first for `FindWindow`. Explorer raises its taskbar when the user
    /// clicks it, so the UI loop calls this after foreground changes. Mirroring the
    /// taskbar rectangle keeps applications that size popups from `Shell_TrayWnd` right.
    pub fn raise(&self) {
        let mut rect = RECT::default();
        if let Some(explorer) = explorer_tray() {
            let _ = unsafe { GetWindowRect(explorer, &mut rect) };
        }
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        };
    }

    pub fn icons(&self) -> &[TrayIcon] {
        &self.icons
    }

    /// Applies queued notifications and drops icons whose owner window closed.
    /// Returns whether the visible tray changed.
    pub fn update(&mut self) -> bool {
        let commands = COMMANDS.take();
        let mut changed = false;
        for command in commands {
            changed |= self.apply(command);
        }
        let before = self.icons.len();
        self.icons
            .retain(|icon| unsafe { IsWindow(Some(HWND(icon.owner as _))) }.as_bool());
        changed || self.icons.len() != before
    }

    fn apply(&mut self, n: Notify) -> bool {
        let found = self.icons.iter().position(|icon| icon.matches(&n));
        match (n.message, found) {
            (NIM_DELETE, Some(i)) => {
                self.icons.remove(i);
                true
            }
            (NIM_SETVERSION, Some(i)) => {
                self.icons[i].version = n.version;
                false
            }
            (NIM_ADD | NIM_MODIFY, Some(i)) => {
                self.next_stamp += 1;
                let icon = &mut self.icons[i];
                icon.stamp = self.next_stamp;
                if n.flags & NIF_MESSAGE != 0 {
                    icon.callback = n.callback;
                }
                if n.flags & NIF_ICON != 0 {
                    icon.icon = n.icon;
                }
                if n.flags & NIF_TIP != 0 && !n.tip.is_empty() {
                    icon.label = n.tip;
                }
                if n.flags & NIF_STATE != 0 {
                    icon.state = (icon.state & !n.state_mask) | (n.state & n.state_mask);
                }
                true
            }
            // DeskIsle may start after an icon was added. Accept the first update that
            // carries an image so icons from applications ignoring TaskbarCreated appear.
            (NIM_ADD, None) | (NIM_MODIFY, None) if n.message == NIM_ADD || n.icon.is_some() => {
                self.next_stamp += 1;
                let guid = (n.flags & NIF_GUID != 0).then_some(n.guid);
                let exe = owner_path(n.owner);
                let ident = guid.map_or_else(|| format!("id:{}", n.id), |g| format!("{g:?}"));
                let label = if n.tip.is_empty() {
                    exe.as_ref()
                        .and_then(|p| p.file_stem())
                        .map_or_else(|| "托盘图标".into(), |s| s.to_string_lossy().into_owned())
                } else {
                    n.tip
                };
                let state = if n.flags & NIF_STATE != 0 {
                    n.state & n.state_mask
                } else {
                    0
                };
                self.icons.push(TrayIcon {
                    key: format!(
                        "{}|{ident}",
                        exe.map(|p| p.to_string_lossy().to_lowercase())
                            .unwrap_or_default()
                    ),
                    label,
                    stamp: self.next_stamp,
                    icon: n.icon,
                    owner: n.owner,
                    id: n.id,
                    guid,
                    callback: if n.flags & NIF_MESSAGE != 0 {
                        n.callback
                    } else {
                        0
                    },
                    version: 0,
                    state,
                });
                true
            }
            _ => false,
        }
    }

    /// Sends the click the system tray would send. `at` is the cursor in screen pixels.
    /// Returns whether the icon list changed because its owner had closed.
    pub fn click(&mut self, index: usize, right: bool, at: POINT) -> Result<bool> {
        let icon = &self.icons[index];
        let owner = HWND(icon.owner as _);
        if !unsafe { IsWindow(Some(owner)) }.as_bool() {
            self.icons.remove(index);
            return Ok(true);
        }
        if icon.callback == 0 {
            return Ok(false);
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(owner, Some(&mut pid)) };
        // This process handled the last input, so it may pass foreground rights on to the
        // application that opens a menu or window in response.
        let _ = unsafe { AllowSetForegroundWindow(pid) };
        let messages = if right {
            [WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU]
        } else {
            [WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT]
        };
        for (step, message) in messages.into_iter().enumerate() {
            // Version 3 added the selection message; version 4 moved the ID into lParam.
            let (wp, lp) = match icon.version {
                4.. => (
                    (at.x as u16 as usize) | ((at.y as u16 as usize) << 16),
                    (message as isize & 0xffff) | ((icon.id as isize & 0xffff) << 16),
                ),
                3 => (icon.id as usize, message as isize),
                _ if step == 2 => continue,
                _ => (icon.id as usize, message as isize),
            };
            if let Err(error) =
                unsafe { PostMessageW(Some(owner), icon.callback, WPARAM(wp), LPARAM(lp)) }
            {
                if !unsafe { IsWindow(Some(owner)) }.as_bool() {
                    self.icons.remove(index);
                    return Ok(true);
                }
                return Err(Error::new(
                    error.code(),
                    format!(
                        "无法把点击发送给托盘图标“{}”，该应用可能以管理员权限运行：{error}",
                        icon.label
                    ),
                ));
            }
        }
        Ok(false)
    }

    /// Asks Explorer's taskbar to show the desktop, or to restore what it minimized.
    pub fn toggle_desktop(&self) -> Result<()> {
        let Some(taskbar) = explorer_tray() else {
            return Err(Error::new(E_FAIL, "未找到资源管理器任务栏，无法显示桌面"));
        };
        // The command Explorer's own "Show desktop" corner sends to its taskbar window.
        const TOGGLE_DESKTOP: usize = 407;
        unsafe { PostMessageW(Some(taskbar), WM_COMMAND, WPARAM(TOGGLE_DESKTOP), LPARAM(0)) }
    }
}

impl Drop for TrayHost {
    fn drop(&mut self) {
        // FindWindow returns Explorer's taskbar again once this window is gone.
        let _ = unsafe { DestroyWindow(self.hwnd) };
        COMMANDS.take();
    }
}

fn owner_path(owner: isize) -> Option<PathBuf> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(HWND(owner as _), Some(&mut pid)) };
    (pid != 0).then(|| shell::process_path(pid).ok()).flatten()
}

fn forward(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let explorer = explorer_tray()?;
    let mut result = 0;
    // The system marshals WM_COPYDATA again; other forwarded messages carry no pointers.
    let sent = unsafe {
        SendMessageTimeoutW(
            explorer,
            msg,
            wp,
            lp,
            SMTO_ABORTIFHUNG,
            FORWARD_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    (sent.0 != 0).then_some(LRESULT(result as isize))
}

/// # Safety
/// `data` comes from a `WM_COPYDATA` being handled, so its block is valid for `cbData`
/// bytes until the message returns.
unsafe fn parse(data: &COPYDATASTRUCT) -> Option<Notify> {
    let len = data.cbData as usize;
    if data.lpData.is_null() || len < MIN_TRAY_MESSAGE {
        return None;
    }
    // Plain integers and arrays: an all-zero value is valid, and older, shorter
    // structures leave the newer fields zero.
    let mut message: TrayMessage = unsafe { std::mem::zeroed() };
    unsafe {
        std::ptr::copy_nonoverlapping(
            data.lpData as *const u8,
            (&raw mut message).cast::<u8>(),
            len.min(size_of::<TrayMessage>()),
        )
    };
    if message.signature != TRAY_SIGNATURE || message.message > NIM_SETVERSION {
        return None;
    }
    let n = message.data;
    let icon = if n.flags & NIF_ICON != 0 && n.icon != 0 {
        // Handles are 32-bit values shared across processes; sign-extend them. The sender
        // may destroy its icon as soon as this message returns, so copy it now.
        unsafe { CopyIcon(HICON(n.icon as i32 as isize as *mut c_void)) }
            .ok()
            .map(OwnedIcon)
    } else {
        None
    };
    let end = n.tip.iter().position(|&c| c == 0).unwrap_or(n.tip.len());
    Some(Notify {
        message: message.message,
        owner: n.hwnd as i32 as isize,
        id: n.id,
        flags: n.flags,
        callback: n.callback,
        icon,
        tip: String::from_utf16_lossy(&n.tip[..end]).trim().to_owned(),
        state: n.state,
        state_mask: n.state_mask,
        version: n.version,
        guid: n.guid,
    })
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            let data = lp.0 as *const COPYDATASTRUCT;
            let mut notify = false;
            if !data.is_null() {
                let data = unsafe { &*data };
                if data.dwData == COPYDATA_NOTIFY {
                    notify = true;
                    if let Some(command) = unsafe { parse(data) } {
                        COMMANDS.with_borrow_mut(|commands| commands.push(command));
                        queue(hwnd, Event::Tray);
                    }
                }
            }
            // Appbar and icon-position requests are answered by Explorer alone.
            forward(msg, wp, lp).unwrap_or(LRESULT(notify as isize))
        }
        // Explorer's private messages to its taskbar. Registered messages are
        // broadcast, so Explorer already receives those itself.
        WM_COMMAND | WM_USER..0xC000 => {
            forward(msg, wp, lp).unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_layout_matches_the_shell_contract() {
        // NOTIFYICONDATAW as shell32 marshals it to the tray: 956 bytes plus the header.
        assert_eq!(size_of::<NotifyData32>(), 956);
        assert_eq!(offset_of!(TrayMessage, data), 8);
    }

    #[test]
    fn short_or_foreign_copy_data_is_ignored() {
        let mut message: TrayMessage = unsafe { std::mem::zeroed() };
        message.signature = TRAY_SIGNATURE;
        message.message = NIM_MODIFY;
        message.data.id = 7;
        message.data.flags = NIF_TIP;
        message.data.tip[..2].copy_from_slice(&[b'h' as u16, b'i' as u16]);
        let mut data = COPYDATASTRUCT {
            dwData: COPYDATA_NOTIFY,
            cbData: size_of::<TrayMessage>() as u32,
            lpData: (&raw mut message).cast(),
        };
        let parsed = unsafe { parse(&data) }.unwrap();
        assert_eq!((parsed.id, parsed.tip.as_str()), (7, "hi"));
        data.cbData = MIN_TRAY_MESSAGE as u32 - 1;
        assert!(unsafe { parse(&data) }.is_none());
        data.cbData = size_of::<TrayMessage>() as u32;
        unsafe { (*data.lpData.cast::<TrayMessage>()).signature = 0 };
        assert!(unsafe { parse(&data) }.is_none());
    }
}
