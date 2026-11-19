#![cfg(windows)]

use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Shell::ABN_FULLSCREENAPP;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GWL_EXSTYLE, GetShellWindow, GetSystemMetrics, GetWindowLongW,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SM_CXSCREEN,
    SM_CYSCREEN, SMTO_ABORTIFHUNG, SPI_GETWORKAREA, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageTimeoutW,
    SetWindowPos, ShowWindow, SystemParametersInfoW, WM_APP, WM_CANCELMODE, WM_CLOSE,
    WM_DISPLAYCHANGE, WM_LBUTTONDOWN, WM_LBUTTONUP, WS_EX_NOACTIVATE,
};
use windows::core::{BOOL, w};

struct TestInstance(Child);

impl TestInstance {
    fn start() -> Self {
        Self(
            Command::new(env!("CARGO_BIN_EXE_deskisle"))
                .spawn()
                .unwrap(),
        )
    }

    fn wait_for_bars(&mut self) -> (HWND, HWND) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert_eq!(
                self.0.try_wait().unwrap(),
                None,
                "exited before showing both bars"
            );
            let bars = Bars::visible_for(self.0.id());
            if let (Some(status), Some(dock)) = (bars.status, bars.dock) {
                return (status, dock);
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for both visible bars"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for TestInstance {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() && self.0.kill().is_ok() {
            let _ = self.0.wait();
        }
    }
}

fn explorer_taskbar() -> Option<HWND> {
    let shell = unsafe { GetShellWindow() };
    let mut shell_pid = 0;
    unsafe { GetWindowThreadProcessId(shell, Some(&mut shell_pid)) };
    let mut after = None;
    loop {
        let hwnd = unsafe { FindWindowExW(None, after, w!("Shell_TrayWnd"), None) }.ok()?;
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid != 0 && pid == shell_pid {
            return Some(hwnd);
        }
        after = Some(hwnd);
    }
}

struct TaskbarSnapshot {
    hwnd: HWND,
    visible: bool,
}

impl Drop for TaskbarSnapshot {
    fn drop(&mut self) {
        // Also restore the desktop when a failed assertion kills the test child.
        if explorer_taskbar() == Some(self.hwnd)
            && unsafe { IsWindowVisible(self.hwnd) }.as_bool() != self.visible
        {
            let show = if self.visible {
                SW_SHOWNOACTIVATE
            } else {
                SW_HIDE
            };
            let _ = unsafe { ShowWindow(self.hwnd, show) };
        }
    }
}

fn window_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.unwrap();
    rect
}

fn wait_until(description: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "{description}");
        thread::sleep(Duration::from_millis(25));
    }
}

struct Bars {
    pid: u32,
    status: Option<HWND>,
    dock: Option<HWND>,
    menu: Option<HWND>,
}

impl Bars {
    fn visible_for(pid: u32) -> Self {
        let mut bars = Self {
            pid,
            status: None,
            dock: None,
            menu: None,
        };
        unsafe { EnumWindows(Some(find_bar), LPARAM((&raw mut bars) as isize)) }.unwrap();
        bars
    }
}

unsafe extern "system" fn find_bar(hwnd: HWND, data: LPARAM) -> BOOL {
    // EnumWindows invokes this synchronously on the caller's thread.
    let bars = unsafe { &mut *(data.0 as *mut Bars) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == bars.pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        let mut title = [0; 48];
        let len = unsafe { GetWindowTextW(hwnd, &mut title) };
        match String::from_utf16_lossy(&title[..len as usize]).as_str() {
            "DeskIsle Status" => bars.status = Some(hwnd),
            "DeskIsle Dock" => bars.dock = Some(hwnd),
            "DeskIsle Menu" => bars.menu = Some(hwnd),
            _ => {}
        }
    }
    BOOL(1)
}

fn send_to_child(pid: u32, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) {
    let mut owner = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
    assert_eq!(owner, pid, "target window no longer belongs to our child");
    let sent = unsafe { SendMessageTimeoutW(hwnd, msg, wp, lp, SMTO_ABORTIFHUNG, 2000, None) };
    assert_ne!(sent.0, 0, "child did not process message {msg:#x}");
}

#[test]
#[ignore = "requires an interactive Windows desktop"]
fn bars_open_in_the_primary_work_area_and_close_together() {
    let read_work = || {
        let mut work = RECT::default();
        unsafe {
            SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some((&raw mut work).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
            )
        }
        .expect("read primary work area");
        work
    };
    // This test thread reads physical screen coordinates on mixed-DPI desktops.
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!previous.0.is_null(), "set test thread DPI context");
    let work = read_work();
    let hwnd = explorer_taskbar().expect("interactive desktop with an Explorer taskbar");
    let taskbar = TaskbarSnapshot {
        hwnd,
        visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
    };
    let taskbar_rect = window_rect(hwnd);

    let mut app = TestInstance::start();
    let (status, dock) = app.wait_for_bars();

    wait_until("native taskbar still visible with the Dock running", || {
        !unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool()
    });
    let mut dock_work = read_work();
    if taskbar_rect.top == dock_work.bottom
        && taskbar_rect.bottom == unsafe { GetSystemMetrics(SM_CYSCREEN) }
        && taskbar_rect.left <= dock_work.left
        && taskbar_rect.right >= dock_work.right
    {
        dock_work.bottom = taskbar_rect.bottom;
    }
    for (hwnd, is_top) in [(status, true), (dock, false)] {
        let rect = window_rect(hwnd);
        assert!(rect.right > rect.left && rect.bottom > rect.top, "{rect:?}");
        if is_top {
            // The status bar spans the primary monitor and reserves its own strip at the
            // top, so it may sit above the work area measured before launch.
            let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            assert_eq!((rect.left, rect.right), (0, width), "{rect:?}");
            assert!(rect.top >= 0 && rect.top <= work.top, "{rect:?}");
            let style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
            assert_ne!(style & WS_EX_NOACTIVATE.0, 0, "bar can take focus");
            continue;
        }
        assert!(
            rect.left >= dock_work.left && rect.right <= dock_work.right,
            "{rect:?}"
        );
        assert!(
            rect.top >= dock_work.top && rect.bottom <= dock_work.bottom,
            "{rect:?}"
        );
        assert!(
            ((rect.left - dock_work.left) - (dock_work.right - rect.right)).abs() <= 1,
            "{rect:?}"
        );
        let dpi = unsafe { GetDpiForWindow(hwnd) } as i32;
        let inset = ((8 * dpi + 48) / 96)
            .min((dock_work.bottom - dock_work.top) - (rect.bottom - rect.top));
        assert_eq!(
            dock_work.bottom - rect.bottom,
            inset,
            "wrong Dock bottom inset: {rect:?}"
        );
        let style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
        assert_ne!(style & WS_EX_NOACTIVATE.0, 0, "bar can take focus");
    }
    let pid = app.0.id();
    // Send Explorer's AppBar notification to the window that registered for it.
    let fullscreen = |open| {
        send_to_child(
            pid,
            status,
            WM_APP + 2,
            WPARAM(ABN_FULLSCREENAPP as usize),
            LPARAM(isize::from(open)),
        );
    };
    fullscreen(true);
    wait_until("full-screen notification did not hide both bars", || {
        !unsafe { IsWindowVisible(status) }.as_bool() && !unsafe { IsWindowVisible(dock) }.as_bool()
    });
    // Wait for a visible effect of processing the layout event before checking that the
    // repositioned Dock stayed hidden. Returning from SendMessage only queues the event.
    let dock_rect = window_rect(dock);
    unsafe {
        SetWindowPos(
            dock,
            None,
            dock_rect.left,
            dock_rect.top - 16,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    }
    .unwrap();
    send_to_child(pid, status, WM_DISPLAYCHANGE, WPARAM(0), LPARAM(0));
    wait_until("display change did not reposition the Dock", || {
        window_rect(dock).top == dock_rect.top
    });
    assert!(!unsafe { IsWindowVisible(status) }.as_bool());
    assert!(!unsafe { IsWindowVisible(dock) }.as_bool());
    fullscreen(false);
    wait_until("leaving full screen did not restore both bars", || {
        unsafe { IsWindowVisible(status) }.as_bool() && unsafe { IsWindowVisible(dock) }.as_bool()
    });
    assert!(!unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool());
    let point = LPARAM(18 << 16 | 18);
    send_to_child(pid, status, WM_LBUTTONDOWN, WPARAM(1), point);
    send_to_child(pid, status, WM_LBUTTONUP, WPARAM(0), point);
    let deadline = Instant::now() + Duration::from_secs(5);
    let popup = loop {
        if let Some(menu) = Bars::visible_for(pid).menu {
            break menu;
        }
        assert!(Instant::now() < deadline, "top bar did not open its menu");
        thread::sleep(Duration::from_millis(25));
    };
    // The status bar's own menu activates itself so clicks elsewhere dismiss it.
    let style = unsafe { GetWindowLongW(popup, GWL_EXSTYLE) } as u32;
    assert_eq!(style & WS_EX_NOACTIVATE.0, 0, "menu cannot take focus");
    let mut rect = RECT::default();
    unsafe { GetWindowRect(popup, &mut rect) }.unwrap();
    assert!(rect.bottom > rect.top && rect.right > rect.left, "{rect:?}");

    send_to_child(pid, popup, WM_CANCELMODE, WPARAM(0), LPARAM(0));
    let deadline = Instant::now() + Duration::from_secs(5);
    while Bars::visible_for(pid).menu.is_some() {
        assert!(Instant::now() < deadline, "top bar menu did not close");
        thread::sleep(Duration::from_millis(25));
    }
    send_to_child(pid, dock, WM_CLOSE, WPARAM(0), LPARAM(0));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(exit) = app.0.try_wait().unwrap() {
            assert!(exit.success(), "DeskIsle exited with {exit}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "DeskIsle did not exit after WM_CLOSE"
        );
        thread::sleep(Duration::from_millis(50));
    }
    wait_until("native taskbar visibility not restored after exit", || {
        unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool() == taskbar.visible
    });
    // Leaving releases the reserved strip.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = read_work();
        if (now.left, now.top, now.right, now.bottom)
            == (work.left, work.top, work.right, work.bottom)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "work area not restored after exit"
        );
        thread::sleep(Duration::from_millis(50));
    }
    // A second launch must leave the active owner's windows and taskbar intact.
    let mut owner = TestInstance::start();
    let (status, dock) = owner.wait_for_bars();
    wait_until("restarted Dock did not hide the native taskbar", || {
        !unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool()
    });
    let mut duplicate = TestInstance::start();
    wait_until("second launch did not exit", || {
        duplicate.0.try_wait().unwrap().is_some()
    });
    assert!(duplicate.0.wait().unwrap().success());
    assert_eq!(owner.0.try_wait().unwrap(), None);
    let bars = Bars::visible_for(owner.0.id());
    assert_eq!((bars.status, bars.dock), (Some(status), Some(dock)));
    assert!(!unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool());

    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    wait_until(
        "native taskbar visibility not restored after forced termination",
        || unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool() == taskbar.visible,
    );
    unsafe { SetThreadDpiAwarenessContext(previous) };
}
