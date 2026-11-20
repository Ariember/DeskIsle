//! Tray host for the status bar.
//!
//! Windows 11 offers no API to read the notification area. `Shell_NotifyIcon` sends a
//! `WM_COPYDATA` to the first `Shell_TrayWnd` that `FindWindow` returns, so DeskIsle owns
//! a hidden window of that class above Explorer's taskbar in the Z order, records the
//! icons it receives and forwards registrations to Explorer. The system tray keeps
//! working while DeskIsle runs and after it exits.

mod menu;
mod placement;

pub(super) use menu::window_event as menu_event;

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, GUID, Result, w};

use super::taskbar::explorer_tray;
use super::{Event, TASKBAR_CREATED, queue, shell};
use crate::geometry::Rect;

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
const ACTIVATION_TIMER: usize = 1;
const ACTIVATION_WAIT_MS: u32 = 300;

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
    registration: u64,
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
    activation: Option<PendingActivation>,
}

struct PendingActivation {
    registration: u64,
    pid: u32,
    foreground: HWND,
    previous_windows: Option<Vec<HWND>>,
    click_at: POINT,
    menu_at: POINT,
    sent_double_click: bool,
    deadline: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrayClick {
    Left,
    Right,
    // The first press and release were already posted.
    CompleteDoubleClick,
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
                activation: None,
            };
            host.raise();
            // Applications add their icons again on TaskbarCreated, now through this host.
            // UIPI keeps the broadcast from elevated processes; their icons appear on their
            // next update.
            host.request_icons()?;
            Ok(host)
        }
    }

    pub fn request_icons(&self) -> Result<()> {
        unsafe { SendNotifyMessageW(HWND_BROADCAST, TASKBAR_CREATED.get(), WPARAM(0), LPARAM(0)) }
    }

    /// Keeps this host first for `FindWindow`. Explorer raises its taskbar when the user
    /// clicks it, so the UI loop calls this after foreground changes.
    pub fn raise(&self) {
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        };
    }

    /// Apps that locate the tray by window class use the edge hosting our icons.
    pub fn position(&self, status_bar: Option<Rect>) -> Result<()> {
        let bar = match status_bar {
            Some(rect) => rect,
            None => {
                // When the user keeps the native tray, Explorer supplies its geometry.
                let mut rect = RECT::default();
                let Some(explorer) = explorer_tray() else {
                    return Ok(());
                };
                if unsafe { GetWindowRect(explorer, &mut rect) }.is_err() {
                    return Ok(()); // Explorer can restart during a display change.
                }
                super::to_rect(rect)
            }
        };
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                bar.x,
                bar.y,
                bar.width,
                bar.height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        }
    }

    /// Publishes screen-space hit rectangles for synchronous Shell position queries.
    pub fn set_icon_rects(&self, rects: impl Iterator<Item = (usize, Rect)>) {
        placement::update(rects.map(|(index, rect)| (&self.icons[index], rect)));
    }

    /// Shell32 looks up Shell_TrayWnd when negotiating an AppBar. Its position
    /// requests use memory shared with that process, so Explorer must receive
    /// them directly, including after a display change or Explorer restart.
    pub fn with_explorer_taskbar<T>(&self, action: impl FnOnce() -> Result<T>) -> Result<T> {
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )?;
        }
        let result = action();
        self.raise();
        result
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
            (NIM_MODIFY, Some(i)) => {
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
            (NIM_ADD, _) | (NIM_MODIFY, None) if n.message == NIM_ADD || n.icon.is_some() => {
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
                let icon = TrayIcon {
                    key: format!(
                        "{}|{ident}",
                        exe.map(|p| p.to_string_lossy().to_lowercase())
                            .unwrap_or_default()
                    ),
                    label,
                    stamp: self.next_stamp,
                    registration: self.next_stamp,
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
                };
                if let Some(index) = found {
                    self.icons[index] = icon;
                } else {
                    self.icons.push(icon);
                }
                true
            }
            _ => false,
        }
    }

    /// Activates an app window or forwards the click. `at` is a screen-pixel menu anchor.
    /// Returns whether the icon list changed because its owner had closed.
    pub fn click(&mut self, index: usize, right: bool, at: POINT) -> Result<bool> {
        self.cancel_click();
        let icon = &self.icons[index];
        let pid = shell::window_process(HWND(icon.owner as _));
        if pid == 0 {
            self.icons.remove(index);
            return Ok(true);
        }
        let foreground = unsafe { GetForegroundWindow() };
        let mut previous_windows = if right {
            None
        } else {
            shell::tray_windows(pid).ok()
        };
        if let Some(windows) = &mut previous_windows {
            let root = unsafe { GetAncestor(foreground, GA_ROOTOWNER) };
            if windows.contains(&foreground) || windows.contains(&root) {
                return Ok(false);
            }
            if icon.callback == 0 && shell::activate_tray_window(pid, windows)? {
                return Ok(false);
            }
            windows.retain(|&hwnd| !unsafe { IsIconic(hwnd) }.as_bool());
        }
        if icon.callback == 0 {
            return Err(Error::new(
                E_FAIL,
                format!("托盘图标“{}”未注册点击回调，无法请求应用菜单", icon.label),
            ));
        }
        let registration = icon.registration;
        let click_at = if right {
            at
        } else {
            super::cursor().unwrap_or(at)
        };
        let click = if right {
            TrayClick::Right
        } else {
            TrayClick::Left
        };
        let changed = self.send_click(index, pid, click, click_at)?;
        if !right && !changed {
            // Posting a callback only queues it. Let the app restore its own hidden window
            // before considering a second action, without blocking the UI thread.
            if unsafe { SetTimer(Some(self.hwnd), ACTIVATION_TIMER, ACTIVATION_WAIT_MS, None) } == 0
            {
                return Err(Error::new(E_FAIL, "无法设置托盘点击定时器"));
            }
            self.activation = Some(PendingActivation {
                registration,
                pid,
                foreground,
                previous_windows,
                click_at,
                menu_at: at,
                sent_double_click: false,
                deadline: Instant::now() + Duration::from_millis(u64::from(ACTIVATION_WAIT_MS)),
            });
        }
        Ok(changed)
    }

    pub fn cancel_click(&mut self) {
        menu::stop();
        if self.activation.take().is_some() {
            let _ = unsafe { KillTimer(Some(self.hwnd), ACTIVATION_TIMER) };
        }
    }

    pub fn finish_activation(&mut self) -> Result<bool> {
        self.check_activation(Instant::now(), unsafe { GetForegroundWindow() })
    }

    fn check_activation(&mut self, now: Instant, foreground: HWND) -> Result<bool> {
        let Some(pending) = &self.activation else {
            return Ok(false);
        };
        // A timer message queued before cancellation can still arrive. If a registration
        // change is queued, let its Tray event run before using the icon again.
        if now < pending.deadline || !COMMANDS.with_borrow(Vec::is_empty) {
            return Ok(false);
        }
        if foreground != pending.foreground {
            // The app responded or the user moved on to another window.
            self.cancel_click();
            return Ok(false);
        }
        let Some(index) = self.icons.iter().position(|icon| {
            icon.registration == pending.registration && icon.visible() && icon.callback != 0
        }) else {
            self.cancel_click();
            return Ok(false);
        };
        let owner = HWND(self.icons[index].owner as _);
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(owner, Some(&mut pid)) };
        if thread == 0 || pid != pending.pid {
            self.cancel_click();
            return Ok(false);
        }
        let mut gui = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetGUIThreadInfo(thread, &mut gui) }.is_ok()
            && (gui.flags & (GUI_INMENUMODE | GUI_POPUPMENUMODE)).0 != 0
        {
            self.cancel_click();
            return Ok(false);
        }
        if let Ok(windows) = shell::tray_windows(pid) {
            // A background helper that was already visible is not evidence of a
            // response. Give the app's double-click action a turn before trying it.
            let new_windows: Vec<_> = windows
                .iter()
                .copied()
                .filter(|hwnd| {
                    pending
                        .previous_windows
                        .as_ref()
                        .is_some_and(|old| !old.contains(hwnd))
                        && !unsafe { IsIconic(*hwnd) }.as_bool()
                })
                .collect();
            let targets = if pending.sent_double_click && new_windows.is_empty() {
                &windows
            } else {
                &new_windows
            };
            if shell::activate_tray_window(pid, targets).unwrap_or(false) {
                self.cancel_click();
                return Ok(false);
            }
        }
        if pending.sent_double_click {
            let at = pending.menu_at;
            self.cancel_click();
            return self.send_click(index, pid, TrayClick::Right, at);
        }
        let at = pending.click_at;
        match self.send_click(index, pid, TrayClick::CompleteDoubleClick, at) {
            Ok(false) => {
                let pending = self.activation.as_mut().expect("tray activation pending");
                pending.sent_double_click = true;
                pending.deadline = now + Duration::from_millis(u64::from(ACTIVATION_WAIT_MS));
                Ok(false)
            }
            result => {
                self.cancel_click();
                result
            }
        }
    }

    fn send_click(&mut self, index: usize, pid: u32, click: TrayClick, at: POINT) -> Result<bool> {
        let icon = &self.icons[index];
        let owner = HWND(icon.owner as _);
        // This process handled the last input, so it may pass foreground rights on to the
        // application that opens a menu or window in response.
        let _ = unsafe { AllowSetForegroundWindow(pid) };
        if click == TrayClick::Right {
            menu::watch(pid, icon.registration, at);
        }
        for (wp, lp) in click_callbacks(icon.version, icon.id, click, at) {
            if let Err(error) = unsafe { PostMessageW(Some(owner), icon.callback, wp, lp) } {
                menu::stop();
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
        self.cancel_click();
        // FindWindow returns Explorer's taskbar again once this window is gone.
        let _ = unsafe { DestroyWindow(self.hwnd) };
        COMMANDS.take();
        placement::update(std::iter::empty());
    }
}

fn click_callbacks(
    version: u32,
    id: u32,
    click: TrayClick,
    at: POINT,
) -> impl Iterator<Item = (WPARAM, LPARAM)> {
    let messages: &[u32] = match click {
        TrayClick::Right if version >= 3 => &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU],
        TrayClick::Right => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
        TrayClick::Left if version >= 3 => &[WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT],
        TrayClick::Left => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
        TrayClick::CompleteDoubleClick => &[WM_LBUTTONDBLCLK, WM_LBUTTONUP],
    };
    // Version 4 moved the ID into lParam.
    messages.iter().map(move |&message| {
        if version >= 4 {
            let point = usize::from(at.x as u16) | (usize::from(at.y as u16) << 16);
            let event = message as isize | (((id & 0xffff) as isize) << 16);
            (WPARAM(point), LPARAM(event))
        } else {
            (WPARAM(id as usize), LPARAM(message as isize))
        }
    })
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
        WM_TIMER if wp.0 == ACTIVATION_TIMER => {
            queue(hwnd, Event::TrayActivation);
            LRESULT(0)
        }
        WM_COPYDATA => {
            let data = lp.0 as *const COPYDATASTRUCT;
            let mut notify = false;
            if !data.is_null() {
                let data = unsafe { &*data };
                if data.dwData == placement::COPYDATA_ICON_RECT
                    && let Some(result) = unsafe { placement::reply(data) }
                {
                    return result;
                }
                if data.dwData == COPYDATA_NOTIFY {
                    notify = true;
                    if let Some(command) = unsafe { parse(data) } {
                        COMMANDS.with_borrow_mut(|commands| commands.push(command));
                        queue(hwnd, Event::Tray);
                    }
                }
            }
            // Explorer handles AppBars and any icon positions we cannot supply.
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
    fn unhandled_left_click_completes_double_click_before_requesting_menu() -> Result<()> {
        // A message-only owner receives the same posted callbacks as a tray application,
        // without creating a taskbar host or changing the desktop's foreground window.
        let owner = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )?
        };
        const CALLBACK: u32 = WM_APP + 125;
        let mut host = TrayHost {
            hwnd: owner,
            icons: Vec::new(),
            next_stamp: 0,
            activation: None,
        };
        let registration = || Notify {
            message: NIM_ADD,
            owner: owner.0 as isize,
            id: 77,
            flags: NIF_MESSAGE,
            callback: CALLBACK,
            icon: None,
            tip: "Test".into(),
            state: 0,
            state_mask: 0,
            version: 0,
            guid: GUID::zeroed(),
        };
        host.apply(registration());
        host.icons[0].version = 4;
        let receive = || {
            let mut events = Vec::new();
            let mut msg = MSG::default();
            while unsafe { PeekMessageW(&mut msg, Some(owner), CALLBACK, CALLBACK, PM_REMOVE) }
                .as_bool()
            {
                assert_eq!(msg.lParam.0 >> 16, 77);
                events.push((msg.lParam.0 as u16 as u32, msg.wParam.0));
            }
            events
        };
        let anchor = POINT { x: -250, y: 48 };
        host.click(0, false, anchor)?;
        let pending = host.activation.as_ref().unwrap();
        let (later, foreground) = (pending.deadline, pending.foreground);
        let first = receive();
        assert_eq!(
            first.iter().map(|&(event, _)| event).collect::<Vec<_>>(),
            [WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT]
        );
        let at = first[0].1;
        host.check_activation(later, foreground)?;
        assert_eq!(receive(), [(WM_LBUTTONDBLCLK, at), (WM_LBUTTONUP, at)]);
        host.check_activation(later, foreground)?;
        assert!(receive().is_empty());
        host.check_activation(host.activation.as_ref().unwrap().deadline, foreground)?;
        let menu_at = usize::from(anchor.x as u16) | (usize::from(anchor.y as u16) << 16);
        assert_eq!(
            receive(),
            [
                (WM_RBUTTONDOWN, menu_at),
                (WM_RBUTTONUP, menu_at),
                (WM_CONTEXTMENU, menu_at)
            ]
        );
        host.check_activation(later, foreground)?;
        assert!(receive().is_empty());

        // Single-click handlers can foreground a window belonging to a helper process.
        host.click(0, false, anchor)?;
        receive();
        let pending = host.activation.as_ref().unwrap();
        let (later, foreground) = (pending.deadline, pending.foreground);
        host.check_activation(later, HWND_MESSAGE)?;
        host.check_activation(later, foreground)?;
        assert!(receive().is_empty());

        host.click(0, false, anchor)?;
        receive();
        let pending = host.activation.as_ref().unwrap();
        let (later, foreground) = (pending.deadline, pending.foreground);
        host.apply(registration());
        host.check_activation(later, foreground)?;
        assert!(receive().is_empty());
        Ok(())
    }

    #[test]
    fn tray_menu_callbacks_preserve_the_anchor_and_legacy_icon_id() {
        let at = POINT { x: -250, y: 48 };
        let modern: Vec<_> = click_callbacks(4, 77, TrayClick::Right, at)
            .map(|(wp, lp)| {
                (
                    (wp.0 as i16, (wp.0 >> 16) as i16),
                    (lp.0 as u16, (lp.0 >> 16) as u16),
                )
            })
            .collect();
        assert_eq!(
            modern,
            [WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU]
                .map(|event| ((-250, 48), (event as u16, 77)))
        );
        for (version, events) in [
            (0, &[WM_RBUTTONDOWN, WM_RBUTTONUP][..]),
            (3, &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU][..]),
        ] {
            let legacy: Vec<_> = click_callbacks(version, 0x1004d, TrayClick::Right, at)
                .map(|(wp, lp)| (wp.0, lp.0 as u32))
                .collect();
            assert!(
                legacy
                    .into_iter()
                    .eq(events.iter().map(|&event| (0x1004d, event)))
            );
        }
    }

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
