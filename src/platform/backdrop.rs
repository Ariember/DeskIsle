//! In-memory backdrops for static glass. Samples belong to an open surface and are
//! discarded with it. GDI area-averages into a smaller image as the blur increases;
//! the bitmap brush interpolates those samples when painting at window size.
use windows::Win32::{
    Foundation::{E_INVALIDARG, HWND},
    Graphics::Dwm::{DWMWA_CLOAK, DwmFlush, DwmSetWindowAttribute},
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::IsWindowVisible,
};
use windows::core::{Error, Owned, Result};

use crate::geometry::Rect;

pub struct Backdrop {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

struct Screen(HDC);
impl Drop for Screen {
    fn drop(&mut self) {
        unsafe { ReleaseDC(None, self.0) };
    }
}

struct MemoryDc(HDC);
impl Drop for MemoryDc {
    fn drop(&mut self) {
        let _ = unsafe { DeleteDC(self.0) };
    }
}

impl Backdrop {
    pub fn capture(rect: Rect, blur: i32, wallpaper: bool) -> Result<Self> {
        let sample = 1. + 0.32 * blur.clamp(0, 100) as f64;
        let width = (rect.width as f64 / sample).ceil().max(1.) as i32;
        let height = (rect.height as f64 / sample).ceil().max(1.) as i32;
        Self::sample(rect, width, height, wallpaper)
    }

    /// When geometry or glass settings change while a popup is visible, exclude its
    /// old frame from the new sample without altering activation, focus or hit testing.
    pub fn behind(hwnd: HWND, rect: Rect, blur: i32, wallpaper: bool) -> Result<Self> {
        if wallpaper || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return Self::capture(rect, blur, wallpaper);
        }
        let cloak = |on: i32| unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_CLOAK,
                (&raw const on).cast(),
                size_of_val(&on) as u32,
            )
        };
        cloak(1)?;
        let sample = unsafe { DwmFlush() }.and_then(|()| Self::capture(rect, blur, wallpaper));
        cloak(0)?;
        sample
    }

    fn sample(rect: Rect, width: i32, height: i32, wallpaper: bool) -> Result<Self> {
        if rect.width <= 0 || rect.height <= 0 {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let length = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= isize::MAX as usize)
            .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
        unsafe {
            let screen = Screen(GetDC(None));
            if screen.0.is_invalid() {
                return Err(Error::from_thread());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = Owned::new(CreateDIBSection(
                Some(screen.0),
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )?);
            // GDI writes the color channels. Initialize the reserved byte and any pixels
            // outside the desktop's clip before reading the DIB as a byte slice.
            bits.cast::<u8>().write_bytes(0, length);
            // Destroying the memory DC releases its bitmap selection before bitmap drops.
            let dest = MemoryDc(CreateCompatibleDC(Some(screen.0)));
            if dest.0.is_invalid() || SelectObject(dest.0, (*bitmap).into()).is_invalid() {
                return Err(Error::from_thread());
            }
            if wallpaper {
                if SetMapMode(dest.0, MM_ANISOTROPIC) == 0 {
                    return Err(Error::from_thread());
                }
                SetWindowExtEx(dest.0, rect.width, rect.height, None).ok()?;
                SetViewportExtEx(dest.0, width, height, None).ok()?;
                SetWindowOrgEx(dest.0, rect.x, rect.y, None).ok()?;
                PaintDesktop(dest.0).ok()?;
            } else {
                if SetStretchBltMode(dest.0, HALFTONE) == 0 {
                    return Err(Error::from_thread());
                }
                SetBrushOrgEx(dest.0, 0, 0, None).ok()?;
                StretchBlt(
                    dest.0,
                    0,
                    0,
                    width,
                    height,
                    Some(screen.0),
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    SRCCOPY | CAPTUREBLT,
                )
                .ok()?;
            }
            GdiFlush().ok()?;
            // The top-down 32-bit DIB has exactly four bytes per pixel without padding.
            let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), length).to_vec();
            Ok(Self {
                width: width as u32,
                height: height as u32,
                pixels,
            })
        }
    }
}

pub fn wallpaper_tint(rect: Rect) -> Result<(u8, u8, u8)> {
    let image = Backdrop::sample(rect, 16, 16, true)?;
    let mut sums = [0u32; 3];
    let (pixels, _) = image.pixels.as_chunks::<4>();
    for pixel in pixels {
        for (sum, component) in sums.iter_mut().zip(pixel) {
            *sum += u32::from(*component);
        }
    }
    let [b, g, r] = sums.map(|sum| (sum / 256) as u8);
    Ok((r, g, b))
}
