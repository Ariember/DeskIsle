//! In-memory backdrops for glass. Samples belong to an open surface and are discarded
//! with it. Screen samples come from GDI; wallpaper samples from the wallpaper image
//! Windows lays out on the monitor, area-averaged into a smaller image as requested.
use std::hash::{DefaultHasher, Hash, Hasher};

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::{
    Foundation::{E_INVALIDARG, HWND, POINT},
    Graphics::Dwm::{DWMWA_CLOAK, DWMWA_USE_HOSTBACKDROPBRUSH, DwmFlush, DwmSetWindowAttribute},
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::{
        IsWindowVisible, SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        SystemParametersInfoW,
    },
};
use windows::core::{Error, Owned, Result, w};

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
        // Keep enough detail for GPU optical meniscus refraction and smooth Gaussian
        // diffusion; only large full-screen snapshots step down slightly to cap memory.
        let area = i64::from(rect.width.max(1)) * i64::from(rect.height.max(1));
        let scale = if area > 800_000 {
            1.6 + 0.004 * blur.clamp(0, 100) as f64
        } else {
            1.0
        };
        let width = (rect.width as f64 / scale).ceil().max(1.) as i32;
        let height = (rect.height as f64 / scale).ceil().max(1.) as i32;
        Self::sample(rect, width, height, wallpaper)
    }

    /// The desktop wallpaper under `rect` at full resolution, the scene Liquid Glass
    /// refracts. Windows above the desktop are not part of it.
    pub fn wallpaper(rect: Rect) -> Result<Self> {
        Self::from_wallpaper(rect, rect.width, rect.height)
    }

    /// Area-averages the wallpaper under `rect` into `width` x `height` pixels. Parts of
    /// `rect` outside the monitor that holds its center repeat that monitor's edge.
    fn from_wallpaper(rect: Rect, width: i32, height: i32) -> Result<Self> {
        if rect.width <= 0 || rect.height <= 0 || width <= 0 || height <= 0 {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let monitor = monitor_at(rect.x + rect.width / 2, rect.y + rect.height / 2)?;
        let image = super::wallpaper::for_monitor(monitor, wallpaper_stamp())?;
        let (iw, ih) = (image.width as i64, image.height as i64);
        let (w, h) = (width as usize, height as usize);
        let mut pixels = vec![0u8; w * h * 4];
        let kx = f64::from(rect.width) / f64::from(width);
        let ky = f64::from(rect.height) / f64::from(height);
        for y in 0..h {
            let y0 = (y as f64 * ky) as i64;
            let y1 = (((y + 1) as f64 * ky) as i64).max(y0 + 1);
            for x in 0..w {
                let x0 = (x as f64 * kx) as i64;
                let x1 = (((x + 1) as f64 * kx) as i64).max(x0 + 1);
                let mut sum = [0u32; 3];
                let mut n = 0u32;
                for sy in y0..y1 {
                    let my = (i64::from(rect.y - monitor.y) + sy).clamp(0, ih - 1);
                    for sx in x0..x1 {
                        let mx = (i64::from(rect.x - monitor.x) + sx).clamp(0, iw - 1);
                        let i = ((my * iw + mx) * 4) as usize;
                        for (total, &v) in sum.iter_mut().zip(&image.pixels[i..i + 3]) {
                            *total += u32::from(v);
                        }
                        n += 1;
                    }
                }
                let o = (y * w + x) * 4;
                for k in 0..3 {
                    pixels[o + k] = (sum[k] / n) as u8;
                }
                pixels[o + 3] = 255;
            }
        }
        Ok(Self {
            width: width as u32,
            height: height as u32,
            pixels,
        })
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
        if wallpaper {
            return Self::from_wallpaper(rect, width, height);
        }
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
            {
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

/// Lets `hwnd`'s `Windows.UI.Composition` tree draw the live, system-blurred content
/// behind it through `Compositor::CreateHostBackdropBrush`. Windows 10 and Windows 11
/// before build 22000 reject the attribute.
pub fn use_host_backdrop(hwnd: HWND) -> Result<()> {
    let on = 1i32;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_HOSTBACKDROPBRUSH,
            (&raw const on).cast(),
            size_of_val(&on) as u32,
        )
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

/// Changes whenever the desktop wallpaper image or background color changes, including
/// slideshow steps, which rewrite the transcoded wallpaper file.
pub fn wallpaper_stamp() -> u64 {
    let mut hasher = DefaultHasher::new();
    let mut path = [0u16; 260];
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            path.len() as u32,
            Some(path.as_mut_ptr().cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if read.is_ok() {
        let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
        let path = String::from_utf16_lossy(&path[..end]);
        path.hash(&mut hasher);
        if let Ok(meta) = std::fs::metadata(&path) {
            meta.len().hash(&mut hasher);
            meta.modified().ok().hash(&mut hasher);
        }
    }
    unsafe { GetSysColor(COLOR_DESKTOP) }.hash(&mut hasher);
    // Fill, fit, tile and the like are stored here rather than in the image path.
    for name in [w!("WallpaperStyle"), w!("TileWallpaper")] {
        let mut value = [0u16; 8];
        let mut size = size_of_val(&value) as u32;
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!(r"Control Panel\Desktop"),
                name,
                RRF_RT_REG_SZ,
                None,
                Some(value.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if read.is_ok() {
            value.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Bounds of the monitor nearest to the screen point `(x, y)`.
fn monitor_at(x: i32, y: i32) -> Result<Rect> {
    let monitor = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    monitor_rect(monitor)
}

fn monitor_rect(monitor: HMONITOR) -> Result<Rect> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }.ok()?;
    let r = info.rcMonitor;
    Ok(Rect {
        x: r.left,
        y: r.top,
        width: r.right - r.left,
        height: r.bottom - r.top,
    })
}

/// Screen position of `hwnd`'s client origin and the bounds of the monitor showing it.
pub fn placement(hwnd: HWND) -> Result<(i32, i32, Rect)> {
    let mut origin = POINT::default();
    unsafe { ClientToScreen(hwnd, &mut origin) }.ok()?;
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }.ok()?;
    let r = info.rcMonitor;
    Ok((
        origin.x,
        origin.y,
        Rect {
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
        },
    ))
}
