#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BLACK_BRUSH, FillRect, GdiFlush, GetDC, GetPixel, GetStockObject, HBRUSH, ReleaseDC,
    WHITE_BRUSH,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentProcess, PROCESS_NAME_WIN32, QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::Shell::{ABN_FULLSCREENAPP, IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowExW,
    GWL_EXSTYLE, GetClientRect, GetCursorPos, GetForegroundWindow, GetShellWindow,
    GetSystemMetrics, GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, IsZoomed, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SM_CXSCREEN,
    SM_CYSCREEN, SMTO_ABORTIFHUNG, SPI_GETWORKAREA, SPI_SETWORKAREA, SW_HIDE, SW_MAXIMIZE,
    SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageTimeoutW, SetCursorPos, SetForegroundWindow,
    SetWindowPos, ShowWindow, SystemParametersInfoW, WM_APP, WM_CANCELMODE, WM_CLOSE,
    WM_DISPLAYCHANGE, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_SETTINGCHANGE, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_OVERLAPPEDWINDOW, WS_POPUP,
};
use windows::core::{BOOL, HSTRING, Interface, PWSTR, w};

struct TestProfile(PathBuf);

impl TestProfile {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("DeskIsle-bars-{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("DeskIsle")).unwrap();
        Self(root)
    }
}

impl Drop for TestProfile {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("启动测试.lnk"));
        let _ = fs::remove_file(self.0.join("launched.txt"));
        let _ = fs::remove_file(self.0.join("DeskIsle/launchpad.txt"));
        let _ = fs::remove_file(self.0.join("DeskIsle/pins.txt"));
        let _ = fs::remove_dir(self.0.join("DeskIsle"));
        let _ = fs::remove_dir(&self.0);
    }
}

struct TestInstance(Child);

impl TestInstance {
    fn start(appdata: &Path) -> Self {
        Self(
            Command::new(env!("CARGO_BIN_EXE_deskisle"))
                .env("APPDATA", appdata)
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
            poll_desktop();
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
    work: RECT,
}

impl Drop for TaskbarSnapshot {
    fn drop(&mut self) {
        // Also restore the desktop when a failed assertion kills the test child.
        if explorer_taskbar() != Some(self.hwnd) {
            return;
        }
        if unsafe { IsWindowVisible(self.hwnd) }.as_bool() != self.visible {
            let show = if self.visible {
                SW_SHOWNOACTIVATE
            } else {
                SW_HIDE
            };
            let _ = unsafe { ShowWindow(self.hwnd, show) };
        }
        if primary_work_area().is_ok_and(|work| work != self.work) {
            let _ = set_work_area(self.work);
        }
    }
}

fn primary_work_area() -> windows::core::Result<RECT> {
    let mut work = RECT::default();
    unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&raw mut work).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )?;
    }
    Ok(work)
}

fn set_work_area(mut work: RECT) -> windows::core::Result<()> {
    unsafe {
        SystemParametersInfoW(
            SPI_SETWORKAREA,
            1,
            Some((&raw mut work).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
    }
}

fn window_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.unwrap();
    rect
}

struct TestWindow(HWND);

unsafe extern "system" fn probe_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

impl TestWindow {
    fn create() -> Self {
        unsafe {
            let instance = GetModuleHandleW(None).unwrap().into();
            let class = w!("DeskIsle.TestWindow");
            let wc = WNDCLASSW {
                hInstance: instance,
                lpszClassName: class,
                lpfnWndProc: Some(probe_proc),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&wc), 0);
            let hwnd = CreateWindowExW(
                Default::default(),
                class,
                w!("DeskIsle maximization test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                360,
                240,
                None,
                None,
                Some(instance),
                None,
            )
            .unwrap();
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            assert!(IsZoomed(hwnd).as_bool());
            Self(hwnd)
        }
    }

    /// A borderless window over the whole primary monitor in the foreground, the way a
    /// full-screen game runs. `create` registers its class.
    fn cover() -> Self {
        unsafe {
            let instance = GetModuleHandleW(None).unwrap().into();
            let hwnd = CreateWindowExW(
                Default::default(),
                w!("DeskIsle.TestWindow"),
                w!("DeskIsle full-screen test"),
                WS_POPUP,
                0,
                0,
                GetSystemMetrics(SM_CXSCREEN),
                GetSystemMetrics(SM_CYSCREEN),
                None,
                None,
                Some(instance),
                None,
            )
            .unwrap();
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            Self(hwnd)
        }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn poll_desktop() {
    // Deliver work-area broadcasts to the test window while waiting for the child.
    let mut msg = MSG::default();
    while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
        unsafe { DispatchMessageW(&msg) };
    }
    thread::sleep(Duration::from_millis(25));
}

fn wait_until(description: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "{description}");
        poll_desktop();
    }
}

struct Bars {
    pid: u32,
    status: Option<HWND>,
    dock: Option<HWND>,
    menu: Option<HWND>,
    launchpad: Option<HWND>,
    preview: Option<HWND>,
}

impl Bars {
    fn visible_for(pid: u32) -> Self {
        let mut bars = Self {
            pid,
            status: None,
            dock: None,
            menu: None,
            launchpad: None,
            preview: None,
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
            "DeskIsle 启动台" => bars.launchpad = Some(hwnd),
            "DeskIsle Preview" => bars.preview = Some(hwnd),
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

fn hovering_the_probe_shows_its_preview(pid: u32, dock: HWND, probe: HWND) {
    struct SavedCursor(POINT);
    impl Drop for SavedCursor {
        fn drop(&mut self) {
            let _ = unsafe { SetCursorPos(self.0.x, self.0.y) };
        }
    }

    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.unwrap();
    let _saved = SavedCursor(point);
    let rect = window_rect(dock);
    let dpi = unsafe { GetDpiForWindow(dock) } as i32;
    // The probe is the first pinned app, one 52-DIP slot after the launchpad.
    let x = rect.left + (84 * dpi + 48) / 96;
    let y = (rect.top + rect.bottom) / 2;
    unsafe { SetCursorPos(x, y) }.unwrap();
    wait_until(
        "hovering the pinned test window did not show its preview",
        || {
            Bars::visible_for(pid)
                .preview
                .is_some_and(|hwnd| window_rect(hwnd).bottom <= rect.top)
        },
    );
    // Change only the middle of the source. A blank host or a static image cannot
    // reproduce both colors at the center of the thumbnail above the Dock.
    let mut client = RECT::default();
    unsafe { GetClientRect(probe, &mut client) }.unwrap();
    let patch = RECT {
        left: client.right / 3,
        right: client.right * 2 / 3,
        top: client.bottom / 3,
        bottom: client.bottom * 2 / 3,
    };
    let preview = window_rect(Bars::visible_for(pid).preview.unwrap());
    for brush in [WHITE_BRUSH, BLACK_BRUSH] {
        unsafe {
            let dc = GetDC(Some(probe));
            let filled = FillRect(dc, &patch, HBRUSH(GetStockObject(brush).0));
            let _ = GdiFlush();
            ReleaseDC(Some(probe), dc);
            assert_ne!(filled, 0);
        }
        wait_until(
            "preview did not reproduce the source window's color change",
            || {
                let pixel = unsafe {
                    let dc = GetDC(None);
                    let color = GetPixel(
                        dc,
                        (preview.left + preview.right) / 2,
                        (preview.top + preview.bottom) / 2,
                    );
                    ReleaseDC(None, dc);
                    color.0
                };
                let [r, g, b, _] = pixel.to_le_bytes();
                pixel != u32::MAX
                    && if brush == WHITE_BRUSH {
                        r.min(g).min(b) >= 215
                    } else {
                        r.max(g).max(b) <= 40
                    }
            },
        );
    }
    unsafe { SetCursorPos(x, rect.top - 1) }.unwrap();
    wait_until("moving off the Dock did not hide the preview", || {
        Bars::visible_for(pid).preview.is_none()
    });
}

fn launchpad_reopens_saved_shortcut(pid: u32, dock: HWND, profile: &Path) {
    let open = || {
        let dpi = unsafe { GetDpiForWindow(dock) } as i32;
        let center = (32 * dpi + 48) / 96;
        let point = LPARAM(((center << 16) | center) as isize);
        send_to_child(pid, dock, WM_LBUTTONDOWN, WPARAM(1), point);
        send_to_child(pid, dock, WM_LBUTTONUP, WPARAM(0), point);
        wait_until("first Dock icon did not open the launchpad", || {
            Bars::visible_for(pid).launchpad.is_some()
        });
        Bars::visible_for(pid).launchpad.unwrap()
    };
    let first = open();
    let rect = window_rect(first);
    assert!(rect.bottom > rect.top && rect.right > rect.left);
    assert!(
        rect.bottom <= window_rect(dock).top,
        "launchpad covers the Dock"
    );
    send_to_child(
        pid,
        first,
        WM_KEYDOWN,
        WPARAM(VK_ESCAPE.0.into()),
        LPARAM(0),
    );
    wait_until("Escape did not close the launchpad", || {
        Bars::visible_for(pid).launchpad.is_none()
    });

    // Seed the isolated profile with a real shortcut. Its command needs both its
    // arguments and working directory for the marker to appear in the right place.
    let shortcut = profile.join("启动测试.lnk");
    let cmd = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/cmd.exe");
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.SetPath(&HSTRING::from(cmd.as_path())).unwrap();
            link.SetArguments(w!("/d /c echo launched>launched.txt"))
                .unwrap();
            link.SetWorkingDirectory(&HSTRING::from(profile)).unwrap();
            link.SetShowCmd(SW_HIDE).unwrap();
            link.cast::<IPersistFile>()
                .unwrap()
                .Save(&HSTRING::from(shortcut.as_path()), true)
                .unwrap();
        }
        CoUninitialize();
    }
    fs::write(
        profile.join("DeskIsle/launchpad.txt"),
        format!("{}\n", shortcut.display()),
    )
    .unwrap();
    let reopened = open();
    send_to_child(
        pid,
        reopened,
        WM_KEYDOWN,
        WPARAM(VK_RETURN.0.into()),
        LPARAM(0),
    );
    wait_until("reopened launchpad did not run the saved shortcut", || {
        fs::read_to_string(profile.join("launched.txt")).is_ok_and(|text| text.trim() == "launched")
    });
    wait_until("launchpad stayed open after launching the shortcut", || {
        Bars::visible_for(pid).launchpad.is_none()
    });
}

#[test]
#[ignore = "requires an interactive Windows desktop"]
fn bars_open_in_the_primary_work_area_and_close_together() {
    let read_work = || primary_work_area().expect("read primary work area");
    // This test thread reads physical screen coordinates on mixed-DPI desktops.
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    assert!(!previous.0.is_null(), "set test thread DPI context");
    let work = read_work();
    let hwnd = explorer_taskbar().expect("interactive desktop with an Explorer taskbar");
    let taskbar = TaskbarSnapshot {
        hwnd,
        visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
        work,
    };
    assert!(
        taskbar.visible,
        "test requires an initially visible Explorer taskbar"
    );
    let taskbar_rect = window_rect(hwnd);
    let screen_bottom = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let bottom_taskbar = taskbar.visible
        && taskbar_rect.top == work.bottom
        && taskbar_rect.bottom >= screen_bottom
        && taskbar_rect.left <= work.left
        && taskbar_rect.right >= work.right;
    let expected_bottom = if bottom_taskbar {
        screen_bottom
    } else {
        work.bottom
    };
    let probe = TestWindow::create();
    let initial = window_rect(probe.0);

    let profile = TestProfile::new();
    // Pin the probe using the same Win32 image path that the Dock sees on enumeration.
    let mut executable = vec![0; 32768];
    let mut len = executable.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            GetCurrentProcess(),
            PROCESS_NAME_WIN32,
            PWSTR(executable.as_mut_ptr()),
            &mut len,
        )
    }
    .unwrap();
    fs::write(
        profile.0.join("DeskIsle/pins.txt"),
        String::from_utf16(&executable[..len as usize]).unwrap(),
    )
    .unwrap();
    let mut app = TestInstance::start(&profile.0);
    let (status, dock) = app.wait_for_bars();

    wait_until("native taskbar still visible with the Dock running", || {
        !unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool()
    });
    wait_until(
        "maximized work area must span from the status bar to the available bottom edge",
        || {
            let current = read_work();
            current.top == window_rect(status).bottom && current.bottom == expected_bottom
        },
    );
    let dock_work = read_work();
    // GetWindowRect includes the invisible resize borders. Compare their offsets
    // against the same window's original maximized frame at this monitor's DPI.
    let fits_work = || {
        let rect = window_rect(probe.0);
        unsafe { IsZoomed(probe.0) }.as_bool()
            && rect.top - dock_work.top == initial.top - work.top
            && rect.bottom - dock_work.bottom == initial.bottom - work.bottom
    };
    wait_until(
        "already-maximized window did not follow the work area",
        fits_work,
    );
    unsafe {
        let _ = ShowWindow(probe.0, SW_RESTORE);
        assert!(!IsZoomed(probe.0).as_bool());
        let _ = ShowWindow(probe.0, SW_MAXIMIZE);
    }
    wait_until("new maximization did not use the work area", fits_work);
    wait_until("Dock did not settle at the bottom edge", || {
        let dpi = unsafe { GetDpiForWindow(dock) } as i32;
        let rect = window_rect(dock);
        let inset = ((8 * dpi + 48) / 96)
            .min((dock_work.bottom - dock_work.top) - (rect.bottom - rect.top));
        rect.bottom == expected_bottom - inset
    });
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
    hovering_the_probe_shows_its_preview(pid, dock, probe.0);
    launchpad_reopens_saved_shortcut(pid, dock, &profile.0);
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
    // The shell reports the desktop as full screen too, so the bars only step aside
    // while an application covering the monitor is in front.
    let cover = TestWindow::cover();
    wait_until("full-screen window did not reach the foreground", || {
        (unsafe { GetForegroundWindow() }) == cover.0
    });
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
    drop(cover);
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
        poll_desktop();
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
        poll_desktop();
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
        poll_desktop();
    }
    wait_until("native taskbar visibility not restored after exit", || {
        unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool() == taskbar.visible
    });
    let restored = || read_work() == work && window_rect(probe.0) == initial;
    wait_until(
        "work area and maximized window not restored after exit",
        restored,
    );
    // A second launch must leave the active owner's windows and taskbar intact.
    let mut owner = TestInstance::start(&profile.0);
    let (status, dock) = owner.wait_for_bars();
    wait_until("restarted Dock did not hide the native taskbar", || {
        !unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool()
    });
    let mut duplicate = TestInstance::start(&profile.0);
    wait_until("second launch did not exit", || {
        duplicate.0.try_wait().unwrap().is_some()
    });
    assert!(duplicate.0.wait().unwrap().success());
    assert_eq!(owner.0.try_wait().unwrap(), None);
    let bars = Bars::visible_for(owner.0.id());
    assert_eq!((bars.status, bars.dock), (Some(status), Some(dock)));
    assert!(!unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool());

    wait_until(
        "second owner did not take over the work area before termination",
        || read_work() == dock_work && fits_work(),
    );
    assert_ne!(
        dock_work, work,
        "forced-exit test needs an active reservation"
    );
    if bottom_taskbar {
        // A utility explicitly reserves the same strip as the hidden native taskbar.
        // Its setting notification must move the Dock without reclaiming the strip.
        let external = RECT {
            bottom: taskbar_rect.top,
            ..dock_work
        };
        let old_bottom = window_rect(dock).bottom;
        set_work_area(external).unwrap();
        send_to_child(
            owner.0.id(),
            status,
            WM_SETTINGCHANGE,
            WPARAM(SPI_SETWORKAREA.0 as usize),
            LPARAM(0),
        );
        wait_until(
            "Dock did not honor another utility's bottom reservation",
            || {
                read_work() == external
                    && window_rect(dock).bottom == old_bottom - (dock_work.bottom - external.bottom)
            },
        );
        set_work_area(dock_work).unwrap();
        send_to_child(
            owner.0.id(),
            status,
            WM_SETTINGCHANGE,
            WPARAM(SPI_SETWORKAREA.0 as usize),
            LPARAM(0),
        );
        wait_until(
            "original takeover not active before forced termination",
            || window_rect(dock).bottom == old_bottom && fits_work(),
        );
    }
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    // Start immediately after the main process dies. Its helper can still be
    // restoring Explorer, and the replacement must complete its own takeover.
    let mut replacement = TestInstance::start(&profile.0);
    let (replacement_status, _) = replacement.wait_for_bars();
    wait_until(
        "rapid relaunch did not take over both desktop edges",
        || {
            !unsafe { IsWindowVisible(hwnd) }.as_bool()
                && read_work().top == window_rect(replacement_status).bottom
                && read_work().bottom == expected_bottom
                && fits_work()
        },
    );
    replacement.0.kill().unwrap();
    replacement.0.wait().unwrap();
    wait_until(
        "native taskbar visibility not restored after forced termination",
        || unsafe { IsWindowVisible(taskbar.hwnd) }.as_bool() == taskbar.visible,
    );
    wait_until(
        "work area and maximized window not restored after forced termination",
        restored,
    );
    if bottom_taskbar {
        // Another utility already hid Explorer but kept its original reservation.
        // The Dock uses the vacant strip locally, without taking ownership of either.
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        set_work_area(work).unwrap();
        let mut prehidden = TestInstance::start(&profile.0);
        let (status, dock) = prehidden.wait_for_bars();
        wait_until(
            "Dock did not use space above an externally hidden taskbar",
            || {
                let dpi = unsafe { GetDpiForWindow(dock) } as i32;
                !unsafe { IsWindowVisible(hwnd) }.as_bool()
                    && read_work().bottom == work.bottom
                    && window_rect(dock).bottom == screen_bottom - (8 * dpi + 48) / 96
            },
        );
        send_to_child(prehidden.0.id(), status, WM_CLOSE, WPARAM(0), LPARAM(0));
        wait_until("prehidden Dock did not exit", || {
            prehidden.0.try_wait().unwrap().is_some()
        });
        assert!(prehidden.0.wait().unwrap().success());
        wait_until("prehidden work area not restored after exit", restored);
        assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
    }
    drop(probe);
    drop(taskbar);
    unsafe { SetThreadDpiAwarenessContext(previous) };
}
