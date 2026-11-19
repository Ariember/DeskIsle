#![cfg(windows)]

use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetMenuBarInfo, GetMenuItemCount, GetMenuStringW,
    GetSystemMetrics, GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, MENUBARINFO, MF_BYPOSITION, OBJID_CLIENT, SM_CXSCREEN, SMTO_ABORTIFHUNG,
    SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageTimeoutW,
    SystemParametersInfoW, WM_CANCELMODE, WM_CLOSE, WM_LBUTTONDOWN, WM_LBUTTONUP, WS_EX_NOACTIVATE,
};
use windows::core::BOOL;

struct TestInstance(Child);

impl Drop for TestInstance {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() && self.0.kill().is_ok() {
            let _ = self.0.wait();
        }
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
            _ => {
                let mut class = [0; 32];
                let len = unsafe { GetClassNameW(hwnd, &mut class) };
                if class[..len as usize]
                    .iter()
                    .copied()
                    .eq("#32768".encode_utf16())
                {
                    bars.menu = Some(hwnd);
                }
            }
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

    let mut app = TestInstance(
        Command::new(env!("CARGO_BIN_EXE_deskisle"))
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let (status, dock) = loop {
        assert_eq!(
            app.0.try_wait().unwrap(),
            None,
            "exited before showing both bars"
        );
        let bars = Bars::visible_for(app.0.id());
        if let (Some(status), Some(dock)) = (bars.status, bars.dock) {
            break (status, dock);
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for both visible bars"
        );
        thread::sleep(Duration::from_millis(50));
    };

    for (hwnd, is_top) in [(status, true), (dock, false)] {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }.unwrap();
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
            rect.left >= work.left && rect.right <= work.right,
            "{rect:?}"
        );
        assert!(
            rect.top >= work.top && rect.bottom <= work.bottom,
            "{rect:?}"
        );
        assert!(
            ((rect.left - work.left) - (work.right - rect.right)).abs() <= 1,
            "{rect:?}"
        );
        let gap = work.bottom - rect.bottom;
        assert!(
            gap < rect.bottom - rect.top,
            "bar at wrong screen edge: {rect:?}"
        );
        let style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
        assert_ne!(style & WS_EX_NOACTIVATE.0, 0, "bar can take focus");
    }
    let pid = app.0.id();
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
    let mut info = MENUBARINFO {
        cbSize: size_of::<MENUBARINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMenuBarInfo(popup, OBJID_CLIENT, 0, &mut info) }.unwrap();
    let count = unsafe { GetMenuItemCount(Some(info.hMenu)) };
    assert!(count > 0, "empty top bar menu");
    let last = count as u32 - 1;
    let mut title = [0; 64];
    let len = unsafe { GetMenuStringW(info.hMenu, last, Some(&mut title), MF_BYPOSITION) };
    assert_eq!(
        String::from_utf16_lossy(&title[..len as usize]),
        "退出 DeskIsle"
    );

    send_to_child(pid, status, WM_CANCELMODE, WPARAM(0), LPARAM(0));
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
    unsafe { SetThreadDpiAwarenessContext(previous) };
}
