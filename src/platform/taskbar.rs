use std::io::{BufRead, Write};
use std::process::{ChildStdin, Command, Stdio};

use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HRESULT, Owned, Result, w};

use crate::geometry::Rect;

pub const RESTORE_TASKBAR: &str = "--restore-taskbar";

pub fn claim_session() -> Result<Option<Owned<HANDLE>>> {
    // Only one Dock can manage the primary taskbar in this logon session.
    unsafe {
        let handle = CreateMutexW(None, false, w!("Local\\DeskIsle.Bars"))?;
        let exists = GetLastError() == ERROR_ALREADY_EXISTS;
        let mutex = Owned::new(handle);
        Ok((!exists).then_some(mutex))
    }
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// Only Explorer's primary taskbar, even when other processes host a Shell_TrayWnd.
pub(super) fn explorer_tray() -> Option<HWND> {
    let shell = unsafe { GetShellWindow() };
    let pid = window_pid(shell);
    if pid == 0 {
        return None;
    }
    let mut after = None;
    loop {
        let hwnd = unsafe { FindWindowExW(None, after, w!("Shell_TrayWnd"), None) }.ok()?;
        if window_pid(hwnd) == pid {
            return Some(hwnd);
        }
        after = Some(hwnd);
    }
}

fn identity(hwnd: HWND) -> String {
    format!("{} {}", window_pid(hwnd), hwnd.0 as usize)
}

#[derive(Default)]
pub struct NativeTaskbar {
    recovery: Option<ChildStdin>,
    hidden: String,
}

impl NativeTaskbar {
    pub fn hide(&mut self) -> Result<()> {
        let Some(hwnd) = explorer_tray() else {
            return Ok(());
        };
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return Ok(());
        }
        // Arm recovery before hiding anything. The OS closes our pipe on normal exit,
        // failed startup and process termination alike. Explorer may recreate its HWND.
        if self.recovery.is_none() {
            let child = Command::new(std::env::current_exe()?)
                .arg(RESTORE_TASKBAR)
                .stdin(Stdio::piped())
                .spawn()?;
            self.recovery = child.stdin;
        }
        let input = self
            .recovery
            .as_mut()
            .expect("recovery started with piped stdin");
        self.hidden = identity(hwnd);
        writeln!(input, "{}", self.hidden)?;
        set_visibility(hwnd, SWP_HIDEWINDOW)
    }
}

fn set_visibility(hwnd: HWND, flag: SET_WINDOW_POS_FLAGS) -> Result<()> {
    let result = unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            flag | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        )
    };
    match result {
        // Explorer may destroy this HWND during a restart. A new taskbar is picked up
        // by its TaskbarCreated or window-show event.
        Err(e) if e.code() == HRESULT::from_win32(ERROR_INVALID_WINDOW_HANDLE.0) => Ok(()),
        result => result,
    }
}

impl Drop for NativeTaskbar {
    fn drop(&mut self) {
        // Restore before releasing the session mutex. A blank line disarms the backup
        // so a quick relaunch can immediately take over the taskbar again.
        if let Some(pipe) = &mut self.recovery
            && restore(&self.hidden).is_ok()
        {
            let _ = writeln!(pipe);
        }
    }
}

/// The recovery process has no windows. Restore only the last Explorer taskbar the
/// Dock actually took over; a replacement window keeps its own visibility state.
pub fn restore_on_disconnect() -> Result<()> {
    let mut hidden = String::new();
    for line in std::io::stdin().lock().lines() {
        hidden = line?;
    }
    restore(&hidden)
}

fn restore(hidden: &str) -> Result<()> {
    if !hidden.is_empty()
        && let Some(hwnd) = explorer_tray()
        && identity(hwnd) == hidden
        && !unsafe { IsWindowVisible(hwnd) }.as_bool()
    {
        set_visibility(hwnd, SWP_SHOWWINDOW)?;
    }
    Ok(())
}

pub(super) fn dock_work_area(bounds: Rect, work: Rect) -> Rect {
    let Some(hwnd) = explorer_tray() else {
        return work;
    };
    let mut tray = RECT::default();
    if !unsafe { IsWindowVisible(hwnd) }.as_bool()
        && unsafe { GetWindowRect(hwnd, &mut tray) }.is_ok()
        && tray.top == work.y + work.height
        && tray.bottom >= bounds.y + bounds.height
        && tray.left <= work.x
        && tray.right >= work.x + work.width
    {
        // Explorer can keep its appbar reservation after its window is hidden.
        Rect {
            height: bounds.y + bounds.height - work.y,
            ..work
        }
    } else {
        work
    }
}
