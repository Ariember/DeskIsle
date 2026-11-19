use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use windows::Win32::{
    Foundation::{E_UNEXPECTED, HMODULE, HWND, POINT},
    Graphics::{
        Direct2D::{Common::*, *},
        Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
        Direct3D11::*,
        DirectComposition::*,
        DirectWrite::*,
        Dxgi::{Common::*, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_UNSUPPORTED, IDXGIDevice},
        Imaging::*,
    },
    System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    UI::{
        Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW},
        WindowsAndMessaging::DestroyIcon,
    },
};
use windows::core::{Error, HSTRING, Interface, Result, w};

pub struct DockIcon<'a> {
    pub path: &'a Path,
    pub label: &'a str,
    pub center_x: i32,
    pub center_y: i32,
    pub size: i32,
    pub running: bool,
    pub active: bool,
}

type Dimensions = (i32, i32, u32);

pub struct Renderer {
    device: IDCompositionDesktopDevice,
    top: Bar,
    bottom: Bar,
    write: IDWriteFactory,
    imaging: IWICImagingFactory,
    icons: Vec<IconLayer>,
    bottom_size: Option<Dimensions>,
    indicators: Vec<(i32, bool, bool)>,
    top_state: Option<(Dimensions, String, String)>,
}

struct Bar {
    // The target must outlive all commits to its root.
    _target: IDCompositionTarget,
    root: IDCompositionVisual2,
}

impl Bar {
    fn new(device: &IDCompositionDesktopDevice, hwnd: HWND) -> Result<Self> {
        unsafe {
            let target = device.CreateTargetForHwnd(hwnd, true)?;
            let root = device.CreateVisual()?;
            target.SetRoot(&root)?;
            Ok(Self {
                _target: target,
                root,
            })
        }
    }
}

struct IconLayer {
    path: PathBuf,
    label: String,
    size: i32,
    center: (i32, i32),
    visual: IDCompositionVisual2,
    scale: IDCompositionScaleTransform,
    hovered: bool,
    start_time: Instant,
    start_scale: f32,
}

const LIFTED: f32 = 1.24;
const DURATION: f32 = 0.14;

impl IconLayer {
    fn matches(&self, icon: &DockIcon<'_>) -> bool {
        self.path == icon.path && self.label == icon.label && self.size == icon.size
    }

    fn animate(&mut self, device: &IDCompositionDesktopDevice, hover: bool) -> Result<()> {
        let old_goal = if self.hovered { LIFTED } else { 1.0 };
        let goal = if hover { LIFTED } else { 1.0 };
        let remaining = (1.0 - self.start_time.elapsed().as_secs_f32() / DURATION).clamp(0., 1.);
        let from = old_goal + (self.start_scale - old_goal) * remaining.powi(3);
        let delta = goal - from;
        unsafe {
            let curve = device.CreateAnimation()?;
            curve.AddCubic(
                0.,
                from,
                3. * delta / DURATION,
                -3. * delta / DURATION.powi(2),
                delta / DURATION.powi(3),
            )?;
            curve.End(DURATION as f64, goal)?;
            self.scale.SetScaleX(&curve)?;
            self.scale.SetScaleY(&curve)?;
        }
        self.start_time = Instant::now();
        self.start_scale = from;
        self.hovered = hover;
        Ok(())
    }
}

impl Renderer {
    /// Both HWNDs belong to this UI thread and outlive the renderer.
    pub fn new(top: HWND, bottom: HWND) -> Result<Self> {
        unsafe {
            let create = |driver| -> Result<ID3D11Device> {
                let mut device = None;
                D3D11CreateDevice(
                    None,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )?;
                device.ok_or_else(|| Error::from(E_UNEXPECTED))
            };
            let d3d = match create(D3D_DRIVER_TYPE_HARDWARE) {
                Err(e) if e.code() == DXGI_ERROR_UNSUPPORTED => create(D3D_DRIVER_TYPE_WARP)?,
                result => result?,
            };
            let dxgi: IDXGIDevice = d3d.cast()?;
            let d2d = D2D1CreateDevice(&dxgi, None)?;
            let device: IDCompositionDesktopDevice = DCompositionCreateDevice2(&d2d)?;
            Ok(Self {
                top: Bar::new(&device, top)?,
                bottom: Bar::new(&device, bottom)?,
                device,
                write: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
                imaging: CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?,
                icons: Vec::new(),
                bottom_size: None,
                indicators: Vec::new(),
                top_state: None,
            })
        }
    }

    pub fn check_device(&self) -> Result<()> {
        // The desktop device exposes CheckDeviceState on the original device interface.
        let device: IDCompositionDevice = self.device.cast()?;
        if unsafe { device.CheckDeviceState()? }.as_bool() {
            Ok(())
        } else {
            Err(Error::from(DXGI_ERROR_DEVICE_REMOVED))
        }
    }

    pub fn draw_top(
        &mut self,
        width: i32,
        height: i32,
        dpi: u32,
        active_name: &str,
        time: &str,
    ) -> Result<()> {
        let dimensions = (width, height, dpi);
        if self.top_state.as_ref().is_some_and(|(size, title, clock)| {
            *size == dimensions && title == active_name && clock == time
        }) {
            return Ok(());
        }
        let s = dpi as f32 / 96.;
        let w = width as f32 / s;
        let h = height as f32 / s;
        let surface = paint(&self.device, width, height, dpi, |canvas| {
            canvas.panel(w, h, h * 0.5)?;
            // Mark, name and clock share a baseline; the window title takes the flexible space.
            canvas.round(12., h / 2. - 5., 16., h / 2. + 5., 2., MINT)?;
            canvas.round(18., h / 2. - 2., 22., h / 2. + 5., 2., MINT)?;
            canvas.text(
                &self.write,
                "DeskIsle",
                canvas.rect(29., 0., 84., h),
                13.,
                true,
                INK,
            )?;
            canvas.round(88., h * 0.28, 89., h * 0.72, 0.5, EDGE)?;
            canvas.text(
                &self.write,
                active_name,
                canvas.rect(100., 0., w - 68., h),
                12.,
                false,
                MUTED,
            )?;
            canvas.text(
                &self.write,
                time,
                canvas.rect(w - 54., 0., w - 7., h),
                13.,
                true,
                INK,
            )
        })?;
        unsafe {
            self.top.root.SetContent(&surface)?;
            self.device.Commit()?;
        }
        self.top_state = Some((dimensions, active_name.into(), time.into()));
        Ok(())
    }

    pub fn draw_dock(
        &mut self,
        width: i32,
        height: i32,
        dpi: u32,
        items: &[DockIcon<'_>],
        hover: Option<usize>,
    ) -> Result<()> {
        let size = (width, height, dpi);
        let layout_changed = self.icons.len() != items.len()
            || self.icons.iter().zip(items).any(|(layer, icon)| {
                !layer.matches(icon) || layer.center != (icon.center_x, icon.center_y)
            });
        let statuses = items.iter().map(|i| (i.center_x, i.running, i.active));
        let panel_changed =
            self.bottom_size != Some(size) || statuses.clone().ne(self.indicators.iter().copied());
        if panel_changed {
            let s = dpi as f32 / 96.;
            let w = width as f32 / s;
            let h = height as f32 / s;
            let surface = paint(&self.device, width, height, dpi, |c| {
                c.panel(w, h, 18.)?;
                for icon in items.iter().filter(|i| i.running) {
                    let x = icon.center_x as f32 / s;
                    let half = if icon.active { 5. } else { 2. };
                    c.round(
                        x - half,
                        h - 8.,
                        x + half,
                        h - 5.,
                        1.5,
                        if icon.active { MINT } else { MUTED },
                    )?;
                }
                Ok(())
            })?;
            unsafe { self.bottom.root.SetContent(&surface)? };
            self.indicators = statuses.collect();
            self.bottom_size = Some(size);
        }
        if layout_changed {
            unsafe { self.bottom.root.RemoveAllVisuals()? };
            let mut previous = std::mem::take(&mut self.icons);
            for item in items {
                let mut layer =
                    if let Some(i) = previous.iter().position(|layer| layer.matches(item)) {
                        previous.swap_remove(i)
                    } else {
                        self.icon(item)?
                    };
                layer.center = (item.center_x, item.center_y);
                unsafe {
                    layer
                        .visual
                        .SetOffsetX2((item.center_x - item.size / 2) as f32)?;
                    layer
                        .visual
                        .SetOffsetY2((item.center_y - item.size / 2) as f32)?;
                    self.bottom.root.AddVisual(&layer.visual, true, None)?;
                }
                self.icons.push(layer);
            }
        }
        let mut changed = panel_changed || layout_changed;
        for (index, layer) in self.icons.iter_mut().enumerate() {
            let hovered = hover == Some(index);
            if layer.hovered != hovered {
                layer.animate(&self.device, hovered)?;
                changed = true;
            }
        }
        if changed {
            unsafe { self.device.Commit()? };
        }
        Ok(())
    }

    fn icon(&self, item: &DockIcon<'_>) -> Result<IconLayer> {
        let size = item.size as f32;
        let surface = paint(&self.device, item.size, item.size, 96, |c| {
            if let Some(bitmap) = self.shell_icon(item.path)? {
                unsafe {
                    let texture = c.ctx.CreateBitmapFromWicBitmap(&bitmap, None)?;
                    c.ctx.DrawBitmap(
                        &texture,
                        Some(&c.rect(0., 0., size, size)),
                        1.,
                        D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                        None,
                        None,
                    );
                }
            } else {
                // Shell can lose access to an executable after enumeration. Keep its slot identifiable.
                c.round(0., 0., size, size, size * 0.26, EDGE)?;
                let initial = item.label.chars().next().unwrap_or('?').to_string();
                c.text(
                    &self.write,
                    &initial,
                    c.rect(size * 0.24, 0., size, size),
                    size * 0.56,
                    true,
                    INK,
                )?;
            }
            Ok(())
        })?;
        unsafe {
            let visual = self.device.CreateVisual()?;
            visual.SetContent(&surface)?;
            let scale = self.device.CreateScaleTransform()?;
            scale.SetCenterX2(size * 0.5)?;
            scale.SetCenterY2(size * 0.75)?;
            scale.SetScaleX2(1.)?;
            scale.SetScaleY2(1.)?;
            visual.SetTransform(&scale)?;
            Ok(IconLayer {
                path: item.path.into(),
                label: item.label.into(),
                size: item.size,
                center: (0, 0),
                visual,
                scale,
                hovered: false,
                start_time: Instant::now(),
                start_scale: 1.,
            })
        }
    }

    fn shell_icon(&self, path: &Path) -> Result<Option<IWICBitmapSource>> {
        if path.as_os_str().is_empty() {
            return Ok(None);
        }
        unsafe {
            let mut info = SHFILEINFOW::default();
            if SHGetFileInfoW(
                &HSTRING::from(path),
                Default::default(),
                Some(&mut info),
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            ) == 0
                || info.hIcon.is_invalid()
            {
                return Ok(None);
            }
            // SHGFI_ICON transfers ownership. WIC copies the pixels before the icon is destroyed.
            let copied = self.imaging.CreateBitmapFromHICON(info.hIcon);
            DestroyIcon(info.hIcon)?;
            Ok(Some(WICConvertBitmapSource(
                &GUID_WICPixelFormat32bppPBGRA,
                &copied?,
            )?))
        }
    }
}

type Rgb = (u8, u8, u8);
const EDGE: Rgb = (71, 82, 85);
const PANEL: Rgb = (28, 36, 39);
const INK: Rgb = (244, 247, 242);
const MUTED: Rgb = (182, 197, 197);
const MINT: Rgb = (132, 227, 191);

struct Canvas {
    ctx: ID2D1DeviceContext,
    origin: POINT,
    scale: f32,
}

fn paint(
    device: &IDCompositionDesktopDevice,
    width: i32,
    height: i32,
    dpi: u32,
    draw: impl FnOnce(&Canvas) -> Result<()>,
) -> Result<IDCompositionSurface> {
    unsafe {
        let surface = device.CreateSurface(
            width as u32,
            height as u32,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_ALPHA_MODE_PREMULTIPLIED,
        )?;
        let mut origin = POINT::default();
        let ctx: ID2D1DeviceContext = surface.BeginDraw(None, &mut origin)?;
        // BeginDraw may hand out an atlas region at a nonzero offset. Work in physical pixels
        // and clip the clear along with the drawing to that region.
        ctx.SetDpi(96., 96.);
        ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let bounds = D2D_RECT_F {
            left: origin.x as f32,
            top: origin.y as f32,
            right: (origin.x + width) as f32,
            bottom: (origin.y + height) as f32,
        };
        ctx.PushAxisAlignedClip(&bounds, D2D1_ANTIALIAS_MODE_ALIASED);
        ctx.Clear(Some(&D2D1_COLOR_F::default()));
        let canvas = Canvas {
            ctx,
            origin,
            scale: dpi as f32 / 96.,
        };
        let result = draw(&canvas);
        canvas.ctx.PopAxisAlignedClip();
        drop(canvas);
        // End the DComp update even when a brush, icon or font fails to load.
        let end = surface.EndDraw();
        result.and(end)?;
        Ok(surface)
    }
}

impl Canvas {
    fn rect(&self, left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.origin.x as f32 + left * self.scale,
            top: self.origin.y as f32 + top * self.scale,
            right: self.origin.x as f32 + right * self.scale,
            bottom: self.origin.y as f32 + bottom * self.scale,
        }
    }

    fn brush(&self, (r, g, b): Rgb) -> Result<ID2D1SolidColorBrush> {
        let color = D2D1_COLOR_F {
            r: r as f32 / 255.,
            g: g as f32 / 255.,
            b: b as f32 / 255.,
            a: 1.,
        };
        unsafe { self.ctx.CreateSolidColorBrush(&color, None) }
    }

    fn round(&self, l: f32, t: f32, r: f32, b: f32, radius: f32, color: Rgb) -> Result<()> {
        let shape = D2D1_ROUNDED_RECT {
            rect: self.rect(l, t, r, b),
            radiusX: radius * self.scale,
            radiusY: radius * self.scale,
        };
        unsafe { self.ctx.FillRoundedRectangle(&shape, &self.brush(color)?) };
        Ok(())
    }

    fn panel(&self, w: f32, h: f32, radius: f32) -> Result<()> {
        self.round(0., 0., w, h, radius, EDGE)?;
        self.round(1., 1., w - 1., h - 1., radius - 1., PANEL)
    }

    fn text(
        &self,
        factory: &IDWriteFactory,
        text: &str,
        rect: D2D_RECT_F,
        size: f32,
        bold: bool,
        color: Rgb,
    ) -> Result<()> {
        if rect.right <= rect.left || text.is_empty() {
            return Ok(());
        }
        unsafe {
            let format = factory.CreateTextFormat(
                w!("Segoe UI"),
                None,
                if bold {
                    DWRITE_FONT_WEIGHT_SEMI_BOLD
                } else {
                    DWRITE_FONT_WEIGHT_NORMAL
                },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size * self.scale,
                w!(""),
            )?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let ellipsis = factory.CreateEllipsisTrimmingSign(&format)?;
            format.SetTrimming(
                &DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    ..Default::default()
                },
                &ellipsis,
            )?;
            self.ctx.DrawText(
                &text.encode_utf16().collect::<Vec<_>>(),
                &format,
                &rect,
                &self.brush(color)?,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }
}
