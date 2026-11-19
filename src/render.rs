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
        WindowsAndMessaging::{DestroyIcon, HICON},
    },
};
use windows::core::{Error, HSTRING, Interface, Result, w};

use crate::dock::StatusLayout;
use crate::menu::{self, Entry, Icon, Menu, MenuLayout};

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
    top_state: Option<TopState>,
    menu: Option<MenuView>,
}

#[derive(PartialEq)]
struct TopState {
    layout: StatusLayout,
    dpi: u32,
    title: String,
    time: String,
    stamps: Vec<u64>,
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

const MENU_RADIUS: f32 = 10.;
const OPEN_DURATION: f32 = 0.12;
const HOVER_DURATION: f32 = 0.09;

/// The open context menu. Its surfaces are painted once; pointer movement only animates
/// the highlight's offset and opacity in the compositor.
struct MenuView {
    // The target must outlive all commits to its root.
    _target: IDCompositionTarget,
    _root: IDCompositionVisual2,
    glow: IDCompositionVisual2,
    glow_fade: IDCompositionEffectGroup,
    accent: IDCompositionSurface,
    danger: IDCompositionSurface,
    scale: f32,
    rows: Vec<(f32, f32)>,
    hover: Option<usize>,
    /// The highlight's vertical offset in pixels.
    y: Tween,
}

/// Mirrors an [`ease`] handed to the compositor, so a new one starts where it is now.
struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: f32,
}

impl Tween {
    fn new(from: f32, to: f32, duration: f32) -> Self {
        Self {
            from,
            to,
            start: Instant::now(),
            duration,
        }
    }

    fn at(value: f32) -> Self {
        Self::new(value, value, 1.)
    }

    fn now(&self) -> f32 {
        eased(self.from, self.to, self.start, self.duration)
    }
}

const LIFTED: f32 = 1.24;
const DURATION: f32 = 0.14;

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

/// The value of an [`ease`] started at `start`, as the compositor shows it now.
fn eased(from: f32, to: f32, start: Instant, duration: f32) -> f32 {
    let t = (start.elapsed().as_secs_f32() / duration).clamp(0., 1.);
    to + (from - to) * (1. - t).powi(3)
}

impl IconLayer {
    fn matches(&self, icon: &DockIcon<'_>) -> bool {
        self.path == icon.path && self.label == icon.label && self.size == icon.size
    }

    fn animate(&mut self, device: &IDCompositionDesktopDevice, hover: bool) -> Result<()> {
        let old_goal = if self.hovered { LIFTED } else { 1.0 };
        let goal = if hover { LIFTED } else { 1.0 };
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
                menu: None,
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

    /// `icons[i]` is drawn in `layout.tray[i]`; the stamp identifies its current image.
    pub fn draw_top(
        &mut self,
        layout: &StatusLayout,
        dpi: u32,
        title: &str,
        time: &str,
        icons: &[(Option<HICON>, u64)],
    ) -> Result<()> {
        let state = TopState {
            layout: layout.clone(),
            dpi,
            title: title.into(),
            time: time.into(),
            stamps: icons.iter().map(|&(_, stamp)| stamp).collect(),
        };
        if self.top_state.as_ref() == Some(&state) {
            return Ok(());
        }
        let s = dpi as f32 / 96.;
        let dip = |px: i32| px as f32 / s;
        let w = dip(layout.width);
        let h = dip(layout.height);
        let surface = paint(&self.device, layout.width, layout.height, dpi, |c| {
            // Flush with the screen edge: a solid strip with a hairline toward the desktop.
            c.round(0., 0., w, h, 0., BAR)?;
            c.round(0., h - 1., w, h, 0., EDGE)?;
            c.round(13., h / 2. - 5., 17., h / 2. + 5., 2., MINT)?;
            c.round(19., h / 2. - 2., 23., h / 2. + 5., 2., MINT)?;
            let t = layout.title;
            c.text(
                &self.write,
                title,
                c.rect(dip(t.x), 0., dip(t.x + t.width), h),
                13.,
                true,
                INK,
            )?;
            let icon_size = (16. * s).round();
            for (slot, &(icon, _)) in layout.tray.iter().zip(icons) {
                let r = slot.rect;
                let bitmap = icon.and_then(|icon| self.hicon_bitmap(icon).ok());
                match bitmap {
                    Some(bitmap) => unsafe {
                        let left = (r.x as f32 + (r.width as f32 - icon_size) / 2.).round();
                        let top = (r.y as f32 + (r.height as f32 - icon_size) / 2.).round();
                        let texture = c.ctx.CreateBitmapFromWicBitmap(&bitmap, None)?;
                        c.ctx.DrawBitmap(
                            &texture,
                            Some(&c.pixels(left, top, icon_size)),
                            1.,
                            D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                            None,
                            None,
                        );
                    },
                    // An icon without a usable image keeps its slot visible and clickable.
                    None => {
                        let x = dip(r.x + r.width / 2);
                        c.round(x - 3., h / 2. - 3., x + 3., h / 2. + 3., 3., MUTED)?
                    }
                }
            }
            if let Some(r) = layout.overflow {
                c.glyph(
                    &self.write,
                    "\u{E70D}",
                    c.rect(dip(r.x), 0., dip(r.x + r.width), h),
                    10.,
                    MUTED,
                )?;
            }
            let r = layout.clock;
            c.text(
                &self.write,
                time,
                c.rect(dip(r.x), 0., dip(r.x + r.width), h),
                13.,
                true,
                INK,
            )
        })?;
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
                    if icon.active {
                        // A soft wash behind the foreground application's icon.
                        c.fill(x - 21., 5., x + 21., h - 4., 11., MINT, 0.07)?;
                        c.fill(x - 9., h - 9.5, x + 9., h - 3.5, 3., MINT, 0.18)?;
                    }
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

    /// Width of one line of text in DIPs, estimated when DirectWrite cannot lay it out.
    pub fn measure(&self, text: &str, size: f32, bold: bool) -> f32 {
        let style = Text {
            size,
            bold,
            ..Text::default()
        };
        let units = text.encode_utf16().collect::<Vec<_>>();
        let measured = text_format(&self.write, style, size).and_then(|format| unsafe {
            let layout = self
                .write
                .CreateTextLayout(&units, &format, f32::MAX, f32::MAX)?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics)?;
            Ok(metrics.widthIncludingTrailingWhitespace)
        });
        measured.unwrap_or_else(|_| text.chars().count() as f32 * size * 0.62)
    }

    /// Paints `menu` into `hwnd`, a hidden menu window sized to `layout`, and fades it in.
    /// `tray` resolves tray icon indices while the tray host keeps owning the handles.
    pub fn open_menu<C>(
        &mut self,
        hwnd: HWND,
        menu: &Menu<C>,
        layout: &MenuLayout,
        dpi: u32,
        tray: &dyn Fn(usize) -> Option<HICON>,
    ) -> Result<()> {
        self.close_menu();
        let (width, height) = layout.pixels();
        let (left, top, right, bottom) = layout.panel();
        let background = paint(&self.device, width, height, dpi, |c| {
            c.shadow(left, top, right, bottom, MENU_RADIUS, menu::SHADOW - 2.)?;
            c.gradient(
                left,
                top,
                right,
                bottom,
                MENU_RADIUS,
                (PANEL_TOP, 0.985),
                (PANEL, 0.985),
            )?;
            c.stroke(left, top, right, bottom, MENU_RADIUS, (255, 255, 255), 0.12)
        })?;
        let content = paint(&self.device, width, height, dpi, |c| {
            for (entry, &(row_top, row_bottom)) in menu.entries.iter().zip(&layout.rows) {
                self.paint_entry(c, entry, layout, row_top, row_bottom, tray)?;
            }
            Ok(())
        })?;
        let (row_left, _, row_right, _) = layout.row(0);
        let highlight = |accent: Rgb| {
            paint(
                &self.device,
                ((row_right - row_left) * layout.scale).round() as i32,
                (menu::ITEM_HEIGHT * layout.scale).round() as i32,
                dpi,
                |c| {
                    let (w, h) = (row_right - row_left, menu::ITEM_HEIGHT);
                    c.fill(0., 1., w, h - 1., 6., accent, 0.17)?;
                    c.stroke(0., 1., w, h - 1., 6., accent, 0.22)?;
                    // A short accent bar at the leading edge marks the selected row.
                    c.round(2., h / 2. - 7., 5., h / 2. + 7., 1.5, accent)
                },
            )
        };
        let accent = highlight(MINT)?;
        let danger = highlight(DANGER)?;
        unsafe {
            let target = self.device.CreateTargetForHwnd(hwnd, true)?;
            let root = self.device.CreateVisual()?;
            root.SetContent(&background)?;
            let glow = self.device.CreateVisual()?;
            glow.SetContent(&accent)?;
            glow.SetOffsetX2((row_left * layout.scale).round())?;
            let glow_fade = self.device.CreateEffectGroup()?;
            glow_fade.SetOpacity2(0.)?;
            glow.SetEffect(&glow_fade)?;
            let text = self.device.CreateVisual()?;
            text.SetContent(&content)?;
            root.AddVisual(&glow, true, None)?;
            root.AddVisual(&text, true, None)?;

            // Open with a quick fade and a slight zoom toward the full size.
            let fade = self.device.CreateEffectGroup()?;
            fade.SetOpacity(&ease(&self.device, 0., 1., OPEN_DURATION)?)?;
            root.SetEffect(&fade)?;
            let zoom = self.device.CreateScaleTransform()?;
            zoom.SetCenterX2(width as f32 / 2.)?;
            zoom.SetCenterY2(height as f32 / 2.)?;
            let grow = ease(&self.device, 0.96, 1., OPEN_DURATION)?;
            zoom.SetScaleX(&grow)?;
            zoom.SetScaleY(&grow)?;
            root.SetTransform(&zoom)?;
            target.SetRoot(&root)?;
            self.device.Commit()?;
            self.menu = Some(MenuView {
                _target: target,
                _root: root,
                glow,
                glow_fade,
                accent,
                danger,
                scale: layout.scale,
                rows: layout.rows.clone(),
                hover: None,
                y: Tween::at(0.),
            });
        }
        Ok(())
    }

    fn paint_entry<C>(
        &self,
        c: &Canvas,
        entry: &Entry<C>,
        layout: &MenuLayout,
        top: f32,
        bottom: f32,
        tray: &dyn Fn(usize) -> Option<HICON>,
    ) -> Result<()> {
        let (_, _, row_right, _) = layout.row(0);
        let (row_left, _, _, _) = layout.row(0);
        let middle = (top + bottom) / 2.;
        let icon_x = row_left + 8. + 12.;
        match entry {
            Entry::Separator => c.fill(
                row_left + 8.,
                middle - 0.5,
                row_right - 8.,
                middle + 0.5,
                0.,
                (255, 255, 255),
                0.09,
            ),
            Entry::Header {
                title,
                subtitle,
                icon,
            } => {
                let size = 30.;
                let x = row_left + 6.;
                let drawn = self.paint_icon(c, icon, x, middle - size / 2., size, tray)?;
                let text_x = if drawn { x + size + 10. } else { row_left + 8. };
                let lines = if subtitle.is_empty() { 0. } else { 9. };
                c.draw_text(
                    &self.write,
                    title,
                    c.rect(
                        text_x,
                        middle - 12. - lines,
                        row_right - 8.,
                        middle + 12. - lines,
                    ),
                    Text {
                        size: menu::TITLE_SIZE,
                        bold: true,
                        ..Text::default()
                    },
                    INK,
                    1.,
                )?;
                c.draw_text(
                    &self.write,
                    subtitle,
                    c.rect(text_x, middle, row_right - 8., middle + 20.),
                    Text {
                        size: menu::DETAIL_SIZE,
                        ..Text::default()
                    },
                    MUTED,
                    0.85,
                )
            }
            Entry::Item(item) => {
                let enabled = item.command.is_some();
                let (ink, alpha) = match (item.danger, enabled) {
                    (_, false) => (MUTED, 0.5),
                    (true, true) => (DANGER, 1.),
                    (false, true) => (INK, 1.),
                };
                match &item.icon {
                    Icon::Glyph(glyph) => c.draw_text(
                        &self.write,
                        glyph,
                        c.rect(icon_x - 10., top, icon_x + 10., bottom),
                        Text {
                            size: 15.,
                            glyph: true,
                            align: DWRITE_TEXT_ALIGNMENT_CENTER,
                            ..Text::default()
                        },
                        if item.danger { DANGER } else { MUTED },
                        alpha,
                    )?,
                    Icon::Window { active } => {
                        let (half, color) = if *active { (3.5, MINT) } else { (2.5, MUTED) };
                        c.fill(
                            icon_x - half,
                            middle - half,
                            icon_x + half,
                            middle + half,
                            half,
                            color,
                            alpha,
                        )?;
                    }
                    icon => {
                        let size = 18.;
                        self.paint_icon(
                            c,
                            icon,
                            icon_x - size / 2.,
                            middle - size / 2.,
                            size,
                            tray,
                        )?;
                    }
                }
                let check = if layout.checks {
                    menu::CHECK_COLUMN
                } else {
                    0.
                };
                let label_left = row_left + 8. + menu::ICON_COLUMN;
                let label_right = row_right - menu::TEXT_INSET - check;
                let detail_width = if item.detail.is_empty() {
                    0.
                } else {
                    self.measure(&item.detail, menu::DETAIL_SIZE, false) + 16.
                };
                c.draw_text(
                    &self.write,
                    &item.label,
                    c.rect(label_left, top, label_right - detail_width, bottom),
                    Text {
                        size: menu::LABEL_SIZE,
                        ..Text::default()
                    },
                    ink,
                    alpha,
                )?;
                c.draw_text(
                    &self.write,
                    &item.detail,
                    c.rect(label_left, top, label_right, bottom),
                    Text {
                        size: menu::DETAIL_SIZE,
                        align: DWRITE_TEXT_ALIGNMENT_TRAILING,
                        ..Text::default()
                    },
                    MUTED,
                    0.75 * alpha,
                )?;
                if item.checked == Some(true) {
                    let x = row_right - menu::TEXT_INSET - menu::CHECK_COLUMN / 2.;
                    c.glyph(
                        &self.write,
                        "\u{E73E}",
                        c.rect(x - 10., top, x + 10., bottom),
                        13.,
                        MINT,
                    )?;
                }
                Ok(())
            }
        }
    }

    /// Draws a file or tray icon in a DIP square. Returns whether anything was drawn.
    fn paint_icon(
        &self,
        c: &Canvas,
        icon: &Icon,
        x: f32,
        y: f32,
        size: f32,
        tray: &dyn Fn(usize) -> Option<HICON>,
    ) -> Result<bool> {
        let bitmap = match icon {
            Icon::File(path) => self.shell_icon(path).ok().flatten(),
            Icon::Tray(index) => tray(*index).and_then(|icon| self.hicon_bitmap(icon).ok()),
            Icon::Glyph(glyph) => {
                c.fill(x, y, x + size, y + size, size * 0.28, MINT, 0.16)?;
                c.glyph(
                    &self.write,
                    glyph,
                    c.rect(x, y, x + size, y + size),
                    size * 0.5,
                    MINT,
                )?;
                return Ok(true);
            }
            Icon::None | Icon::Window { .. } => None,
        };
        match bitmap {
            Some(bitmap) => {
                // Snap to whole pixels so small icons stay sharp.
                let px = |v: f32| (v * c.scale).round() / c.scale;
                c.bitmap(&bitmap, c.rect(px(x), px(y), px(x + size), px(y + size)))?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Moves the highlight to entry `index`, sliding from the previous one.
    pub fn menu_hover(&mut self, index: Option<usize>, danger: bool) -> Result<()> {
        let Some(view) = self.menu.as_mut() else {
            return Ok(());
        };
        if view.hover == index {
            return Ok(());
        }
        unsafe {
            match (view.hover, index) {
                (_, None) => {
                    view.glow_fade
                        .SetOpacity(&ease(&self.device, 1., 0., HOVER_DURATION)?)?
                }
                (previous, Some(index)) => {
                    let y = (view.rows[index].0 * view.scale).round();
                    view.glow
                        .SetContent(if danger { &view.danger } else { &view.accent })?;
                    if previous.is_some() {
                        let from = view.y.now();
                        view.glow
                            .SetOffsetY(&ease(&self.device, from, y, HOVER_DURATION)?)?;
                        view.y = Tween::new(from, y, HOVER_DURATION);
                    } else {
                        view.glow.SetOffsetY2(y)?;
                        view.y = Tween::at(y);
                        view.glow_fade
                            .SetOpacity(&ease(&self.device, 0., 1., HOVER_DURATION)?)?;
                    }
                }
            }
            self.device.Commit()?;
        }
        view.hover = index;
        Ok(())
    }

    /// Releases the menu's composition target; call before destroying its window.
    pub fn close_menu(&mut self) {
        if self.menu.take().is_some() {
            let _ = unsafe { self.device.Commit() };
        }
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

type Rgb = (u8, u8, u8);
const EDGE: Rgb = (71, 82, 85);
const PANEL: Rgb = (24, 30, 33);
const PANEL_TOP: Rgb = (38, 47, 51);
const DANGER: Rgb = (255, 140, 128);
const BAR: Rgb = (17, 19, 21);
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

    /// A square in physical pixels relative to the surface.
    fn pixels(&self, left: f32, top: f32, size: f32) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.origin.x as f32 + left,
            top: self.origin.y as f32 + top,
            right: self.origin.x as f32 + left + size,
            bottom: self.origin.y as f32 + top + size,
        }
    }

    fn brush_alpha(&self, (r, g, b): Rgb, a: f32) -> Result<ID2D1SolidColorBrush> {
        unsafe { self.ctx.CreateSolidColorBrush(&color(r, g, b, a), None) }
    }

    fn shape(&self, l: f32, t: f32, r: f32, b: f32, radius: f32) -> D2D1_ROUNDED_RECT {
        D2D1_ROUNDED_RECT {
            rect: self.rect(l, t, r, b),
            radiusX: radius * self.scale,
            radiusY: radius * self.scale,
        }
    }

    fn round(&self, l: f32, t: f32, r: f32, b: f32, radius: f32, color: Rgb) -> Result<()> {
        self.fill(l, t, r, b, radius, color, 1.)
    }

    #[allow(clippy::too_many_arguments)]
    fn fill(&self, l: f32, t: f32, r: f32, b: f32, radius: f32, color: Rgb, a: f32) -> Result<()> {
        let shape = self.shape(l, t, r, b, radius);
        unsafe {
            self.ctx
                .FillRoundedRectangle(&shape, &self.brush_alpha(color, a)?)
        };
        Ok(())
    }

    /// A one-pixel line just inside the edges, so it stays crisp at every scale.
    #[allow(clippy::too_many_arguments)]
    fn stroke(
        &self,
        l: f32,
        t: f32,
        r: f32,
        b: f32,
        radius: f32,
        color: Rgb,
        a: f32,
    ) -> Result<()> {
        let half = 0.5 / self.scale;
        let shape = self.shape(l + half, t + half, r - half, b - half, radius);
        unsafe {
            self.ctx
                .DrawRoundedRectangle(&shape, &self.brush_alpha(color, a)?, 1., None)
        };
        Ok(())
    }

    /// Fills a rounded rectangle with a vertical gradient from `top` to `bottom`.
    #[allow(clippy::too_many_arguments)]
    fn gradient(
        &self,
        l: f32,
        t: f32,
        r: f32,
        b: f32,
        radius: f32,
        top: (Rgb, f32),
        bottom: (Rgb, f32),
    ) -> Result<()> {
        let shape = self.shape(l, t, r, b, radius);
        let stop = |((r, g, b), a): (Rgb, f32), position| D2D1_GRADIENT_STOP {
            position,
            color: color(r, g, b, a),
        };
        unsafe {
            // The device context overload takes extra color-space arguments; the render
            // target one interpolates in gamma 2.2 like the rest of the UI.
            let target: &ID2D1RenderTarget = &self.ctx;
            let stops = target.CreateGradientStopCollection(
                &[stop(top, 0.), stop(bottom, 1.)],
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            let mut line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES::default();
            line.startPoint.X = shape.rect.left;
            line.startPoint.Y = shape.rect.top;
            line.endPoint.X = shape.rect.left;
            line.endPoint.Y = shape.rect.bottom;
            let brush = self.ctx.CreateLinearGradientBrush(&line, None, &stops)?;
            self.ctx.FillRoundedRectangle(&shape, &brush);
        }
        Ok(())
    }

    /// A soft drop shadow under a rounded rectangle, stacked from translucent layers.
    /// It is painted once into the surface, so it costs nothing per frame.
    fn shadow(&self, l: f32, t: f32, r: f32, b: f32, radius: f32, spread: f32) -> Result<()> {
        const LAYERS: usize = 10;
        let drop = spread * 0.25;
        for k in 0..LAYERS {
            let d = spread * (1. - k as f32 / LAYERS as f32);
            self.fill(
                l - d,
                t - d + drop,
                r + d,
                b + d + drop,
                radius + d,
                (0, 0, 0),
                0.03,
            )?;
        }
        Ok(())
    }

    fn panel(&self, w: f32, h: f32, radius: f32) -> Result<()> {
        self.gradient(0., 0., w, h, radius, (PANEL_TOP, 0.97), (PANEL, 0.97))?;
        self.stroke(0., 0., w, h, radius, (255, 255, 255), 0.11)
    }

    fn bitmap(&self, bitmap: &IWICBitmapSource, rect: D2D_RECT_F) -> Result<()> {
        unsafe {
            let texture = self.ctx.CreateBitmapFromWicBitmap(bitmap, None)?;
            self.ctx.DrawBitmap(
                &texture,
                Some(&rect),
                1.,
                D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                None,
                None,
            );
        }
        Ok(())
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
        let style = Text {
            size,
            bold,
            ..Text::default()
        };
        self.draw_text(factory, text, rect, style, color, 1.)
    }

    /// Draws an icon-font glyph centered in `rect`. Windows 11 ships Segoe Fluent Icons.
    fn glyph(
        &self,
        factory: &IDWriteFactory,
        glyph: &str,
        rect: D2D_RECT_F,
        size: f32,
        color: Rgb,
    ) -> Result<()> {
        let style = Text {
            size,
            glyph: true,
            align: DWRITE_TEXT_ALIGNMENT_CENTER,
            ..Text::default()
        };
        self.draw_text(factory, glyph, rect, style, color, 1.)
    }

    fn draw_text(
        &self,
        factory: &IDWriteFactory,
        text: &str,
        rect: D2D_RECT_F,
        style: Text,
        color: Rgb,
        alpha: f32,
    ) -> Result<()> {
        if rect.right <= rect.left || text.is_empty() {
            return Ok(());
        }
        unsafe {
            let format = text_format(factory, style, style.size * self.scale)?;
            self.ctx.DrawText(
                &text.encode_utf16().collect::<Vec<_>>(),
                &format,
                &rect,
                &self.brush_alpha(color, alpha)?,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }
}

fn color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a,
    }
}

#[derive(Clone, Copy)]
struct Text {
    size: f32,
    bold: bool,
    glyph: bool,
    align: DWRITE_TEXT_ALIGNMENT,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            size: 13.,
            bold: false,
            glyph: false,
            align: DWRITE_TEXT_ALIGNMENT_LEADING,
        }
    }
}

/// A single vertically centered line that ends in an ellipsis when clipped. `size` is in
/// the units the caller draws or measures in.
fn text_format(factory: &IDWriteFactory, style: Text, size: f32) -> Result<IDWriteTextFormat> {
    let font = if style.glyph {
        w!("Segoe Fluent Icons")
    } else {
        w!("Segoe UI")
    };
    unsafe {
        let format = factory.CreateTextFormat(
            font,
            None,
            if style.bold {
                DWRITE_FONT_WEIGHT_SEMI_BOLD
            } else {
                DWRITE_FONT_WEIGHT_NORMAL
            },
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!(""),
        )?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        format.SetTextAlignment(style.align)?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        let ellipsis = factory.CreateEllipsisTrimmingSign(&format)?;
        format.SetTrimming(
            &DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                ..Default::default()
            },
            &ellipsis,
        )?;
        Ok(format)
    }
}
