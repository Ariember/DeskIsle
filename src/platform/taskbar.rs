use std::io::{BufRead, Write};
use std::process::{ChildStdin, Command, Stdio};

use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
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
    released_bottom: bool,
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
        let hidden = identity(hwnd);
        if hidden != self.hidden {
            writeln!(self.recovery.as_mut().unwrap(), "{hidden}")?;
            self.hidden = hidden;
            self.released_bottom = false;
        }
        set_visibility(hwnd, SWP_HIDEWINDOW)?;
        if let Err(error) = self.refresh_work_area() {
            self.show()?;
            return Err(error);
        }
        Ok(())
    }

    /// Explorer can publish its taskbar strip again after an AppBar or display change.
    pub fn refresh_work_area(&mut self) -> Result<()> {
        if self.hidden.is_empty() {
            return Ok(());
        }
        if let Some(hwnd) = explorer_tray()
            && identity(hwnd) == self.hidden
            && !unsafe { IsWindowVisible(hwnd) }.as_bool()
            && let Some(work) = adjusted_work_area(hwnd, true)?
        {
            if !self.released_bottom {
                // Arm the helper before changing the desktop, including on partial startup.
                writeln!(self.recovery.as_mut().unwrap(), "bottom {}", self.hidden)?;
                self.released_bottom = true;
            }
            set_work_area(work)?;
        }
        Ok(())
    }

    /// Gives the taskbar back while DeskIsle keeps running, as on exit.
    pub fn show(&mut self) -> Result<()> {
        if self.hidden.is_empty() {
            return Ok(());
        }
        restore(&self.hidden, self.released_bottom)?;
        self.hidden.clear();
        self.released_bottom = false;
        if let Some(pipe) = &mut self.recovery {
            // A blank line disarms the recovery process until the next hide.
            writeln!(pipe)?;
        }
        Ok(())
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
            && restore(&self.hidden, self.released_bottom).is_ok()
        {
            let _ = writeln!(pipe);
        }
    }
}

/// The recovery process has no windows. Restore only the last Explorer taskbar the
/// Dock actually took over; a replacement window keeps its own visibility state.
pub fn restore_on_disconnect() -> Result<()> {
    let mut hidden = String::new();
    let mut input = std::io::stdin().lock();
    let mut line = String::new();
    while input.read_line(&mut line)? != 0 {
        // Termination can interrupt a new record while the previous takeover is live.
        if let Some(record) = line.strip_suffix('\n') {
            hidden.clear();
            hidden.push_str(record);
        }
        line.clear();
    }
    // The helper has no Desktop/UI initialization. Its screen coordinates must still
    // use the same physical pixels as the process which expanded the work area.
    if let Some(id) = hidden.strip_prefix("bottom ") {
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)? };
        restore(id, true)
    } else {
        restore(&hidden, false)
    }
}

fn restore(hidden: &str, released_bottom: bool) -> Result<()> {
    if !hidden.is_empty()
        && let Some(hwnd) = explorer_tray()
        && identity(hwnd) == hidden
    {
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            set_visibility(hwnd, SWP_SHOWWINDOW)?;
        }
        if released_bottom && let Some(work) = adjusted_work_area(hwnd, false)? {
            set_work_area(work)?;
        }
    }
    Ok(())
}

fn adjusted_work_area(hwnd: HWND, release: bool) -> Result<Option<RECT>> {
    let monitor = super::monitor()?;
    let mut tray = RECT::default();
    match unsafe { GetWindowRect(hwnd, &mut tray) } {
        Err(e) if e.code() == HRESULT::from_win32(ERROR_INVALID_WINDOW_HANDLE.0) => {
            return Ok(None);
        }
        result => result?,
    }
    Ok(bottom_work_area(
        monitor.bounds,
        monitor.work,
        tray,
        release,
    ))
}

fn bottom_work_area(bounds: Rect, work: Rect, tray: RECT, release: bool) -> Option<RECT> {
    let screen_bottom = bounds.y + bounds.height;
    let (from, to) = if release {
        (tray.top, screen_bottom)
    } else {
        (screen_bottom, tray.top)
    };
    // Only replace the bottom edge when the current work area and the primary taskbar
    // account for the entire strip. Preserve other AppBars and concurrent layout changes.
    (work.y + work.height == from
        && tray.top > work.y
        && tray.top < screen_bottom
        && tray.bottom >= screen_bottom
        && tray.left <= work.x
        && tray.right >= work.x + work.width)
        .then_some(RECT {
            left: work.x,
            top: work.y,
            right: work.x + work.width,
            bottom: to,
        })
}

fn set_work_area(mut work: RECT) -> Result<()> {
    // On Windows 11, TRUE also reflows windows that are already maximized.
    // SPIF_SENDCHANGE makes Explorer publish its hidden taskbar's reservation again.
    unsafe {
        SystemParametersInfoW(
            SPI_SETWORKAREA,
            1,
            Some((&raw mut work).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_native_bottom_strip_changes_when_other_edges_are_reserved() {
        let screen = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let tray = RECT {
            left: 0,
            top: 1032,
            right: 1920,
            bottom: 1080,
        };
        let work = Rect {
            x: 40,
            y: 32,
            width: 1870,
            height: 1000,
        };
        let expanded = bottom_work_area(screen, work, tray, true).unwrap();
        assert_eq!(
            expanded,
            RECT {
                left: 40,
                top: 32,
                right: 1910,
                bottom: 1080
            }
        );
        let restored =
            bottom_work_area(screen, super::super::to_rect(expanded), tray, false).unwrap();
        assert_eq!(
            restored,
            RECT {
                bottom: 1032,
                ..expanded
            }
        );

        // A second bottom toolbar owns the space between the work area and taskbar.
        let other_toolbar = Rect {
            height: 980,
            ..work
        };
        assert_eq!(bottom_work_area(screen, other_toolbar, tray, true), None);
        assert_eq!(bottom_work_area(screen, other_toolbar, tray, false), None);
    }
}
