mod canvas;
mod menu;
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
use crate::ui::layout::{StatusHit, StatusLayout, SystemItem};
use crate::ui::system::Mark;

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
    pub path: &'a Path,
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
    menu: Option<MenuView>,
    panel: Option<PanelView>,
    palette: Palette,
}

#[derive(PartialEq)]
struct TopState {
    layout: StatusLayout,
    dpi: u32,
    title: String,
    time: String,
    stamps: Vec<u64>,
    marks: Vec<Mark>,
    open: Option<SystemItem>,
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

struct IconLayer {
    path: PathBuf,
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

impl IconLayer {
    fn matches(&self, icon: &DockIcon<'_>) -> bool {
        self.path == icon.path && self.label == icon.label && self.size == icon.size
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
                menu: None,
                panel: None,
                palette: Palette::new(dark),
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
    /// `marks[i]` is drawn in `layout.system[i]`, with a backing behind the control
    /// whose flyout is `open` and a fainter one behind the `hover` target.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_top(
        &mut self,
        layout: &StatusLayout,
        dpi: u32,
        title: &str,
        time: &str,
        icons: &[TrayImage],
        marks: &[Mark],
        open: Option<SystemItem>,
        hover: Option<StatusHit>,
    ) -> Result<()> {
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
            return Ok(());
        }
        let s = dpi as f32 / 96.;
        let dip = |px: i32| px as f32 / s;
        let w = dip(layout.width);
        let h = dip(layout.height);
        // Every target shares one rounded backing: faint while hovered, stronger while
        // its dropdown is open.
        let open_slot = open.and_then(|item| layout.target(StatusHit::System(item)));
        let hover_slot = hover
            .and_then(|hit| layout.target(hit))
            .filter(|&slot| Some(slot) != open_slot);
        let backing = |c: &Canvas, slot: Rect, alpha: f32| {
            let (l, r) = (dip(slot.x) + 2., dip(slot.x + slot.width) - 2.);
            c.fill(l, 4., r, h - 4., PILL_RADIUS, c.p.line, alpha)
        };
        let glyph_text = Text {
            size: 15.,
            glyph: true,
            align: DWRITE_TEXT_ALIGNMENT_CENTER,
            ..Text::default()
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
                // The DeskIsle mark: a ring, as round as the controls beside it.
                c.ring(18., h / 2., 6., 1.75, c.p.accent)?;
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
                    c.glyph(
                        &self.write,
                        "\u{E70D}",
                        c.rect(dip(r.x), 0., dip(r.x + r.width), h),
                        10.,
                        c.p.muted,
                    )?;
                }
                for (&(_, r), mark) in layout.system.iter().zip(marks) {
                    let (l, right) = (dip(r.x), dip(r.x + r.width));
                    match *mark {
                        Mark::Glyph { glyph, dim } => c.draw_text(
                            &self.write,
                            glyph,
                            c.rect(l, 0., right, h),
                            glyph_text,
                            c.p.ink,
                            if dim { 0.4 } else { 1. },
                        )?,
                        Mark::Battery { glyph, percent } => {
                            let Some(percent) = percent else {
                                let area = c.rect(l, 0., right, h);
                                c.draw_text(&self.write, glyph, area, glyph_text, c.p.ink, 1.)?;
                                continue;
                            };
                            let area = c.rect(l + 3., 0., l + 27., h);
                            c.draw_text(&self.write, glyph, area, glyph_text, c.p.ink, 1.)?;
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
                        Mark::Switches => {
                            // Two filled capsules with round knobs, the upper one on.
                            let x = (l + right) / 2.;
                            let y = h / 2.;
                            for (top, on) in [(y - 7., true), (y + 1., false)] {
                                let (bottom, alpha) = (top + 6., if on { 1. } else { 0.3 });
                                c.fill(x - 7., top, x + 7., bottom, 3., c.p.ink, alpha)?;
                                let (knob, color) = if on {
                                    (x + 3.5, c.p.bar)
                                } else {
                                    (x - 3.5, c.p.ink)
                                };
                                c.round(knob - 2., top + 1., knob + 2., bottom - 1., 2., color)?;
                            }
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
        unsafe {
            self.top.root.SetContent(&surface)?;
            self.device.Commit()?;
        }
        self.top_state = Some(state);
        Ok(())
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
            if let Some(bitmap) = self.shell_icon(item.path)? {
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
                path: item.path.into(),
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
