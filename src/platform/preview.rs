use super::{EVENTS, Event, MK_LBUTTON, RELEASING_CAPTURE, queue, shell, track_leave};
use crate::dock::WindowInfo;
use crate::geometry::Rect;
use crate::ui::preview::{PreviewLayout, fit_thumbnail};
use std::cell::RefCell;
use std::mem::size_of;
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWM_THUMBNAIL_PROPERTIES, DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE,
            DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmSetWindowAttribute,
            DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
        },
        Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, DIB_RGB_COLORS, EndPaint,
            InvalidateRect, PAINTSTRUCT, SRCCOPY, StretchDIBits, UpdateWindow,
        },
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::WM_MOUSELEAVE,
        Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture},
        WindowsAndMessaging::*,
    },
};
use windows::core::{BOOL, Error, PCWSTR, Result, w};

const CLASS: PCWSTR = w!("DeskIsle.Preview");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewEvent {
    Move(i32, i32),
    Leave,
    Press(i32, i32),
    Click(i32, i32),
    Cancel,
}

thread_local! {
    /// Rendered BGRA chrome for the single UI-thread preview window, blitted in `WM_PAINT`.
    static PREVIEW_PIXELS: RefCell<Option<(i32, i32, Vec<u8>)>> = const { RefCell::new(None) };
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

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
    let event = match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if !hdc.is_invalid() {
                PREVIEW_PIXELS.with_borrow(|buffer| {
                    if let Some((width, height, pixels)) = buffer
                        && *width > 0
                        && *height > 0
                        && pixels.len() == (*width as usize) * (*height as usize) * 4
                    {
                        let info = BITMAPINFO {
                            bmiHeader: BITMAPINFOHEADER {
                                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                                biWidth: *width,
                                biHeight: -*height,
                                biPlanes: 1,
                                biBitCount: 32,
                                biCompression: BI_RGB.0,
                                ..Default::default()
                            },
                            ..Default::default()
                        };
                        unsafe {
                            StretchDIBits(
                                hdc,
                                0,
                                0,
                                *width,
                                *height,
                                0,
                                0,
                                *width,
                                *height,
                                Some(pixels.as_ptr().cast()),
                                &info,
                                DIB_RGB_COLORS,
                                SRCCOPY,
                            );
                        }
                    }
                });
                let _ = unsafe { EndPaint(hwnd, &ps) };
            }
            return LRESULT(0);
        }
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let _ = track_leave(hwnd);
            // A non-activating popup can miss a release outside its visible area.
            if wp.0 & MK_LBUTTON == 0 && unsafe { GetCapture() } == hwnd {
                queue(hwnd, Event::Preview(PreviewEvent::Cancel));
            }
            PreviewEvent::Move(x, y)
        }
        WM_MOUSELEAVE => PreviewEvent::Leave,
        WM_LBUTTONDOWN => PreviewEvent::Press(x, y),
        WM_LBUTTONUP => PreviewEvent::Click(x, y),
        WM_CAPTURECHANGED if !RELEASING_CAPTURE.get() => PreviewEvent::Cancel,
        WM_CANCELMODE => {
            queue(hwnd, Event::Preview(PreviewEvent::Cancel));
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        // The preview's UI-thread owner destroys it when dropped.
        WM_CLOSE => return LRESULT(0),
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    };
    queue(hwnd, Event::Preview(event));
    LRESULT(0)
}

/// Owns the DWM relationship, never the source application's window.
struct Thumbnail(isize);

impl Thumbnail {
    fn register(host: HWND, window: &WindowInfo) -> Option<Self> {
        let source = HWND(window.id as _);
        let thumbnail = Self(unsafe { DwmRegisterThumbnail(host, source) }.ok()?);
        // Check after registration so a reused HWND cannot redirect this relationship
        // to another application between the identity check and registration.
        shell::window_info(source)
            .ok()?
            .filter(|info| info.app_id == window.app_id)
            .map(|_| thumbnail)
    }

    fn place(&self, cell: Rect) -> Result<bool> {
        let size = unsafe { DwmQueryThumbnailSourceSize(self.0)? };
        if size.cx <= 0 || size.cy <= 0 {
            return Ok(false);
        }
        let r = fit_thumbnail(cell, size.cx, size.cy);
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE,
            rcDestination: RECT {
                left: r.x,
                top: r.y,
                right: r.x + r.width,
                bottom: r.y + r.height,
            },
            fVisible: true.into(),
            ..Default::default()
        };
        unsafe { DwmUpdateThumbnailProperties(self.0, &properties)? };
        Ok(true)
    }
}

impl Drop for Thumbnail {
    fn drop(&mut self) {
        let _ = unsafe { DwmUnregisterThumbnail(self.0) };
    }
}

pub struct PreviewWindow {
    hwnd: HWND,
    thumbnails: Vec<Thumbnail>,
    windows: Vec<WindowInfo>,
    layout: PreviewLayout,
}

impl PreviewWindow {
    /// Creates a hidden preview window above the Dock with DWM thumbnails registered and
    /// positioned. Call [`Self::set_pixels`] and [`Self::reveal`] before showing it.
    pub fn create(
        windows: &[WindowInfo],
        center_x: i32,
        dock_top: i32,
        work: Rect,
        dpi: u32,
        dark: bool,
    ) -> Result<Option<Self>> {
        if windows.is_empty() {
            return Ok(None);
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                CLASS,
                w!("DeskIsle Preview"),
                WS_POPUP,
                center_x,
                dock_top,
                1,
                1,
                None,
                None,
                Some(GetModuleHandleW(None)?.into()),
                None,
            )?
        };
        let mut preview = Self {
            hwnd,
            thumbnails: Vec::new(),
            windows: Vec::new(),
            layout: PreviewLayout {
                bounds: Rect {
                    x: center_x,
                    y: dock_top,
                    width: 1,
                    height: 1,
                },
                cards: Vec::new(),
            },
        };
        // Cosmetic only: older Windows builds ignore these attributes.
        let round = DWMWCP_ROUND;
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&raw const round).cast(),
                size_of_val(&round) as u32,
            )
        };
        preview.set_dark(dark);

        let mut pairs: Vec<(WindowInfo, Thumbnail)> = windows
            .iter()
            .filter_map(|window| {
                Thumbnail::register(hwnd, window).map(|thumbnail| (window.clone(), thumbnail))
            })
            .collect();
        while let Some(layout) = PreviewLayout::new(center_x, dock_top, work, dpi, pairs.len()) {
            let r = layout.bounds;
            unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    r.x,
                    r.y,
                    r.width,
                    r.height,
                    SWP_NOACTIVATE,
                )?;
            }
            let count = pairs.len();
            let mut cards = layout.cards.iter();
            // A source can close or stop providing content while the others are configured.
            // Compact the grid after discarding it. Each extra pass removes at least one source.
            pairs.retain(|(_, thumbnail)| {
                thumbnail
                    .place(cards.next().unwrap().thumbnail)
                    .unwrap_or(false)
            });
            if pairs.len() == count {
                let (shown_windows, thumbnails): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                preview.windows = shown_windows;
                preview.thumbnails = thumbnails;
                preview.layout = layout;
                return Ok(Some(preview));
            }
        }
        Ok(None)
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn layout(&self) -> &PreviewLayout {
        &self.layout
    }

    pub fn bounds(&self) -> Rect {
        self.layout.bounds
    }

    pub fn windows(&self) -> &[WindowInfo] {
        &self.windows
    }

    /// Updates cached window titles when only titles changed. Returns whether any title changed.
    pub fn update_titles(&mut self, fresh: &[WindowInfo]) -> bool {
        let mut changed = false;
        for window in &mut self.windows {
            if let Some(current) = fresh.iter().find(|item| item.id == window.id)
                && window.title != current.title
            {
                window.title.clone_from(&current.title);
                changed = true;
            }
        }
        changed
    }

    pub fn set_dark(&self, dark: bool) {
        let dark = BOOL::from(dark);
        let _ = unsafe {
            DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark).cast(),
                size_of::<BOOL>() as u32,
            )
        };
    }

    pub fn set_pixels(&self, pixels: Vec<u8>) {
        let r = self.layout.bounds;
        PREVIEW_PIXELS.set(Some((r.width, r.height, pixels)));
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    pub fn reveal(&self) -> Result<()> {
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )?;
            let _ = UpdateWindow(self.hwnd);
        }
        Ok(())
    }

    pub fn contains_screen_point(&self, x: i32, y: i32) -> bool {
        self.layout.bounds.contains(x, y) && unsafe { WindowFromPoint(POINT { x, y }) == self.hwnd }
    }

    pub fn track_leave(&self) -> Result<()> {
        track_leave(self.hwnd)
    }

    pub fn capture(&self) {
        unsafe { SetCapture(self.hwnd) };
    }

    pub fn release(&self) {
        if unsafe { GetCapture() } == self.hwnd {
            RELEASING_CAPTURE.set(true);
            let _ = unsafe { ReleaseCapture() };
            RELEASING_CAPTURE.set(false);
        }
    }
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        self.release();
        // End the relationships before releasing their destination on its creating thread.
        self.thumbnails.clear();
        let _ = unsafe { DestroyWindow(self.hwnd) };
        PREVIEW_PIXELS.set(None);
        EVENTS.with_borrow_mut(|events| events.retain(|e| !matches!(e, Event::Preview(_))));
    }
}
