//! Tray host for the status bar.
//!
//! Windows 11 has no public tray enumeration API. Explorer's notification settings
//! supply candidates for live Shell position queries. `Shell_NotifyIcon` sends a
//! `WM_COPYDATA` to the first `Shell_TrayWnd` that `FindWindow` returns, so DeskIsle owns
//! a hidden window of that class above Explorer's taskbar in the Z order, records the
//! icons it receives and forwards registrations to Explorer. The system tray keeps
//! working while DeskIsle runs and after it exits.

mod attention;
mod filter;
mod menu;
mod placement;
mod snapshot;

pub(super) use menu::window_event as menu_event;

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::path::PathBuf;
use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, IWICImagingFactory};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, GUID, Result, w};

use super::taskbar::explorer_tray;
use super::{Event, TASKBAR_CREATED, queue, shell};
use crate::geometry::Rect;
use attention::{Attention, Pixels};

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
const NIF_INFO: u32 = 0x10;
const NIF_GUID: u32 = 0x20;
const NIS_HIDDEN: u32 = 0x01;
const NIN_SELECT: u32 = WM_USER;
const HWND_BROADCAST: HWND = HWND(0xffff as _);
const FORWARD_TIMEOUT_MS: u32 = 3000;
const NIN_KEYSELECT: u32 = WM_USER + 1;

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
    /// Explorer can identify a GUID registration before its owner sends a callback.
    snapshot_path: Option<PathBuf>,
    received: Instant,
    balloon: bool,
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
    /// What the application calls itself: its product name, else the first line of its
    /// first tooltip, else its file name.
    pub name: String,
    /// The current tooltip, which many applications use for live status.
    pub label: String,
    /// Changes when the displayed image or attention mark changes.
    pub stamp: u64,
    pub(crate) registration: u64,
    attention: Attention,
    icon: Option<OwnedIcon>,
    owner: isize,
    id: u32,
    guid: Option<GUID>,
    system_indicator: bool,
    callback: u32,
    version: u32,
    state: u32,
}

impl TrayIcon {
    /// Only the live registration supplies the owner's input message.
    pub fn can_interact(&self) -> bool {
        self.owner != 0 && self.callback != 0
    }

    pub fn needs_attention(&self) -> bool {
        self.attention.unread
    }

    fn set_image(
        &mut self,
        image: Option<OwnedIcon>,
        received: Instant,
        factory: &IWICImagingFactory,
    ) -> bool {
        if self.system_indicator {
            return false;
        }
        let pixels = match &image {
            None => Some(Pixels::default()),
            Some(icon) => Pixels::read(factory, icon.0),
        };
        if let Some(pixels) = pixels {
            if !self.attention.frame(pixels, received) {
                return false;
            }
        } else {
            // Failed to compare this image. Keep normal dynamic icon rendering.
            self.attention.reset_frames();
        }
        self.icon = image;
        true
    }

    /// Owned by the tray host; valid until the next [`TrayHost::update`].
    pub fn icon(&self) -> Option<HICON> {
        self.icon.as_ref().map(|icon| icon.0)
    }

    /// Application icons available to the bar and its overflow menu.
    pub fn visible(&self) -> bool {
        !self.system_indicator && self.state & NIS_HIDDEN == 0
    }

    fn matches(&self, n: &Notify) -> bool {
        self.identifies(n.owner, n.id, (n.flags & NIF_GUID != 0).then_some(n.guid))
    }

    fn changes_click_target(&self, n: &Notify) -> bool {
        match n.message {
            NIM_ADD | NIM_DELETE => true,
            NIM_MODIFY => n.flags & NIF_MESSAGE != 0 && self.callback != n.callback,
            NIM_SETVERSION => self.version != n.version,
            _ => false,
        }
    }

    fn identifies(&self, owner: isize, id: u32, guid: Option<GUID>) -> bool {
        match guid {
            Some(guid) => self.guid == Some(guid),
            None => self.guid.is_none() && self.owner == owner && self.id == id,
        }
    }
}

pub struct TrayHost {
    imaging: IWICImagingFactory,
    hwnd: HWND,
    icons: Vec<TrayIcon>,
    next_stamp: u64,
    last_press: Option<(u64, bool)>,
    /// Owner windows already asked to add their icons again since the last broadcast.
    asked: HashSet<isize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrayInput {
    Press { right: bool, double: bool },
    Release { right: bool },
    KeySelect,
    ContextMenu,
}

impl TrayHost {
    /// Call on the UI thread after `create_bars` registered the TaskbarCreated message.
    pub fn new() -> Result<Self> {
        unsafe {
            let imaging = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
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
                imaging,
                hwnd,
                icons: Vec::new(),
                next_stamp: 0,
                last_press: None,
                asked: HashSet::new(),
            };
            host.raise();
            // The UI loop asks applications for their icons once it can answer them.
            Ok(host)
        }
    }

    /// Seed existing registrations before requesting their callback details. Call once
    /// the UI loop can answer applications that immediately call Shell_NotifyIcon.
    pub fn request_icons(&mut self) -> Result<()> {
        self.asked.clear();
        self.raise();
        self.recover_icons();
        unsafe { SendNotifyMessageW(HWND_BROADCAST, TASKBAR_CREATED.get(), WPARAM(0), LPARAM(0)) }
    }

    /// Older shells may lack the settings key; live registrations still populate
    /// the host. Retry the snapshot when the user opens the list, including after
    /// Explorer was too busy to confirm an icon at startup.
    pub fn recover_icons(&mut self) {
        if let Ok(mut items) =
            snapshot::read(&self.icons, |owner| request_owner(&mut self.asked, owner))
            && !items.is_empty()
        {
            // Real adds, changes and deletes received during the snapshot win.
            COMMANDS.with_borrow_mut(|commands| {
                items.append(commands);
                *commands = items;
            });
            queue(self.hwnd, Event::Tray);
        }
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
        placement::update(rects.filter_map(|(index, rect)| {
            let icon = &self.icons[index];
            // Explorer must answer liveness probes while a GUID has no registered owner.
            (icon.owner != 0).then_some((icon, rect))
        }));
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

    /// The application's own tray menus may offer Exit even when closing its window
    /// only hides it. Match the selected instance while its window is available.
    pub fn app_menus(&self, app_id: &str, window: Option<isize>) -> Vec<(usize, &str)> {
        let window_pid = window.map(|id| shell::window_process(HWND(id as _)));
        let desktop_pid = shell::window_process(unsafe { GetShellWindow() });
        self.icons
            .iter()
            .enumerate()
            .filter(|(_, icon)| icon.visible() && icon.can_interact())
            .filter_map(|(index, icon)| {
                let pid = shell::window_process(HWND(icon.owner as _));
                if pid == 0 || pid == desktop_pid || window_pid.is_some_and(|target| target != pid)
                {
                    return None;
                }
                let path = shell::process_path(pid).ok()?;
                (crate::dock::path_id(&path) == app_id).then_some((index, icon.label.as_str()))
            })
            .collect()
    }

    /// Applies queued notifications and drops icons whose owner window closed.
    /// Returns whether the visible tray changed.
    pub fn update(&mut self, windows_changed: bool) -> bool {
        let commands = COMMANDS.take();
        let mut changed = false;
        for command in commands {
            changed |= self.apply(command);
        }
        let before = self.icons.len();
        self.icons.retain(|icon| {
            if icon.owner == 0 {
                !windows_changed || snapshot::registered(0, icon.id, icon.guid)
            } else {
                unsafe { IsWindow(Some(HWND(icon.owner as _))) }.as_bool()
            }
        });
        changed || self.icons.len() != before
    }

    fn apply(&mut self, n: Notify) -> bool {
        let found = self.icons.iter().position(|icon| icon.matches(&n));
        if let Some(icon) = found.map(|i| &self.icons[i])
            && self
                .last_press
                .is_some_and(|(id, _)| id == icon.registration)
            && icon.changes_click_target(&n)
        {
            self.cancel_click();
        }
        match (n.message, found) {
            (NIM_DELETE, Some(i)) => {
                self.icons.remove(i);
                true
            }
            (NIM_MODIFY | NIM_SETVERSION, Some(i)) => {
                let icon = &mut self.icons[i];
                let mut changed = false;
                if icon.owner == 0
                    && n.owner != 0
                    && unsafe { IsWindow(Some(HWND(n.owner as _))) }.as_bool()
                {
                    ask_to_register(&mut self.asked, &n);
                    icon.owner = n.owner;
                    icon.id = n.id;
                    icon.system_indicator =
                        filter::is_system_indicator(icon.guid, HWND(n.owner as _));
                    changed = true;
                }
                if n.message == NIM_SETVERSION {
                    icon.version = n.version;
                    return changed;
                }
                let was_unread = icon.needs_attention();
                if n.flags & NIF_MESSAGE != 0 {
                    let interactive = icon.can_interact();
                    icon.callback = n.callback;
                    changed |= interactive != icon.can_interact();
                }
                if n.balloon {
                    icon.attention.notify(n.received);
                }
                if n.flags & NIF_ICON != 0 {
                    changed |= icon.set_image(n.icon, n.received, &self.imaging);
                }
                let label_changed =
                    n.flags & NIF_TIP != 0 && !n.tip.is_empty() && icon.label != n.tip;
                if label_changed {
                    icon.attention.tooltip(&mut icon.label, n.tip, n.received);
                }
                if n.flags & NIF_STATE != 0 {
                    let visible = icon.visible();
                    icon.state = (icon.state & !n.state_mask) | (n.state & n.state_mask);
                    changed |= visible != icon.visible();
                }
                changed |= was_unread != icon.needs_attention();
                if changed {
                    self.next_stamp += 1;
                    icon.stamp = self.next_stamp;
                }
                changed || label_changed
            }
            // An icon can predate this host. Keep the fields we have (even a tooltip
            // alone) and request a full registration, including the callback/version.
            (NIM_ADD, _) | (NIM_MODIFY, None) => {
                if n.message == NIM_MODIFY {
                    ask_to_register(&mut self.asked, &n);
                }
                self.next_stamp += 1;
                let guid = (n.flags & NIF_GUID != 0).then_some(n.guid);
                let exe = n.snapshot_path.or_else(|| owner_path(n.owner));
                let ident = guid.map_or_else(|| format!("id:{}", n.id), |g| format!("{g:?}"));
                let stem = exe
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().into_owned());
                let product = exe.as_deref().and_then(shell::product_name);
                let name = display_name(product, &n.tip, stem.as_deref());
                let label = if n.tip.is_empty() {
                    name.clone()
                } else {
                    n.tip
                };
                let state = if n.flags & NIF_STATE != 0 {
                    n.state & n.state_mask
                } else {
                    0
                };
                let mut icon = TrayIcon {
                    key: format!(
                        "{}|{ident}",
                        exe.map(|p| p.to_string_lossy().to_lowercase())
                            .unwrap_or_default()
                    ),
                    name,
                    label,
                    stamp: self.next_stamp,
                    registration: self.next_stamp,
                    attention: Attention::default(),
                    icon: None,
                    owner: n.owner,
                    id: n.id,
                    guid,
                    system_indicator: filter::is_system_indicator(guid, HWND(n.owner as _)),
                    callback: if n.flags & NIF_MESSAGE != 0 {
                        n.callback
                    } else {
                        0
                    },
                    version: 0,
                    state,
                };
                icon.attention.unread = n.balloon;
                if n.flags & NIF_ICON != 0 {
                    icon.set_image(n.icon, n.received, &self.imaging);
                }
                if let Some(index) = found {
                    self.icons[index] = icon;
                } else {
                    self.icons.push(icon);
                }
                true
            }
            (NIM_SETVERSION, None) => {
                ask_to_register(&mut self.asked, &n);
                false
            }
            _ => false,
        }
    }

    /// Forward the user's gesture to the registered owner. For right-button events
    /// the caller supplies the existing menu anchor at the bottom of the bar.
    pub fn click(&mut self, index: usize, mut input: TrayInput, at: POINT) -> Result<bool> {
        let icon = &self.icons[index];
        // A popup keeps its rows stable until it closes. Do not send its gesture to
        // a registration which the owner has already withdrawn or replaced meanwhile.
        if !icon.can_interact()
            || COMMANDS.with_borrow(|pending| {
                pending
                    .iter()
                    .any(|n| icon.matches(n) && icon.changes_click_target(n))
            })
        {
            self.cancel_click();
            return Ok(false);
        }
        match input {
            TrayInput::Press { right, double } => {
                menu::stop();
                let same = self.last_press.replace((icon.registration, right))
                    == Some((icon.registration, right));
                // All slots share an HWND. Windows can report a double click across
                // adjacent icons or after an application replaces its registration.
                input = TrayInput::Press {
                    right,
                    double: double && same,
                };
            }
            TrayInput::Release { right } if self.last_press != Some((icon.registration, right)) => {
                return Ok(false);
            }
            TrayInput::KeySelect | TrayInput::ContextMenu => self.last_press = None,
            _ => {}
        }
        let owner = HWND(icon.owner as _);
        let pid = shell::window_process(owner);
        if pid == 0 {
            // The open menu may still hold indices into this snapshot. Its queued Tray
            // event will prune the dead owner once those indices are no longer in use.
            self.cancel_click();
            queue(self.hwnd, Event::Tray);
            return Ok(false);
        }
        let _ = unsafe { AllowSetForegroundWindow(pid) };
        if matches!(
            input,
            TrayInput::Press { right: true, .. }
                | TrayInput::Release { right: true }
                | TrayInput::ContextMenu
        ) {
            menu::watch(pid, icon.registration, at);
        }
        for (wp, lp) in click_callbacks(icon.version, icon.id, input, at) {
            if let Err(error) = unsafe { PostMessageW(Some(owner), icon.callback, wp, lp) } {
                self.last_press = None;
                menu::stop();
                if !unsafe { IsWindow(Some(owner)) }.as_bool() {
                    queue(self.hwnd, Event::Tray);
                    return Ok(false);
                }
                return Err(error);
            }
        }
        Ok(matches!(
            input,
            TrayInput::Release { right: false } | TrayInput::KeySelect
        ) && self.acknowledge(index, Instant::now()))
    }

    fn acknowledge(&mut self, index: usize, clicked_at: Instant) -> bool {
        let icon = &mut self.icons[index];
        if icon.attention.acknowledge(clicked_at) {
            self.next_stamp += 1;
            icon.stamp = self.next_stamp;
            return true;
        }
        false
    }

    pub fn cancel_click(&mut self) {
        menu::stop();
        self.last_press = None;
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
    input: TrayInput,
    at: POINT,
) -> impl Iterator<Item = (WPARAM, LPARAM)> {
    let messages: &[u32] = match input {
        TrayInput::Press {
            right: false,
            double: false,
        } => &[WM_LBUTTONDOWN],
        TrayInput::Press {
            right: false,
            double: true,
        } => &[WM_LBUTTONDBLCLK],
        TrayInput::Press {
            right: true,
            double: false,
        } => &[WM_RBUTTONDOWN],
        TrayInput::Press {
            right: true,
            double: true,
        } => &[WM_RBUTTONDBLCLK],
        TrayInput::Release { right: false } if version >= 3 => &[WM_LBUTTONUP, NIN_SELECT],
        TrayInput::Release { right: false } => &[WM_LBUTTONUP],
        TrayInput::Release { right: true } if version >= 3 => &[WM_RBUTTONUP, WM_CONTEXTMENU],
        TrayInput::Release { right: true } => &[WM_RBUTTONUP],
        TrayInput::KeySelect if version >= 3 => &[NIN_KEYSELECT],
        TrayInput::KeySelect => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
        // The menu key asks a version-aware owner for its menu directly.
        TrayInput::ContextMenu if version >= 3 => &[WM_CONTEXTMENU],
        TrayInput::ContextMenu => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
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

/// A tray icon's name for lists: the product name, else the tooltip's first line, else
/// the executable's file name.
fn display_name(product: Option<String>, tip: &str, stem: Option<&str>) -> String {
    product
        .or_else(|| {
            tip.lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| stem.map(str::to_owned))
        .unwrap_or_else(|| "托盘图标".into())
}

/// Posts TaskbarCreated to the owner of `n`, once per owner until the next broadcast.
/// Applications answer it by adding their icons again, as after an Explorer restart.
/// Windows' own indicators are left alone.
fn ask_to_register(asked: &mut HashSet<isize>, n: &Notify) {
    let owner = HWND(n.owner as _);
    let guid = (n.flags & NIF_GUID != 0).then_some(n.guid);
    if filter::is_system_indicator(guid, owner) {
        return;
    }
    request_owner(asked, owner);
}

fn request_owner(asked: &mut HashSet<isize>, owner: HWND) {
    if owner.is_invalid() || !asked.insert(owner.0 as isize) {
        return;
    }
    // A closed owner or one UIPI protects cannot be asked; its icon appears on its next
    // update with an image, as before.
    let _ = unsafe { PostMessageW(Some(owner), TASKBAR_CREATED.get(), WPARAM(0), LPARAM(0)) };
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
    let mut n = message.data;
    let icon = if n.flags & NIF_ICON != 0 && n.icon != 0 {
        // Handles are 32-bit values shared across processes; sign-extend them. The sender
        // may destroy its icon as soon as this message returns, so copy it now.
        match unsafe { CopyIcon(HICON(n.icon as i32 as isize as *mut c_void)) } {
            Ok(icon) => Some(OwnedIcon(icon)),
            Err(_) => {
                // A stale handle is not an intentional blank phase.
                n.flags &= !NIF_ICON;
                None
            }
        }
    } else {
        None
    };
    let end = n.tip.iter().position(|&c| c == 0).unwrap_or(n.tip.len());
    Some(Notify {
        snapshot_path: None,
        received: Instant::now(),
        balloon: n.flags & NIF_INFO != 0 && n.info[0] != 0,
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
            let mut accepted = None;
            if !data.is_null() {
                let data = unsafe { &*data };
                if data.dwData == placement::COPYDATA_ICON_RECT
                    && let Some(result) = unsafe { placement::reply(data) }
                {
                    return result;
                }
                if data.dwData == COPYDATA_NOTIFY
                    && let Some(command) = unsafe { parse(data) }
                {
                    if unsafe { IsWindow(Some(HWND(command.owner as _))) }.as_bool() {
                        accepted = Some(command.message);
                    }
                    COMMANDS.with_borrow_mut(|commands| commands.push(command));
                    queue(hwnd, Event::Tray);
                }
            }
            // Explorer handles AppBars and any icon positions we cannot supply.
            let forwarded = forward(msg, wp, lp);
            // Explorer may reject a repeated ADD for an icon it already hosts.
            // This host did accept it, so the app can continue with NIM_SETVERSION.
            if accepted == Some(NIM_ADD) {
                LRESULT(1)
            } else {
                forwarded.unwrap_or(LRESULT(accepted.is_some() as isize))
            }
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
    fn tray_icons_are_named_by_their_product_not_their_status_text() {
        let speed = "\u{2191} 943 B/s\n\u{2193} 0 B/s";
        assert_eq!(
            display_name(Some("Sparkle".into()), speed, Some("Sparkle")),
            "Sparkle"
        );
        assert_eq!(
            display_name(None, "\n NVIDIA GeForce\r\n2 programs", Some("nvcontainer")),
            "NVIDIA GeForce"
        );
        assert_eq!(display_name(None, "", Some("QQ")), "QQ");
        assert_eq!(display_name(None, " ", None), "托盘图标");
    }

    #[test]
    fn updates_for_unknown_icons_ask_their_owner_to_add_them_once() -> Result<()> {
        use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
        // A message-only owner keeps the request inside this test's thread queue.
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
        let message = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        TASKBAR_CREATED.set(message);
        let mut tray = TrayHost {
            imaging: unsafe {
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?
            },
            hwnd: owner,
            icons: Vec::new(),
            next_stamp: 0,
            last_press: None,
            asked: HashSet::new(),
        };
        let update = || Notify {
            snapshot_path: None,
            received: Instant::now(),
            balloon: false,
            message: NIM_MODIFY,
            owner: owner.0 as isize,
            id: 3,
            flags: NIF_TIP,
            callback: 0,
            icon: None,
            tip: "\u{2191} 943 B/s".into(),
            state: 0,
            state_mask: 0,
            version: 0,
            guid: GUID::zeroed(),
        };
        let received = || {
            let mut msg = MSG::default();
            let mut count = 0;
            while unsafe { PeekMessageW(&mut msg, Some(owner), message, message, PM_REMOVE) }
                .as_bool()
            {
                count += 1;
            }
            count
        };
        // The first tooltip-only update is listed immediately. Subsequent updates to
        // that registration don't flood its message-only window with requests.
        assert!(tray.apply(update()));
        tray.apply(update());
        assert_eq!(tray.icons.len(), 1);
        assert!(tray.icons[0].visible());
        assert_eq!(tray.icons[0].label, update().tip);
        assert_eq!(received(), 1);
        // Shell resolves a GUID without providing the HWND. Its subsequent full
        // registration replaces the recovered row, even with a nonzero numeric ID.
        let guid = GUID::from_u128(0x1234);
        tray.apply(Notify {
            guid,
            flags: NIF_GUID | NIF_TIP,
            owner: 0,
            id: 0,
            snapshot_path: Some("C:\\Test.exe".into()),
            ..update()
        });
        assert_eq!(tray.icons.len(), 2);
        assert!(!tray.icons[1].can_interact());
        assert!(!tray.click(1, TrayInput::KeySelect, POINT::default())?);
        tray.apply(Notify {
            guid,
            flags: NIF_GUID | NIF_MESSAGE,
            callback: WM_APP + 3,
            message: NIM_ADD,
            ..update()
        });
        assert_eq!(tray.icons.len(), 2);
        let restored = &tray.icons[1];
        assert!(restored.can_interact());
        assert_eq!(
            (restored.owner, restored.id, restored.callback),
            (owner.0 as isize, 3, WM_APP + 3)
        );
        tray.apply(Notify {
            guid,
            flags: NIF_GUID,
            message: NIM_DELETE,
            ..update()
        });
        assert_eq!(tray.icons.len(), 1);
        drop(tray);
        unsafe { RoUninitialize() };
        Ok(())
    }

    #[test]
    fn owner_receives_only_the_gesture_for_its_registration() -> Result<()> {
        use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { RoUninitialize() };
            }
        }
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
        let _apartment = Apartment;
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
            imaging: unsafe {
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?
            },
            hwnd: owner,
            icons: Vec::new(),
            next_stamp: 0,
            last_press: None,
            asked: HashSet::new(),
        };
        let registration = || Notify {
            snapshot_path: None,
            received: Instant::now(),
            balloon: false,
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
        let balloon = || Notify {
            message: NIM_MODIFY,
            balloon: true,
            ..registration()
        };
        assert!(host.apply(balloon()));
        assert!(host.icons[0].needs_attention());
        let down = TrayInput::Press {
            right: false,
            double: false,
        };
        let double = TrayInput::Press {
            right: false,
            double: true,
        };
        let up = TrayInput::Release { right: false };
        let point = usize::from(anchor.x as u16) | (usize::from(anchor.y as u16) << 16);
        // Revoking input while the button is held takes effect even while an
        // overflow row is waiting for release before applying queued updates.
        host.click(0, down, anchor)?;
        assert_eq!(receive(), [(WM_LBUTTONDOWN, point)]);
        let callback_update = |callback| Notify {
            message: NIM_MODIFY,
            callback,
            ..registration()
        };
        COMMANDS.with_borrow_mut(|pending| pending.push(callback_update(0)));
        assert!(!host.click(0, up, anchor)?);
        assert!(host.update(false));
        assert!(!host.icons[0].can_interact());
        for input in [down, up, TrayInput::KeySelect, TrayInput::ContextMenu] {
            assert!(!host.click(0, input, anchor)?);
        }
        assert!(host.icons[0].needs_attention());
        assert!(host.apply(callback_update(CALLBACK)));
        assert!(!host.click(0, up, anchor)?);
        assert!(receive().is_empty());
        // A fresh gesture is delivered after the application registers again.
        host.click(0, down, anchor)?;
        assert_eq!(receive(), [(WM_LBUTTONDOWN, point)]);
        assert!(host.click(0, up, anchor)?);
        assert!(!host.icons[0].needs_attention());
        assert_eq!(receive(), [(WM_LBUTTONUP, point), (NIN_SELECT, point)]);
        host.click(0, double, anchor)?;
        host.click(0, up, anchor)?;
        assert_eq!(
            receive(),
            [
                (WM_LBUTTONDBLCLK, point),
                (WM_LBUTTONUP, point),
                (NIN_SELECT, point)
            ]
        );

        host.click(0, down, anchor)?;
        receive();
        host.apply(registration());
        host.icons[0].version = 4;
        host.click(0, up, anchor)?;
        assert!(receive().is_empty());
        host.click(0, double, anchor)?;
        assert_eq!(receive(), [(WM_LBUTTONDOWN, point)]);
        host.click(0, TrayInput::KeySelect, anchor)?;
        assert_eq!(receive(), [(NIN_KEYSELECT, point)]);
        host.click(
            0,
            TrayInput::Press {
                right: true,
                double: false,
            },
            anchor,
        )?;
        assert_eq!(receive(), [(WM_RBUTTONDOWN, point)]);
        host.click(0, TrayInput::Release { right: true }, anchor)?;
        assert_eq!(receive(), [(WM_RBUTTONUP, point), (WM_CONTEXTMENU, point)]);
        COMMANDS.with_borrow_mut(|queue| {
            queue.push(Notify {
                message: NIM_DELETE,
                ..registration()
            })
        });
        host.click(0, TrayInput::ContextMenu, anchor)?;
        assert!(receive().is_empty());
        Ok(())
    }

    #[test]
    fn tray_menu_callbacks_preserve_the_anchor_and_legacy_icon_id() {
        let at = POINT { x: -250, y: 48 };
        let modern: Vec<_> = click_callbacks(4, 77, TrayInput::ContextMenu, at)
            .map(|(wp, lp)| {
                (
                    (wp.0 as i16, (wp.0 >> 16) as i16),
                    (lp.0 as u16, (lp.0 >> 16) as u16),
                )
            })
            .collect();
        assert_eq!(modern, [((-250, 48), (WM_CONTEXTMENU as u16, 77))]);
        for (version, events) in [
            (0, &[WM_RBUTTONDOWN, WM_RBUTTONUP][..]),
            (3, &[WM_CONTEXTMENU][..]),
        ] {
            let legacy: Vec<_> = click_callbacks(version, 0x1004d, TrayInput::ContextMenu, at)
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
