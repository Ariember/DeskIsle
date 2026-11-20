use std::cell::Cell;
use std::mem::size_of;

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::shell;

const WATCH_MS: u32 = 500;
const ANCHOR_TOLERANCE_PX: i32 = 48;

#[derive(Clone, Copy)]
struct PendingMenu {
    pid: u32,
    anchor: POINT,
    work: RECT,
    started: u32,
}

thread_local! {
    // The shell delivers out-of-context window events on the UI thread.
    static PENDING: Cell<Option<PendingMenu>> = const { Cell::new(None) };
}

pub(super) fn watch(pid: u32, anchor: POINT) {
    stop();
    let monitor = unsafe { MonitorFromPoint(anchor, MONITOR_DEFAULTTONULL) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if monitor.is_invalid() || !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return;
    }
    PENDING.set(Some(PendingMenu {
        pid,
        anchor,
        work: info.rcWork,
        started: unsafe { GetTickCount() },
    }));
}

pub(super) fn stop() {
    PENDING.set(None);
}

pub(in crate::platform) fn shown(hwnd: HWND, event_time: u32) {
    let Some(pending) = PENDING.get() else {
        return;
    };
    if unsafe { GetTickCount() }.wrapping_sub(pending.started) > WATCH_MS {
        stop();
        return;
    }
    if event_time.wrapping_sub(pending.started) > WATCH_MS
        || shell::window_process(hwnd) != pending.pid
        || !unsafe { IsWindowVisible(hwnd) }.as_bool()
        || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd
        || !is_menu_window(hwnd)
    {
        return;
    }

    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err()
        || !near_anchor(rect, pending.anchor, pending.work)
    {
        return;
    }
    // An already-correct menu also consumes the watch so a submenu cannot match it.
    stop();
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

fn is_menu_window(hwnd: HWND) -> bool {
    let mut class = [0; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
    let class = String::from_utf16_lossy(&class[..len]);
    if class == "#32768" {
        return true;
    }
    // Qt uses a specific popup class, with optional version and shadow suffixes.
    let class = class
        .strip_prefix("Qt")
        .map_or(class.as_str(), |versioned| {
            versioned.trim_start_matches(|ch: char| ch.is_ascii_digit())
        });
    class.starts_with("QWindowPopup")
}

fn near_anchor(rect: RECT, anchor: POINT, work: RECT) -> bool {
    rect.left < work.right
        && rect.right > work.left
        && rect.top < work.bottom
        && rect.bottom > work.top - ANCHOR_TOLERANCE_PX
        && (rect.left - ANCHOR_TOLERANCE_PX..=rect.right + ANCHOR_TOLERANCE_PX).contains(&anchor.x)
        && (rect.top - ANCHOR_TOLERANCE_PX..=rect.bottom + ANCHOR_TOLERANCE_PX).contains(&anchor.y)
}

fn position(rect: RECT, anchor: POINT, work: RECT) -> Option<POINT> {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let top = anchor.y.max(work.top);
    if rect.top >= top
        || width <= 0
        || height <= 0
        || width > work.right - work.left
        || height > work.bottom - top
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

    const WORK: RECT = RECT {
        left: -1920,
        top: -160,
        right: 0,
        bottom: 800,
    };
    const ANCHOR: POINT = POINT { x: -35, y: -160 };

    #[test]
    fn upward_menu_moves_below_the_bar_and_stays_inside_the_monitor() {
        let rect = RECT {
            left: -120,
            top: -440,
            right: 80,
            bottom: -160,
        };
        assert!(near_anchor(rect, ANCHOR, WORK));
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
        assert!(near_anchor(rect, ANCHOR, WORK));
        assert_eq!(position(rect, ANCHOR, WORK), None);
    }

    #[test]
    fn menu_stays_put_if_it_cannot_fit_below() {
        let rect = RECT {
            left: -240,
            top: -190,
            right: -20,
            bottom: 800,
        };
        assert_eq!(position(rect, ANCHOR, WORK), None);
    }

    #[test]
    fn popup_far_from_the_clicked_icon_is_ignored() {
        let rect = RECT {
            left: -1750,
            top: -190,
            right: -1530,
            bottom: 90,
        };
        assert!(!near_anchor(rect, ANCHOR, WORK));
    }
}
