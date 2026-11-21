use super::shell;
use crate::dock::WindowInfo;
use crate::geometry::Rect;
use crate::ui::preview::{PreviewLayout, fit_thumbnail};
use windows::Win32::{
    Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWM_THUMBNAIL_PROPERTIES, DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE,
            DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
            DwmUpdateThumbnailProperties,
        },
        Gdi::{DKGRAY_BRUSH, GetStockObject, HBRUSH},
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};
use windows::core::{Error, PCWSTR, Result, w};

const CLASS: PCWSTR = w!("DeskIsle.Preview");

pub(super) fn register_class(instance: HINSTANCE) -> Result<()> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: CLASS,
        // The stock brush belongs to GDI and outlives every preview.
        hbrBackground: HBRUSH(unsafe { GetStockObject(DKGRAY_BRUSH) }.0),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(Error::from_thread());
    }
    Ok(())
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // The preview's UI-thread owner closes it when the pointer leaves its icon.
    if msg == WM_CLOSE {
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
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
}

impl PreviewWindow {
    /// Places the preview above the Dock without activating either the host or its sources.
    pub fn show(
        windows: &[WindowInfo],
        center_x: i32,
        dock_top: i32,
        work: Rect,
        dpi: u32,
    ) -> Result<Option<Self>> {
        if windows.is_empty() {
            return Ok(None);
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT,
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
        };
        // A layered, WS_EX_TRANSPARENT window passes input through even to other processes.
        unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)? };
        preview.thumbnails = windows
            .iter()
            .filter_map(|window| Thumbnail::register(hwnd, window))
            .collect();
        while let Some(layout) =
            PreviewLayout::new(center_x, dock_top, work, dpi, preview.thumbnails.len())
        {
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
            let count = preview.thumbnails.len();
            let mut cells = layout.cells.into_iter();
            // A source can close or stop providing content while the others are configured.
            // Compact the grid after discarding it. Each extra pass removes at least one source.
            preview
                .thumbnails
                .retain(|thumbnail| thumbnail.place(cells.next().unwrap()).unwrap_or(false));
            if preview.thumbnails.len() == count {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        0,
                        0,
                        0,
                        0,
                        SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                    )?;
                }
                return Ok(Some(preview));
            }
        }
        Ok(None)
    }
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        // End the relationships before releasing their destination on its creating thread.
        self.thumbnails.clear();
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}
