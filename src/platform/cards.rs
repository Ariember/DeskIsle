//! A non-activating tool window just above Explorer's desktop icon host. The card
//! window remains owned by DeskIsle if Explorer restarts.
use super::{Event, queue};
use crate::{cards::Card, geometry::Rect, render::Renderer, ui::cards::CardLayout};
use std::cell::Cell;
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};
use windows::core::{Error, Owned, PCWSTR, Result, w};

const CLASS: PCWSTR = w!("DeskIsle.Cards");

/// The standalone publisher owns this mutex while writing its snapshots.
pub(crate) fn external_magpie_active() -> bool {
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    // Open without creating so a manual publisher can start at any time.
    unsafe {
        match OpenMutexW(
            SYNCHRONIZATION_SYNCHRONIZE,
            false,
            w!("Local\\DeskIsle-magpie-usage-Publisher"),
        ) {
            Ok(handle) => {
                drop(Owned::new(handle));
                true
            }
            Err(err) => err.code() != ERROR_FILE_NOT_FOUND.to_hresult(),
        }
    }
}

pub(super) fn register_class(instance: HINSTANCE, cursor: HCURSOR) -> Result<()> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: CLASS,
        hCursor: cursor,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(Error::from_thread());
    }
    Ok(())
}

thread_local! {
    static NAV: Cell<Option<Rect>> = const { Cell::new(None) };
    static WHEEL: Cell<i32> = const { Cell::new(0) };
}

#[derive(Default)]
pub(crate) struct DesktopCards {
    window: Option<HWND>,
    layout: Option<CardLayout>,
    page: usize,
}

impl DesktopCards {
    pub fn update(
        &mut self,
        renderer: &mut Renderer,
        cards: &[Card],
        work: Rect,
        dpi: u32,
    ) -> Result<()> {
        let Some(layout) = CardLayout::new(work, dpi, cards, self.page) else {
            self.page = 0;
            self.hide();
            return Ok(());
        };
        self.page = layout.page;
        let Some(desktop) = desktop_icon_host() else {
            // Explorer may be starting again. Leave the work area clear until it is ready.
            self.hide();
            return Ok(());
        };
        let hwnd = match self.window {
            Some(hwnd) => hwnd,
            None => {
                let hwnd = unsafe {
                    CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                        CLASS,
                        w!("桌面卡片"),
                        WS_POPUP,
                        0,
                        0,
                        1,
                        1,
                        None,
                        None,
                        Some(GetModuleHandleW(None)?.into()),
                        None,
                    )?
                };
                self.window = Some(hwnd);
                hwnd
            }
        };
        let bounds = layout.bounds;
        let changed = self.layout.as_ref() != Some(&layout);
        if changed {
            set_shape(hwnd, &layout, dpi)?;
        }
        renderer.draw_cards(hwnd, &layout, &cards[layout.items.clone()], dpi)?;
        NAV.set(layout.navigation);
        unsafe {
            let previous = GetWindow(desktop, GW_HWNDPREV).ok();
            let visible = IsWindowVisible(hwnd).as_bool();
            if changed || !visible || previous != Some(hwnd) {
                let (after, order) = match previous {
                    Some(prev) if prev == hwnd => (None, SWP_NOZORDER),
                    // A neighbor in the topmost band would promote our window. HWND_TOP
                    // instead places it first among the ordinary windows.
                    Some(prev)
                        if !WINDOW_EX_STYLE(GetWindowLongPtrW(prev, GWL_EXSTYLE) as u32)
                            .contains(WS_EX_TOPMOST) =>
                    {
                        (Some(prev), SET_WINDOW_POS_FLAGS(0))
                    }
                    _ => (Some(HWND_TOP), SET_WINDOW_POS_FLAGS(0)),
                };
                let positioned = SetWindowPos(
                    hwnd,
                    after,
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                    order | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                if let Err(err) = positioned {
                    // Both shell handles are borrowed and can disappear while we draw.
                    if !IsWindow(Some(desktop)).as_bool()
                        || previous.is_some_and(|w| !IsWindow(Some(w)).as_bool())
                    {
                        self.hide();
                        return Ok(());
                    }
                    return Err(err);
                }
            }
        }
        if changed {
            self.layout = Some(layout);
        }
        Ok(())
    }

    pub fn hide(&self) {
        if let Some(hwnd) = self.window
            && unsafe { IsWindowVisible(hwnd) }.as_bool()
        {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            WHEEL.set(0);
        }
    }

    pub fn change_page(&mut self, delta: i32) {
        self.page = self.page.saturating_add_signed(delta as isize);
    }
}

impl Drop for DesktopCards {
    fn drop(&mut self) {
        if let Some(hwnd) = self.window {
            // Created and destroyed on the message-loop thread.
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        NAV.set(None);
        WHEEL.set(0);
    }
}

fn desktop_icon_host() -> Option<HWND> {
    // Explorer hosts SHELLDLL_DefView directly under Progman or a top-level WorkerW.
    unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return None;
        }
        let owns_icons =
            |hwnd| FindWindowExW(Some(hwnd), None, w!("SHELLDLL_DefView"), None).is_ok();
        if owns_icons(shell) {
            return Some(shell);
        }
        let mut previous = None;
        while let Ok(worker) = FindWindowExW(None, previous, w!("WorkerW"), None) {
            if owns_icons(worker) {
                return Some(worker);
            }
            previous = Some(worker);
        }
        None
    }
}

/// Clip the window and its input to the same set of rounded tiles. Desktop icons and
/// the desktop context menu remain accessible in every gap.
fn set_shape(hwnd: HWND, layout: &CardLayout, dpi: u32) -> Result<()> {
    let diameter = (crate::ui::cards::CARD_RADIUS_DIP * 2. * dpi as f32 / 96.).round() as i32;
    unsafe {
        let region = Owned::new(CreateRectRgn(0, 0, 0, 0));
        if region.is_invalid() {
            return Err(Error::from_thread());
        }
        for r in layout.tiles.iter().chain(layout.navigation.iter()) {
            let part = Owned::new(CreateRoundRectRgn(
                r.x,
                r.y,
                r.x + r.width + 1,
                r.y + r.height + 1,
                diameter,
                diameter,
            ));
            if part.is_invalid()
                || CombineRgn(Some(*region), Some(*region), Some(*part), RGN_OR) == RGN_ERROR
            {
                return Err(Error::from_thread());
            }
        }
        if SetWindowRgn(hwnd, Some(*region), true) == 0 {
            return Err(Error::from_thread());
        }
        // SetWindowRgn transfers ownership to USER32 only on success.
        std::mem::forget(region);
    }
    Ok(())
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_PAINT => {
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
        }
        WM_LBUTTONUP => {
            let x = lp.0 as i16 as i32;
            let y = (lp.0 >> 16) as i16 as i32;
            if let Some(r) = NAV.get().filter(|r| r.contains(x, y)) {
                if x < r.x + r.height {
                    queue(hwnd, Event::CardPage(-1));
                } else if x >= r.x + r.width - r.height {
                    queue(hwnd, Event::CardPage(1));
                }
            }
        }
        WM_MOUSEWHEEL if NAV.get().is_some() => {
            let delta = WHEEL.get() + ((wp.0 >> 16) as i16 as i32);
            WHEEL.set(delta % 120);
            if delta.abs() >= 120 {
                queue(hwnd, Event::CardPage(-delta / 120));
            }
        }
        WM_CLOSE => {}
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cards::CardState, platform, settings::Settings};
    use windows::Win32::Foundation::RECT;

    #[test]
    #[ignore = "requires an interactive Explorer desktop and DirectComposition"]
    fn cards_show_page_and_hide_on_the_desktop_without_activation() -> Result<()> {
        let _apartment = platform::Desktop::init()?;
        // These HWNDs stay hidden. No AppBar registration, taskbar hiding or user files.
        let (top, bottom) = platform::create_bars()?;
        let mut window = DesktopCards::default();
        let mut renderer = Renderer::new(top.0, bottom.0, false, &Settings::default())?;
        let monitor = platform::monitor()?;
        let work = Rect {
            height: (370. * monitor.dpi as f32 / 96.).round() as i32,
            ..monitor.work
        };
        let data: Vec<_> = [CardState::Ready, CardState::Error, CardState::Setup]
            .into_iter()
            .map(|state| Card::simple("窗口测试", "42", "显示与翻页", state))
            .collect();
        let desktop = desktop_icon_host().expect("Explorer desktop is running");
        window.update(&mut renderer, &data, work, monitor.dpi)?;
        let hwnd = window.window.unwrap();
        let bounds = || -> Result<RECT> {
            let mut r = RECT::default();
            unsafe {
                GetWindowRect(hwnd, &mut r)?;
            }
            Ok(r)
        };
        assert!(unsafe { IsWindowVisible(hwnd) }.as_bool());
        assert_ne!(unsafe { GetForegroundWindow() }, hwnd);
        assert_eq!(unsafe { GetWindow(hwnd, GW_HWNDNEXT)? }, desktop);
        let first = bounds()?;
        let layout = window.layout.as_ref().unwrap();
        let nav = layout.navigation.unwrap();
        // The transparent gap between the first two cards is outside the HWND.
        unsafe {
            let shape = Owned::new(CreateRectRgn(0, 0, 0, 0));
            assert_ne!(GetWindowRgn(hwnd, *shape), RGN_ERROR);
            let r = layout.tiles[0];
            assert!(PtInRegion(*shape, r.width / 2, r.y + r.height / 2).as_bool());
            assert!(!PtInRegion(*shape, r.width / 2, r.y + r.height + 2).as_bool());
            let x = nav.x + nav.width - nav.height / 2;
            let y = nav.y + nav.height / 2;
            SendMessageW(
                hwnd,
                WM_LBUTTONUP,
                None,
                Some(LPARAM(((y << 16) | x) as isize)),
            );
        }
        let Event::CardPage(delta) = platform::next_event()? else {
            panic!("page button did not queue a turn");
        };
        window.change_page(delta);
        window.update(&mut renderer, &data, work, monitor.dpi)?;
        let last = bounds()?;
        assert!(first.bottom - first.top > last.bottom - last.top);
        window.update(&mut renderer, &[], work, monitor.dpi)?;
        assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
        drop(renderer);
        drop(window);
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
        Ok(())
    }
}
