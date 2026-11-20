use std::cell::Cell;
use std::mem::size_of;

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::shell;

const WATCH_MS: u32 = 500;
const ANCHOR_TOLERANCE_DIP: i32 = 48;

#[derive(Clone, Copy)]
struct PendingMenu {
    pid: u32,
    registration: u64,
    anchor: POINT,
    work: RECT,
    tolerance: i32,
    started: u32,
    timer: usize,
    location_hook: HWINEVENTHOOK,
    candidate: Option<HWND>,
    matched: bool,
}

thread_local! {
    // The shell delivers out-of-context window events on the UI thread.
    static PENDING: Cell<Option<PendingMenu>> = const { Cell::new(None) };
    // A repeated right click can reuse a still-visible popup without another SHOW.
    static LAST_MENU: Cell<Option<(u64, HWND)>> = const { Cell::new(None) };
}

pub(super) fn watch(pid: u32, registration: u64, anchor: POINT) {
    stop();
    let monitor = unsafe { MonitorFromPoint(anchor, MONITOR_DEFAULTTONULL) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if monitor.is_invalid() || !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return;
    }
    // Legacy callbacks can open the menu at the pointer anywhere inside the
    // scaled bar, while version 4 callbacks get the anchor at its lower edge.
    let mut dpi = 0;
    let mut dpi_y = 0;
    let tolerance = if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y) }
        .is_ok()
        && dpi != 0
    {
        (ANCHOR_TOLERANCE_DIP * dpi as i32 + 48) / 96
    } else {
        ANCHOR_TOLERANCE_DIP.max(anchor.y - info.rcMonitor.top)
    };
    subscribe(pid, registration, anchor, info.rcWork, tolerance);
}

fn subscribe(pid: u32, registration: u64, anchor: POINT, work: RECT, tolerance: i32) {
    let timer = unsafe { SetTimer(None, 0, WATCH_MS, Some(end_discovery)) };
    if timer == 0 {
        return;
    }
    // Only this icon's process can supply a menu. Requests and cleanup run on the
    // same UI thread that receives the out-of-context events.
    let location_hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            None,
            Some(location_event),
            pid,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    let pending = PendingMenu {
        pid,
        registration,
        anchor,
        work,
        tolerance,
        started: unsafe { GetTickCount() },
        timer,
        location_hook,
        candidate: None,
        matched: false,
    };
    PENDING.set(Some(pending));
    if let Some((previous, hwnd)) = LAST_MENU.get()
        && previous == registration
    {
        check_window(pending, hwnd, true);
    }
}

pub(super) fn stop() {
    if let Some(pending) = PENDING.take() {
        if pending.timer != 0 {
            let _ = unsafe { KillTimer(None, pending.timer) };
        }
        if !pending.location_hook.is_invalid() {
            let _ = unsafe { UnhookWinEvent(pending.location_hook) };
        }
    }
}

unsafe extern "system" fn end_discovery(_: HWND, _: u32, timer: usize, _: u32) {
    if let Some(pending) = PENDING.get()
        && pending.timer == timer
    {
        if pending.matched {
            let _ = unsafe { KillTimer(None, timer) };
            PENDING.set(Some(PendingMenu {
                timer: 0,
                ..pending
            }));
        } else {
            stop();
        }
    }
}

unsafe extern "system" fn location_event(
    hook: HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _: u32,
    event_time: u32,
) {
    if object == OBJID_WINDOW.0
        && child == 0
        && PENDING
            .get()
            .is_some_and(|pending| pending.location_hook == hook)
    {
        window_event(EVENT_OBJECT_LOCATIONCHANGE, hwnd, event_time);
    }
}

pub(in crate::platform) fn window_event(event: u32, hwnd: HWND, event_time: u32) {
    if matches!(event, EVENT_OBJECT_HIDE | EVENT_OBJECT_DESTROY) {
        let previous = LAST_MENU
            .get()
            .is_some_and(|(_, previous)| previous == hwnd);
        let pending = PENDING
            .get()
            .filter(|pending| pending.candidate == Some(hwnd));
        if !previous && pending.is_none() {
            return;
        }
        // Out-of-context HIDE can arrive after the same HWND has reopened.
        if event == EVENT_OBJECT_HIDE && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return;
        }
        if previous {
            LAST_MENU.set(None);
        }
        if let Some(pending) = pending {
            if pending.timer == 0 {
                stop();
            } else {
                // The new click can hide the old popup and then show a fresh one.
                PENDING.set(Some(PendingMenu {
                    candidate: None,
                    matched: false,
                    ..pending
                }));
            }
        }
        return;
    }
    let Some(pending) = PENDING.get() else {
        return;
    };
    if pending.matched {
        if pending.candidate != Some(hwnd) {
            return;
        }
    } else if event_time.wrapping_sub(pending.started) > WATCH_MS {
        return;
    }
    // A location change alone can discover a known menu class. For an arbitrary
    // borderless popup require a SHOW in this click, or a previously verified HWND.
    let allow_custom = event == EVENT_OBJECT_SHOW || pending.candidate == Some(hwnd);
    check_window(pending, hwnd, allow_custom);
}

fn check_window(pending: PendingMenu, hwnd: HWND, allow_custom: bool) {
    if shell::window_process(hwnd) != pending.pid
        || !unsafe { IsWindowVisible(hwnd) }.as_bool()
        || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd
        || !is_menu_window(hwnd, allow_custom)
    {
        return;
    }

    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    let near = rect.right > rect.left
        && rect.bottom > rect.top
        && near_anchor(rect, pending.anchor, pending.work, pending.tolerance);
    // Keep a newly shown popup while its layout settles. Once one reaches the
    // anchor, follow only that window so opening a submenu cannot move it.
    if !pending.matched && (near || allow_custom) {
        PENDING.set(Some(PendingMenu {
            candidate: Some(hwnd),
            matched: near,
            ..pending
        }));
    }
    if !near {
        return;
    }
    LAST_MENU.set(Some((pending.registration, hwnd)));
    if let Some(at) = position(rect, pending.anchor, pending.work) {
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                at.x,
                at.y,
                0,
                0,
                SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSIZE | SWP_NOZORDER,
            )
        };
    }
}

fn is_menu_window(hwnd: HWND, allow_custom: bool) -> bool {
    let mut class = [0; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
    let class = String::from_utf16_lossy(&class[..len]);
    let style = WINDOW_STYLE(unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32);
    let ex_style = WINDOW_EX_STYLE(unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32);
    menu_style(&class, style, ex_style, allow_custom)
}

fn menu_style(
    class: &str,
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    allow_custom: bool,
) -> bool {
    if matches!(class, "#32768" | "LynxContextMenu") {
        return true;
    }
    // Qt uses a specific popup class, with optional version and shadow suffixes.
    let class = class.strip_prefix("Qt").map_or(class, |versioned| {
        versioned.trim_start_matches(|ch: char| ch.is_ascii_digit())
    });
    class.starts_with("QWindowPopup")
        || (allow_custom
            && style.contains(WS_POPUP)
            && !style.contains(WS_CAPTION)
            && (style & (WS_CHILD | WS_DISABLED)).0 == 0
            && (ex_style & (WS_EX_APPWINDOW | WS_EX_TRANSPARENT)).0 == 0
            && !matches!(class, "tooltips_class32" | "SysShadow" | "#32770"))
}

fn near_anchor(rect: RECT, anchor: POINT, work: RECT, tolerance: i32) -> bool {
    rect.left < work.right
        && rect.right > work.left
        && rect.top < work.bottom
        && (rect.left - tolerance..=rect.right + tolerance).contains(&anchor.x)
        && (rect.top - tolerance..=rect.bottom + tolerance).contains(&anchor.y)
}

fn position(rect: RECT, anchor: POINT, work: RECT) -> Option<POINT> {
    let width = rect.right - rect.left;
    let top = anchor.y.max(work.top);
    if rect.top >= top
        || width <= 0
        || rect.bottom <= rect.top
        || width > work.right - work.left
        || top >= work.bottom
        // Even an oversized menu gains visible rows by moving down unless it
        // already fills the full height below the bar.
        || rect.bottom >= work.bottom
    {
        return None;
    }
    Some(POINT {
        x: rect.left.clamp(work.left, work.right - width),
        y: top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::{Result, w};

    const WORK: RECT = RECT {
        left: -1920,
        top: -160,
        right: 0,
        bottom: 800,
    };
    const ANCHOR: POINT = POINT { x: -35, y: -160 };

    #[test]
    fn custom_popup_is_repositioned_after_the_app_finishes_its_layout() -> Result<()> {
        // Keep the test window outside the virtual desktop, including multi-monitor setups.
        let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } - 1000;
        let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } - 1000;
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("STATIC"),
                None,
                WS_POPUP | WS_BORDER | WS_VISIBLE,
                x + 250,
                y + 250,
                100,
                120,
                None,
                None,
                None,
                None,
            )?
        };
        let work = RECT {
            left: x - 500,
            right: x + 500,
            top: y,
            bottom: y + 500,
        };
        let right_click = || {
            stop();
            subscribe(
                std::process::id(),
                1,
                POINT { x, y },
                work,
                ANCHOR_TOLERANCE_DIP,
            );
        };
        right_click();
        window_event(EVENT_OBJECT_SHOW, hwnd, unsafe { GetTickCount() });
        let upward_layout = || -> Result<RECT> {
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    x,
                    y - 120,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
                )?;
            }
            // Dispatch the actual process-scoped location notification. A timeout
            // keeps a missing hook from hanging the test.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                let mut message = MSG::default();
                unsafe {
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        DispatchMessageW(&message);
                    }
                    let mut rect = RECT::default();
                    GetWindowRect(hwnd, &mut rect)?;
                    if rect.top == y || std::time::Instant::now() >= deadline {
                        return Ok(rect);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let first = upward_layout()?;
        right_click();
        // The reused HWND receives no new SHOW, and the app repositions it
        // again after the discovery timer has finished.
        let timer = PENDING.get().expect("menu watch").timer;
        unsafe {
            end_discovery(HWND::default(), WM_TIMER, timer, 0);
        }
        window_event(EVENT_OBJECT_HIDE, hwnd, unsafe { GetTickCount() });
        let repeated = upward_layout()?;
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        window_event(EVENT_OBJECT_HIDE, hwnd, unsafe { GetTickCount() });
        let stopped_on_hide = PENDING.get().is_none();
        unsafe { DestroyWindow(hwnd)? };
        stop();
        for result in [first, repeated] {
            assert_eq!((result.left, result.top, result.bottom), (x, y, y + 120));
        }
        assert!(stopped_on_hide);
        Ok(())
    }

    #[test]
    fn ordinary_windows_and_popup_decorations_do_not_qualify_as_custom_menus() {
        assert!(!menu_style(
            "App",
            WS_OVERLAPPEDWINDOW,
            WS_EX_APPWINDOW,
            true
        ));
        assert!(!menu_style("App", WS_POPUP, WS_EX_APPWINDOW, true));
        assert!(!menu_style("SysShadow", WS_POPUP, WS_EX_TOOLWINDOW, true));
        assert!(!menu_style(
            "tooltips_class32",
            WS_POPUP,
            WS_EX_TOPMOST,
            true
        ));
        assert!(!menu_style("App", WS_POPUP, WS_EX_TOOLWINDOW, false));
        assert!(menu_style(
            "LynxContextMenu",
            WS_POPUP,
            WS_EX_TOOLWINDOW,
            false
        ));
    }

    #[test]
    fn upward_menu_moves_below_the_bar_and_stays_inside_the_monitor() {
        let rect = RECT {
            left: -120,
            top: -440,
            right: 80,
            bottom: -160,
        };
        assert!(near_anchor(rect, ANCHOR, WORK, ANCHOR_TOLERANCE_DIP));
        assert_eq!(
            position(rect, ANCHOR, WORK),
            Some(POINT { x: -200, y: -160 })
        );

        let contained = RECT {
            left: -240,
            right: -20,
            ..rect
        };
        assert_eq!(
            position(contained, ANCHOR, WORK),
            Some(POINT { x: -240, y: -160 }),
        );
    }

    #[test]
    fn existing_downward_menu_keeps_its_position() {
        let rect = RECT {
            left: -240,
            top: -157,
            right: -20,
            bottom: 123,
        };
        assert!(near_anchor(rect, ANCHOR, WORK, ANCHOR_TOLERANCE_DIP));
        assert_eq!(position(rect, ANCHOR, WORK), None);
    }

    #[test]
    fn oversized_menu_moves_when_more_of_it_can_be_shown() {
        let rect = RECT {
            left: -240,
            top: -190,
            right: -20,
            bottom: 800,
        };
        assert_eq!(position(rect, ANCHOR, WORK), None);
        // At 300% scale a cursor at the top of the bar is over 48px from the anchor.
        let upward = RECT {
            top: -1500,
            bottom: ANCHOR.y - 90,
            ..rect
        };
        assert!(near_anchor(upward, ANCHOR, WORK, ANCHOR_TOLERANCE_DIP * 3));
        assert_eq!(
            position(upward, ANCHOR, WORK),
            Some(POINT { x: -240, y: -160 })
        );
    }

    #[test]
    fn popup_far_from_the_clicked_icon_is_ignored() {
        let rect = RECT {
            left: -1750,
            top: -190,
            right: -1530,
            bottom: 90,
        };
        assert!(!near_anchor(rect, ANCHOR, WORK, ANCHOR_TOLERANCE_DIP));
    }
}
