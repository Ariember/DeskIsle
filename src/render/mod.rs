mod about;
mod canvas;
mod launchpad;
mod menu;
mod metrics;
mod panel;

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use windows::Win32::{
    Foundation::{D2DERR_RECREATE_TARGET, E_UNEXPECTED, HMODULE, HWND},
    Graphics::{
        Direct2D::D2D1CreateDevice,
        Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
        Direct3D11::*,
        DirectComposition::*,
        DirectWrite::*,
        Dxgi::{
            DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, DXGI_ERROR_UNSUPPORTED, IDXGIDevice,
        },
        Imaging::*,
    },
    System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    UI::{
        Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW},
        WindowsAndMessaging::{DestroyIcon, HICON},
    },
};
use windows::core::{Error, HSTRING, Interface, Result};

use crate::geometry::Rect;
use crate::metrics::Indicator;
use crate::ui::icon::{self, Symbol};
use crate::ui::layout::{StatusHit, StatusLayout};
use crate::ui::system::Mark;

use self::about::AboutView;
use self::canvas::{Canvas, Palette, Text, paint};
use self::menu::MenuView;
use self::panel::PanelView;

pub(crate) fn is_device_lost(error: &Error) -> bool {
    matches!(
        error.code(),
        DXGI_ERROR_DEVICE_REMOVED | DXGI_ERROR_DEVICE_RESET | D2DERR_RECREATE_TARGET
    )
}

pub struct DockIcon<'a> {
    /// None draws the built-in launchpad.
    pub path: Option<&'a Path>,
    pub label: &'a str,
    pub center_x: i32,
    pub center_y: i32,
    pub size: i32,
    pub running: bool,
    pub active: bool,
}

pub struct TrayImage {
    pub icon: Option<HICON>,
    pub stamp: u64,
    pub needs_attention: bool,
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
    top_state: Option<TopState>,
    metric_layer: Option<metrics::MetricLayer>,
    menu: Option<MenuView>,
    panel: Option<PanelView>,
    about: Option<AboutView>,
    launchpad: Option<launchpad::LaunchpadView>,
    palette: Palette,
    mark: Option<MarkLayer>,
}

#[derive(PartialEq)]
struct TopState {
    layout: StatusLayout,
    dpi: u32,
    title: String,
    time: String,
    stamps: Vec<u64>,
    marks: Vec<Mark>,
    open: Option<StatusHit>,
    hover: Option<StatusHit>,
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

/// The DeskIsle mark on its own layer above the status bar, so a click can squash and
/// stretch it on the compositor without repainting the bar.
struct MarkLayer {
    visual: IDCompositionVisual2,
    scale: IDCompositionScaleTransform,
    /// The bar height in pixels, DPI and palette the ring was painted for.
    key: (i32, u32, Palette),
}

struct IconLayer {
    path: Option<PathBuf>,
    label: String,
    size: i32,
    center: (i32, i32),
    visual: IDCompositionVisual2,
    scale: IDCompositionScaleTransform,
    hovered: bool,
    /// The hovered scale the last animation used.
    lift: f32,
    start_time: Instant,
    start_scale: f32,
}

const DURATION: f32 = 0.14;
/// Corner radius of the backing behind a hovered or open status bar target, in DIPs.
const PILL_RADIUS: f32 = 7.;
/// Size of the system icons on the status bar, in DIPs. The battery is drawn larger with
/// the same stroke weight, so its body reads at the bar's height.
const BAR_ICON: f32 = 16.;
const BATTERY_ICON: f32 = 22.;
/// The chevron of the tray overflow button, smaller than the icons beside it.
const OVERFLOW_ICON: f32 = 13.;
/// The DeskIsle mark: a ring centered this far from the bar's leading edge, in DIPs.
const MARK_X: f32 = 18.;
const MARK_RADIUS: f32 = 6.;
const MARK_STROKE: f32 = 1.75;
/// Edge of the mark's layer in DIPs, with room for the ring's antialiased rim.
const MARK_BOX: f32 = 20.;
/// The mark's squash and stretch on a click: (seconds, horizontal, vertical scale).
/// It flattens wide, springs back tall and narrow, wobbles once and settles round.
const MARK_BOUNCE: [(f32, f32, f32); 5] = [
    (0., 1., 1.),
    (0.07, 1.32, 0.72),
    (0.17, 0.86, 1.14),
    (0.26, 1.06, 0.95),
    (0.36, 1., 1.),
];

/// An ease-out cubic from `from` to `to` over `duration` seconds.
fn ease(
    device: &IDCompositionDesktopDevice,
    from: f32,
    to: f32,
    duration: f32,
) -> Result<IDCompositionAnimation> {
    let delta = to - from;
    unsafe {
        let curve = device.CreateAnimation()?;
        curve.AddCubic(
            0.,
            from,
            3. * delta / duration,
            -3. * delta / duration.powi(2),
            delta / duration.powi(3),
        )?;
        curve.End(duration as f64, to)?;
        Ok(curve)
    }
}

/// A curve through `keys` of (seconds, value) that eases in and out of each key and
/// holds the last value afterwards.
fn keyframes(
    device: &IDCompositionDesktopDevice,
    keys: impl Iterator<Item = (f32, f32)>,
) -> Result<IDCompositionAnimation> {
    unsafe {
        let curve = device.CreateAnimation()?;
        let mut previous: Option<(f32, f32)> = None;
        for (time, value) in keys {
            if let Some((start, from)) = previous {
                let (d, delta) = (time - start, value - from);
                curve.AddCubic(
                    start as f64,
                    from,
                    0.,
                    3. * delta / d.powi(2),
                    -2. * delta / d.powi(3),
                )?;
            }
            previous = Some((time, value));
        }
        if let Some((time, value)) = previous {
            curve.End(time as f64, value)?;
        }
        Ok(curve)
    }
}

impl IconLayer {
    fn matches(&self, icon: &DockIcon<'_>) -> bool {
        self.path.as_deref() == icon.path && self.label == icon.label && self.size == icon.size
    }

    fn animate(
        &mut self,
        device: &IDCompositionDesktopDevice,
        hover: bool,
        lift: f32,
    ) -> Result<()> {
        let old_goal = if self.hovered { self.lift } else { 1.0 };
        let goal = if hover { lift } else { 1.0 };
        let remaining = (1.0 - self.start_time.elapsed().as_secs_f32() / DURATION).clamp(0., 1.);
        let from = old_goal + (self.start_scale - old_goal) * remaining.powi(3);
        unsafe {
            let curve = ease(device, from, goal, DURATION)?;
            self.scale.SetScaleX(&curve)?;
            self.scale.SetScaleY(&curve)?;
        }
        self.start_time = Instant::now();
        self.start_scale = from;
        self.hovered = hover;
        self.lift = lift;
        Ok(())
    }
}

impl Renderer {
    /// Both HWNDs belong to this UI thread and outlive the renderer.
    pub fn new(top: HWND, bottom: HWND, dark: bool) -> Result<Self> {
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
                metric_layer: None,
                menu: None,
                panel: None,
                about: None,
                launchpad: None,
                palette: Palette::new(dark),
                mark: None,
            })
        }
    }

    /// Switches between the light and dark palette. Returns whether it changed; the bars
    /// repaint on their next draw, and the caller repaints the open settings panel.
    pub fn set_dark(&mut self, dark: bool) -> bool {
        let palette = Palette::new(dark);
        if self.palette == palette {
            return false;
        }
        self.palette = palette;
        self.top_state = None;
        self.bottom_size = None;
        // Icon layers hold fallback tiles painted in the old colors.
        self.icons.clear();
        true
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

    /// Each stamp identifies the current tray image and its attention indicator.
    /// `marks[i]` is drawn in `layout.system[i]`, with a backing behind the target whose
    /// menu is `open` and a fainter one behind the `hover` target. The overflow button
    /// points down while its menu is closed and up while it is open.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_top(
        &mut self,
        layout: &StatusLayout,
        dpi: u32,
        title: &str,
        time: &str,
        icons: &[TrayImage],
        marks: &[Mark],
        open: Option<StatusHit>,
        hover: Option<StatusHit>,
        metrics: &[Indicator],
    ) -> Result<()> {
        let metrics_changed = self.update_metrics(layout, dpi, metrics)?;
        let state = TopState {
            layout: layout.clone(),
            dpi,
            title: title.into(),
            time: time.into(),
            stamps: icons.iter().map(|image| image.stamp).collect(),
            marks: marks.to_vec(),
            open,
            hover,
        };
        if self.top_state.as_ref() == Some(&state) {
            if metrics_changed {
                unsafe { self.device.Commit()? };
            }
            return Ok(());
        }
        let s = dpi as f32 / 96.;
        let dip = |px: i32| px as f32 / s;
        let w = dip(layout.width);
        let h = dip(layout.height);
        // Every target shares one rounded backing: faint while hovered, stronger while
        // its dropdown is open.
        let open_slot = open.and_then(|hit| layout.target(hit));
        let hover_slot = hover
            .and_then(|hit| layout.target(hit))
            .filter(|&slot| Some(slot) != open_slot);
        let backing = |c: &Canvas, slot: Rect, alpha: f32| {
            let (l, r) = (dip(slot.x) + 2., dip(slot.x + slot.width) - 2.);
            c.fill(l, 4., r, h - 4., PILL_RADIUS, c.p.line, alpha)
        };
        let surface = paint(
            &self.device,
            self.palette,
            layout.width,
            layout.height,
            dpi,
            |c| {
                c.bar_glass(w, h, 0.)?;
                c.fill(0., h - dip(1), w, h, 0., c.p.line, 0.12)?;
                let t = layout.title;
                c.text(
                    &self.write,
                    title,
                    c.rect(dip(t.x), 0., dip(t.x + t.width), h),
                    13.,
                    true,
                    c.p.ink,
                )?;
                if let Some(slot) = hover_slot {
                    backing(c, slot, 0.08)?;
                }
                if let Some(slot) = open_slot {
                    backing(c, slot, 0.15)?;
                }
                let icon_size = (16. * s).round();
                for (slot, image) in layout.tray.iter().zip(icons) {
                    let r = slot.rect;
                    let bitmap = image.icon.and_then(|icon| self.hicon_bitmap(icon).ok());
                    match bitmap {
                        Some(bitmap) => {
                            let left = (r.x as f32 + (r.width as f32 - icon_size) / 2.).round();
                            let top = (r.y as f32 + (r.height as f32 - icon_size) / 2.).round();
                            c.bitmap(&bitmap, c.pixels(left, top, icon_size))?;
                        }
                        // An icon without a usable image keeps its slot visible and clickable.
                        None => {
                            let x = dip(r.x + r.width / 2);
                            c.round(x - 3., h / 2. - 3., x + 3., h / 2. + 3., 3., c.p.muted)?
                        }
                    }
                    if image.needs_attention {
                        let x = dip(r.x + r.width / 2) + 7.;
                        let y = h / 2. - 7.;
                        c.round(x - 3.5, y - 3.5, x + 3.5, y + 3.5, 3.5, c.p.bar)?;
                        c.round(x - 2.5, y - 2.5, x + 2.5, y + 2.5, 2.5, c.p.danger)?;
                    }
                }
                if let Some(r) = layout.overflow {
                    let x = (dip(r.x) + dip(r.x + r.width)) / 2.;
                    let expanded = open == Some(StatusHit::Overflow);
                    let (symbol, ink) = match (expanded, hover == Some(StatusHit::Overflow)) {
                        (true, _) => (Symbol::ChevronUp, c.p.ink),
                        (false, true) => (Symbol::ChevronDown, c.p.ink),
                        (false, false) => (Symbol::ChevronDown, c.p.muted),
                    };
                    c.symbol(symbol, x, h / 2., OVERFLOW_ICON, ink, 1.)?;
                }
                for (&(_, r), mark) in layout.system.iter().zip(marks) {
                    let (l, right) = (dip(r.x), dip(r.x + r.width));
                    let (x, y) = ((l + right) / 2., h / 2.);
                    match *mark {
                        Mark::Icon { symbol, dim } => {
                            let alpha = if dim { 0.4 } else { 1. };
                            c.symbol(symbol, x, y, BAR_ICON, c.p.ink, alpha)?;
                        }
                        Mark::Battery { symbol, percent } => {
                            let weight = icon::weight(BAR_ICON);
                            let Some(percent) = percent else {
                                c.symbol_weighted(symbol, x, y, BATTERY_ICON, weight, c.p.ink, 1.)?;
                                continue;
                            };
                            c.symbol_weighted(
                                symbol,
                                l + 15.,
                                y,
                                BATTERY_ICON,
                                weight,
                                c.p.ink,
                                1.,
                            )?;
                            c.draw_text(
                                &self.write,
                                &format!("{percent}%"),
                                c.rect(l + 27., 0., right - 2., h),
                                Text {
                                    size: 11.5,
                                    ..Text::default()
                                },
                                c.p.ink,
                                1.,
                            )?;
                        }
                    }
                }
                let r = layout.clock;
                c.text(
                    &self.write,
                    time,
                    c.rect(dip(r.x), 0., dip(r.x + r.width), h),
                    13.,
                    true,
                    c.p.ink,
                )
            },
        )?;
        unsafe { self.top.root.SetContent(&surface)? };
        self.place_mark(layout.height, dpi)?;
        unsafe { self.device.Commit()? };
        self.top_state = Some(state);
        Ok(())
    }

    /// Paints the DeskIsle mark, a ring as round as the controls beside it, onto its own
    /// layer above the bar. The layer is kept while the bar's height, DPI and palette are.
    fn place_mark(&mut self, height: i32, dpi: u32) -> Result<()> {
        let key = (height, dpi, self.palette);
        if self.mark.as_ref().is_some_and(|mark| mark.key == key) {
            return Ok(());
        }
        let s = dpi as f32 / 96.;
        // Whole-pixel offsets keep the ring as crisp as when it was painted into the bar.
        let (cx, cy) = (MARK_X * s, height as f32 / 2.);
        let left = (cx - MARK_BOX / 2. * s).floor();
        let top = (cy - MARK_BOX / 2. * s).floor();
        let (x, y) = (cx - left, cy - top);
        let edge = (MARK_BOX * s).ceil() as i32 + 1;
        let surface = paint(&self.device, self.palette, edge, edge, dpi, |c| {
            c.ring(x / s, y / s, MARK_RADIUS, MARK_STROKE, c.p.accent)
        })?;
        unsafe {
            if let Some(old) = self.mark.take() {
                self.top.root.RemoveVisual(&old.visual)?;
            }
            let visual = self.device.CreateVisual()?;
            visual.SetContent(&surface)?;
            visual.SetOffsetX2(left)?;
            visual.SetOffsetY2(top)?;
            let scale = self.device.CreateScaleTransform()?;
            scale.SetCenterX2(x)?;
            scale.SetCenterY2(y)?;
            scale.SetScaleX2(1.)?;
            scale.SetScaleY2(1.)?;
            visual.SetTransform(&scale)?;
            self.top.root.AddVisual(&visual, true, None)?;
            self.mark = Some(MarkLayer { visual, scale, key });
        }
        Ok(())
    }

    /// Squashes and stretches the DeskIsle mark once, as feedback for a click. A click
    /// during the bounce starts it over.
    pub fn bounce_mark(&self) -> Result<()> {
        let Some(mark) = &self.mark else {
            return Ok(());
        };
        let wide = keyframes(&self.device, MARK_BOUNCE.iter().map(|&(t, x, _)| (t, x)))?;
        let tall = keyframes(&self.device, MARK_BOUNCE.iter().map(|&(t, _, y)| (t, y)))?;
        unsafe {
            mark.scale.SetScaleX(&wide)?;
            mark.scale.SetScaleY(&tall)?;
            self.device.Commit()
        }
    }

    pub fn draw_dock(
        &mut self,
        width: i32,
        height: i32,
        dpi: u32,
        items: &[DockIcon<'_>],
        hover: Option<usize>,
        lift: f32,
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
            let surface = paint(&self.device, self.palette, width, height, dpi, |c| {
                c.bar_glass(w, h, 18.)?;
                c.stroke(0., 0., w, h, 18., c.p.line, 0.16)?;
                for icon in items.iter().filter(|i| i.running) {
                    let x = icon.center_x as f32 / s;
                    let half = if icon.active { 5. } else { 2. };
                    if icon.active {
                        // A soft wash behind the foreground application's icon.
                        c.fill(x - 21., 5., x + 21., h - 4., 11., c.p.accent, 0.07)?;
                        c.fill(x - 9., h - 9.5, x + 9., h - 3.5, 3., c.p.accent, 0.18)?;
                    }
                    c.round(
                        x - half,
                        h - 8.,
                        x + half,
                        h - 5.,
                        1.5,
                        if icon.active { c.p.accent } else { c.p.muted },
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
                layer.animate(&self.device, hovered, lift)?;
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
        let surface = paint(&self.device, self.palette, item.size, item.size, 96, |c| {
            let Some(path) = item.path else {
                c.gradient(
                    0.,
                    0.,
                    size,
                    size,
                    size * 0.24,
                    ((108, 139, 250), 1.),
                    ((67, 79, 188), 1.),
                )?;
                return c.symbol(
                    Symbol::Apps,
                    size * 0.5,
                    size * 0.5,
                    size * 0.83,
                    (255, 255, 255),
                    1.,
                );
            };
            if let Some(bitmap) = self.shell_icon(path)? {
                c.bitmap(&bitmap, c.rect(0., 0., size, size))?;
            } else {
                // Shell can lose access to an executable after enumeration. Keep its slot identifiable.
                c.round(0., 0., size, size, size * 0.26, c.p.edge)?;
                let initial = item.label.chars().next().unwrap_or('?').to_string();
                c.text(
                    &self.write,
                    &initial,
                    c.rect(size * 0.24, 0., size, size),
                    size * 0.56,
                    true,
                    c.p.ink,
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
                path: item.path.map(Path::to_owned),
                label: item.label.into(),
                size: item.size,
                center: (0, 0),
                visual,
                scale,
                hovered: false,
                lift: 1.,
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
            let copied = self.hicon_bitmap(info.hIcon);
            DestroyIcon(info.hIcon)?;
            Ok(Some(copied?))
        }
    }

    /// Copies the pixels of `icon`, which the caller keeps owning.
    fn hicon_bitmap(&self, icon: HICON) -> Result<IWICBitmapSource> {
        unsafe {
            let copied = self.imaging.CreateBitmapFromHICON(icon)?;
            WICConvertBitmapSource(&GUID_WICPixelFormat32bppPBGRA, &copied)
        }
    }
}
