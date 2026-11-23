//! The desktop wallpaper as Windows lays it out on each monitor, read from its image
//! file. `PaintDesktop` into a memory DC fails on current Windows 11 builds, so the
//! layout follows `IDesktopWallpaper` instead, as PrismGlass's `MonitorWallpaper` does.

use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::{
    Foundation::GENERIC_READ,
    Graphics::Imaging::*,
    System::Com::{CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
    UI::Shell::*,
};
use windows::core::{PCWSTR, PWSTR, Result};

use crate::geometry::Rect;

/// A monitor-sized BGRA image with opaque alpha.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

thread_local! {
    /// Composed wallpapers by monitor bounds and [`super::backdrop::wallpaper_stamp`].
    /// Decoding a photo takes tens of milliseconds; glass asks on every repaint.
    static CACHE: RefCell<Vec<(Rect, u64, Rc<Image>)>> = const { RefCell::new(Vec::new()) };
}

/// The wallpaper composed for `monitor`, cached while `stamp` stays the same.
pub fn for_monitor(monitor: Rect, stamp: u64) -> Result<Rc<Image>> {
    if let Some(image) = CACHE.with_borrow(|cache| {
        cache
            .iter()
            .find(|(rect, s, _)| *rect == monitor && *s == stamp)
            .map(|(_, _, image)| image.clone())
    }) {
        return Ok(image);
    }
    let image = Rc::new(compose(monitor)?);
    CACHE.with_borrow_mut(|cache| {
        cache.retain(|(rect, s, _)| *rect != monitor && *s == stamp);
        cache.push((monitor, stamp, image.clone()));
    });
    Ok(image)
}

/// Takes ownership of a `CoTaskMemAlloc` string from the shell.
fn take(text: PWSTR) -> String {
    if text.is_null() {
        return String::new();
    }
    let value = unsafe { text.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(text.0.cast())) };
    value
}

fn compose(monitor: Rect) -> Result<Image> {
    let (w, h) = (monitor.width.max(1) as u32, monitor.height.max(1) as u32);
    let wallpaper: IDesktopWallpaper =
        unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL)? };
    // COLORREF is 0x00BBGGRR.
    let background = unsafe { wallpaper.GetBackgroundColor() }.map_or(0, |c| c.0);
    let fill = [
        (background >> 16) as u8,
        (background >> 8) as u8,
        background as u8,
        255,
    ];
    let mut pixels = fill.repeat(w as usize * h as usize);
    let position = unsafe { wallpaper.GetPosition() }.unwrap_or(DWPOS_FILL);

    // The monitor's own wallpaper; with one image for all, any monitor's path is it.
    let mut path = String::new();
    let count = unsafe { wallpaper.GetMonitorDevicePathCount() }.unwrap_or(0);
    for index in 0..count {
        let Ok(id) = (unsafe { wallpaper.GetMonitorDevicePathAt(index) }) else {
            continue;
        };
        let rect = unsafe { wallpaper.GetMonitorRECT(PCWSTR(id.0)) };
        let image = unsafe { wallpaper.GetWallpaper(PCWSTR(id.0)) }.map(take);
        take(id);
        let (Ok(rect), Ok(image)) = (rect, image) else {
            continue;
        };
        let matches = rect.left == monitor.x && rect.top == monitor.y;
        if matches || path.is_empty() {
            path = image;
        }
        if matches {
            break;
        }
    }

    if !path.is_empty()
        && let Ok(source) = decode(&path)
    {
        place(&source, position, w, h, &mut pixels)?;
    }
    Ok(Image {
        width: w,
        height: h,
        pixels,
    })
}

fn decode(path: &str) -> Result<IWICBitmapSource> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let decoder = factory.CreateDecoderFromFilename(
            &windows::core::HSTRING::from(path),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        WICConvertBitmapSource(&GUID_WICPixelFormat32bppBGRA, &frame)
    }
}

/// Scales `source` to `width` x `height` and copies its pixels.
fn scaled(source: &IWICBitmapSource, width: u32, height: u32) -> Result<Vec<u8>> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let scaler = factory.CreateBitmapScaler()?;
        scaler.Initialize(source, width, height, WICBitmapInterpolationModeFant)?;
        let mut pixels = vec![0; width as usize * height as usize * 4];
        scaler.CopyPixels(std::ptr::null(), width * 4, &mut pixels)?;
        Ok(pixels)
    }
}

/// Lays `source` out on a `w` x `h` monitor the way Windows does for `position`.
/// Span is treated as fill, since only this monitor's share is needed.
fn place(
    source: &IWICBitmapSource,
    position: DESKTOP_WALLPAPER_POSITION,
    w: u32,
    h: u32,
    target: &mut [u8],
) -> Result<()> {
    let (mut iw, mut ih) = (0, 0);
    unsafe { source.GetSize(&mut iw, &mut ih)? };
    if iw == 0 || ih == 0 {
        return Ok(());
    }
    let (fw, fh) = (w as f64, h as f64);
    let (sw, sh) = match position {
        DWPOS_CENTER | DWPOS_TILE => (iw, ih),
        DWPOS_STRETCH => (w, h),
        DWPOS_FIT => {
            let k = (fw / iw as f64).min(fh / ih as f64);
            (
                (iw as f64 * k).round() as u32,
                (ih as f64 * k).round() as u32,
            )
        }
        _ => {
            let k = (fw / iw as f64).max(fh / ih as f64);
            ((iw as f64 * k).ceil() as u32, (ih as f64 * k).ceil() as u32)
        }
    };
    let (sw, sh) = (sw.max(1), sh.max(1));
    let pixels = if (sw, sh) == (iw, ih) {
        scaled(source, iw, ih)?
    } else {
        scaled(source, sw, sh)?
    };
    let tile = position == DWPOS_TILE;
    // Top-left of the image relative to the monitor; centered unless tiled.
    let (ox, oy) = if tile {
        (0, 0)
    } else {
        ((w as i64 - sw as i64) / 2, (h as i64 - sh as i64) / 2)
    };
    for y in 0..h as i64 {
        let sy = if tile { y % sh as i64 } else { y - oy };
        if sy < 0 || sy >= sh as i64 {
            continue;
        }
        for x in 0..w as i64 {
            let sx = if tile { x % sw as i64 } else { x - ox };
            if sx < 0 || sx >= sw as i64 {
                continue;
            }
            let from = ((sy * sw as i64 + sx) * 4) as usize;
            let to = ((y * w as i64 + x) * 4) as usize;
            target[to..to + 3].copy_from_slice(&pixels[from..from + 3]);
            target[to + 3] = 255;
        }
    }
    Ok(())
}
