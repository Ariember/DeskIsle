mod about;
mod canvas;
mod launchpad;
mod menu;
mod metrics;
mod panel;
mod preview;

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use windows::Win32::{
    Foundation::{D2DERR_RECREATE_TARGET, E_UNEXPECTED, HMODULE, HWND},
    Graphics::{
        Direct2D::{D2D1CreateDevice, ID2D1Device},
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
use crate::settings::{Key, Settings, TopBarBackground};
use crate::ui::icon::{self, Symbol};
use crate::ui::layout::{self, StatusHit, StatusLayout};
use crate::ui::panel::Area;
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
    d2d: ID2D1Device,
    device: IDCompositionDesktopDevice,
    top: Bar,
    bottom: Bar,
    write: IDWriteFactory,
    imaging: IWICImagingFactory,
    icons: Vec<IconLayer>,
    bottom_size: Option<Dimensions>,
    top_state: Option<TopState>,
    metric_layer: Option<metrics::MetricLayer>,
    menu: Option<MenuView>,
    panel: Option<PanelView>,
    about: Option<AboutView>,
    launchpad: Option<launchpad::LaunchpadView>,
    palette: Palette,
    top_background: TopBarBackground,
    color_tray_icons: bool,
    traditional_panel: bool,
    mark: Option<MarkLayer>,
}

#[derive(PartialEq)]
struct TopState {
    layout: StatusLayout,
    background: TopBarBackground,
    frosted: bool,
    color_tray_icons: bool,
    immersive_color: Option<u32>,
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

/// The DeskIsle mark on its own layer above the status bar, so the compositor can animate
/// it without repainting the bar. A container holds the ring and the core that grows
/// inside it while the system menu is open, each scaled about the ring's center.
struct MarkLayer {
    visual: IDCompositionVisual2,
    ring: IDCompositionScaleTransform,
    core: IDCompositionScaleTransform,
    core_size: Tween,
    open: bool,
    /// The bar height in pixels, DPI, palette and shadow state the ring was painted for.
    key: (i32, u32, Palette, bool),
}

struct IconLayer {
    path: Option<PathBuf>,
    label: String,
    size: i32,
    center: (i32, i32),
    /// Holds the icon and its running indicator, and slides between slots.
    visual: IDCompositionVisual2,
    /// Lifts the icon, not its indicator, around the icon's lower middle.
    scale: IDCompositionScaleTransform,
    lift: Tween,
    x: Tween,
    /// Darkens the launchpad while an app is held over it.
    shade: Option<(IDCompositionEffectGroup, Tween)>,
    /// The running indicator below the icon.
    dot: Option<IDCompositionVisual2>,
    /// Active state, slot top, bar height and DPI the indicator was placed for.
    dot_key: Option<(bool, i32, i32, u32)>,
}

const DURATION: f32 = 0.14;
/// How long an icon takes to slide into the slot a drag makes for it, in seconds.
const SLIDE: f32 = 0.22;
/// The launchpad's extra lift and darkening while an app is held over it.
const DROP_LIFT: f32 = 1.06;
const DROP_SHADE: f32 = 0.32;
/// The running indicator's layer in DIPs: its size, and how far its top sits above the
/// Dock's bottom edge.
const DOT_BOX: (f32, f32) = (20., 8.);
const DOT_TOP: f32 = 10.5;
/// Vertical offset in DIPs for the soft shadow behind translucent top-bar text and icons.
pub(super) const TOP_SHADOW_Y: f32 = 0.85;

/// Translucent desktop top-bar styles draw a subtle drop shadow so text and icons stay
/// crisp over varied wallpaper tones.
pub(super) fn top_bar_shadow(style: TopBarBackground, immersive_color: Option<u32>) -> bool {
    immersive_color.is_none()
        && matches!(
            style,
            TopBarBackground::Tinted | TopBarBackground::Transparent
        )
}

/// Corner radius of the backing behind a hovered or open status bar target, in DIPs.
const PILL_RADIUS: f32 = 7.;
/// Size of the icons on the status bar, in DIPs. The battery is drawn larger with the
/// same stroke weight, so its body reads at the bar's height.
const BAR_ICON: f32 = layout::BAR_ICON as f32;
const BATTERY_ICON: f32 = layout::BATTERY_ICON as f32;
/// Opacity of a status icon whose feature is unavailable: visibly faded, yet keeping
/// more than 3:1 contrast with the light bar.
const DIM: f32 = 0.55;
/// The DeskIsle mark: a ring centered this far from the bar's leading edge, in DIPs.
const MARK_X: f32 = 18.;
const MARK_RADIUS: f32 = 6.;
const MARK_STROKE: f32 = 1.75;
/// Edge of the mark's layer in DIPs, with room for the ring's antialiased rim.
const MARK_BOX: f32 = 20.;
/// Radius in DIPs of the core that fills the ring while the system menu is open, so the
/// ring reads as an isle with land at its middle. It is painted at this size and scaled
/// down to nothing at rest, so it is crisp whenever it is fully shown.
const MARK_CORE: f32 = 2.5;
/// The ring's press as the menu opens: (seconds, scale). It draws in slightly while the
/// core grows, then settles back to its painted size.
const MARK_PRESS: [(f32, f32); 3] = [(0., 1.), (0.08, 0.86), (0.22, 1.)];
/// Seconds for the core to grow as the menu opens and to recede after it closes.
const MARK_GROW: f32 = 0.22;
const MARK_SHRINK: f32 = 0.16;

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

/// A child of `parent` showing `content`, scaled by `scale` about `center` in the
/// parent's pixels. Returns the transform so the caller can animate the scale.
fn scaled_child(
    device: &IDCompositionDesktopDevice,
    parent: &IDCompositionVisual2,
    content: &IDCompositionSurface,
    (x, y): (f32, f32),
    scale: f32,
) -> Result<IDCompositionScaleTransform> {
    unsafe {
        let visual = device.CreateVisual()?;
        visual.SetContent(content)?;
        let transform = device.CreateScaleTransform()?;
        transform.SetCenterX2(x)?;
        transform.SetCenterY2(y)?;
        transform.SetScaleX2(scale)?;
        transform.SetScaleY2(scale)?;
        visual.SetTransform(&transform)?;
        parent.AddVisual(&visual, true, None)?;
        Ok(transform)
    }
}

/// A property the compositor eases toward `goal` with an ease-out cubic. A new goal
/// starts from wherever the running curve has reached, so an interruption does not jump.
struct Tween {
    from: f32,
    goal: f32,
    start: Instant,
    duration: f32,
}

impl Tween {
    fn new(value: f32) -> Self {
        Self {
            from: value,
            goal: value,
            start: Instant::now(),
            duration: 0.,
        }
    }

    fn value(&self) -> f32 {
        if self.duration <= 0. {
            return self.goal;
        }
        let remaining = (1. - self.start.elapsed().as_secs_f32() / self.duration).clamp(0., 1.);
        self.goal + (self.from - self.goal) * remaining.powi(3)
    }

    /// The curve toward `goal`, or None while that already is the goal.
    fn retarget(
        &mut self,
        device: &IDCompositionDesktopDevice,
        goal: f32,
        duration: f32,
    ) -> Result<Option<IDCompositionAnimation>> {
        if (goal - self.goal).abs() < 1e-3 {
            return Ok(None);
        }
        let from = self.value();
        let curve = ease(device, from, goal, duration)?;
        *self = Self {
            from,
            goal,
            start: Instant::now(),
            duration,
        };
        Ok(Some(curve))
    }
}

impl IconLayer {
    fn matches(&self, icon: &DockIcon<'_>) -> bool {
        self.path.as_deref() == icon.path && self.label == icon.label && self.size == icon.size
    }

    /// Shows the running indicator below the icon, inside the layer so it slides along.
    /// Returns whether it changed.
    fn place_dot(
        &mut self,
        device: &IDCompositionDesktopDevice,
        palette: Palette,
        item: &DockIcon<'_>,
        height: i32,
        dpi: u32,
    ) -> Result<bool> {
        let top = item.center_y - item.size / 2;
        let key = item.running.then_some((item.active, top, height, dpi));
        if key == self.dot_key {
            return Ok(false);
        }
        unsafe {
            if let Some(old) = self.dot.take() {
                self.visual.RemoveVisual(&old)?;
            }
            if key.is_some() {
                let s = dpi as f32 / 96.;
                let (w, h) = DOT_BOX;
                let width = (w * s).ceil() as i32;
                let surface = paint(device, palette, width, (h * s).ceil() as i32, dpi, |c| {
                    let mid = w / 2.;
                    let half = if item.active { 5. } else { 2. };
                    if item.active {
                        c.fill(mid - 9., 1., mid + 9., 7., 3., c.p.accent, 0.18)?;
                    }
                    let ink = if item.active { c.p.accent } else { c.p.muted };
                    c.round(mid - half, 2.5, mid + half, 5.5, 1.5, ink)
                })?;
                let dot = device.CreateVisual()?;
                dot.SetContent(&surface)?;
                dot.SetOffsetX2(((self.size - width) / 2) as f32)?;
                dot.SetOffsetY2((height as f32 - DOT_TOP * s).round() - top as f32)?;
                // Behind the icon, which may grow over it while lifted.
                self.visual.AddVisual(&dot, true, None)?;
                self.dot = Some(dot);
            }
        }
        self.dot_key = key;
        Ok(true)
    }
}

impl Renderer {
    /// Both HWNDs belong to this UI thread and outlive the renderer.
    pub fn new(top: HWND, bottom: HWND, dark: bool, settings: &Settings) -> Result<Self> {
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
                d2d,
                device,
                write: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
                imaging: CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?,
                icons: Vec::new(),
                bottom_size: None,
                top_state: None,
                metric_layer: None,
                menu: None,
                panel: None,
                about: None,
                launchpad: None,
                palette: Palette::new(dark, settings),
                top_background: settings.top_bar_background(),
                color_tray_icons: settings.enabled(Key::ColorTrayIcons),
                traditional_panel: settings.enabled(Key::Language),
                mark: None,
            })
        }
    }

    pub fn is_dark(&self) -> bool {
        self.palette.dark
    }

    /// Invalidates painted layers when their colors or text settings change.
    pub fn set_appearance(&mut self, dark: bool, settings: &Settings) -> bool {
        self.top_background = settings.top_bar_background();
        self.color_tray_icons = settings.enabled(Key::ColorTrayIcons);
        // Panel labels are repainted when the changed language choice is laid out.
        self.traditional_panel = settings.enabled(Key::Language);
        let palette = Palette::new(dark, settings);
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
    /// shows its list reversed out of a solid tile while its menu is open.
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
        frosted: bool,
        immersive_color: Option<u32>,
    ) -> Result<()> {
        let palette = self
            .palette
            .for_top_bar(self.top_background, immersive_color);
        let shadow = top_bar_shadow(self.top_background, immersive_color);
        let metrics_changed = self.update_metrics(layout, dpi, metrics, palette, shadow)?;
        let state = TopState {
            layout: layout.clone(),
            background: self.top_background,
            frosted,
            color_tray_icons: self.color_tray_icons,
            immersive_color,
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
        let (hover_alpha, open_alpha) = if palette.light_ground() {
            (0.08, 0.15)
        } else {
            (0.14, 0.24)
        };
        let backing = |c: &Canvas, slot: Rect, alpha: f32| {
            let (l, r) = (dip(slot.x) + 2., dip(slot.x + slot.width) - 2.);
            c.fill(l, 4., r, h - 4., PILL_RADIUS, c.p.line, alpha)
        };
        let surface = paint(
            &self.device,
            palette,
            layout.width,
            layout.height,
            dpi,
            |c| {
                if immersive_color.is_some() {
                    c.round(0., 0., w, h, 0., c.p.bar)?;
                } else {
                    c.top_background(Area::new(0., 0., w, h), self.top_background, frosted)?;
                }
                let (shadow_rgb, shadow_alpha) = c.top_shadow();
                let t = layout.title;
                if shadow {
                    c.draw_text(
                        &self.write,
                        title,
                        c.rect(dip(t.x), TOP_SHADOW_Y, dip(t.x + t.width), h + TOP_SHADOW_Y),
                        Text {
                            size: 13.,
                            bold: true,
                            ..Text::default()
                        },
                        shadow_rgb,
                        shadow_alpha,
                    )?;
                }
                c.text(
                    &self.write,
                    title,
                    c.rect(dip(t.x), 0., dip(t.x + t.width), h),
                    13.,
                    true,
                    c.p.ink,
                )?;
                if let Some(slot) = hover_slot {
                    backing(c, slot, hover_alpha)?;
                }
                if let Some(slot) = open_slot {
                    backing(c, slot, open_alpha)?;
                }
                let icon_size = (BAR_ICON * s).round();
                for (slot, image) in layout.tray.iter().zip(icons) {
                    let r = slot.rect;
                    let bitmap = image.icon.and_then(|icon| self.hicon_bitmap(icon).ok());
                    match bitmap {
                        Some(bitmap) => {
                            let left = (r.x as f32 + (r.width as f32 - icon_size) / 2.).round();
                            let top = (r.y as f32 + (r.height as f32 - icon_size) / 2.).round();
                            let rect = c.pixels(left, top, icon_size);
                            if self.color_tray_icons {
                                c.bitmap(&bitmap, rect)?;
                            } else {
                                // An image that cannot be read is drawn as it is.
                                let faint = pixels(&bitmap)
                                    .is_ok_and(|pixels| faint(&pixels, c.p.light_ground()));
                                c.gray_bitmap(&bitmap, rect, faint)?;
                            }
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
                    if shadow && !expanded {
                        c.symbol(
                            Symbol::List { expanded: false },
                            x,
                            h / 2. + TOP_SHADOW_Y,
                            BAR_ICON,
                            shadow_rgb,
                            shadow_alpha,
                        )?;
                    }
                    c.symbol(Symbol::List { expanded }, x, h / 2., BAR_ICON, c.p.ink, 1.)?;
                }
                for (&(_, r), mark) in layout.system.iter().zip(marks) {
                    let (l, right) = (dip(r.x), dip(r.x + r.width));
                    let (x, y) = ((l + right) / 2., h / 2.);
                    match *mark {
                        Mark::Text(text) => {
                            let style = Text {
                                size: 13.,
                                align: DWRITE_TEXT_ALIGNMENT_CENTER,
                                ..Text::default()
                            };
                            if shadow {
                                c.draw_text(
                                    &self.write,
                                    text,
                                    c.rect(l, TOP_SHADOW_Y, right, h + TOP_SHADOW_Y),
                                    style,
                                    shadow_rgb,
                                    shadow_alpha,
                                )?;
                            }
                            c.draw_text(
                                &self.write,
                                text,
                                c.rect(l, 0., right, h),
                                style,
                                c.p.ink,
                                1.,
                            )?
                        }
                        Mark::Icon { symbol, dim } => {
                            let alpha = if dim { DIM } else { 1. };
                            if shadow {
                                c.symbol(
                                    symbol,
                                    x,
                                    y + TOP_SHADOW_Y,
                                    BAR_ICON,
                                    shadow_rgb,
                                    shadow_alpha * alpha,
                                )?;
                            }
                            c.symbol(symbol, x, y, BAR_ICON, c.p.ink, alpha)?;
                        }
                        Mark::Battery(symbol) => {
                            let weight = icon::weight(BAR_ICON);
                            if shadow {
                                c.symbol_weighted(
                                    symbol,
                                    x,
                                    y + TOP_SHADOW_Y,
                                    BATTERY_ICON,
                                    weight,
                                    shadow_rgb,
                                    shadow_alpha,
                                )?;
                            }
                            c.symbol_weighted(symbol, x, y, BATTERY_ICON, weight, c.p.ink, 1.)?;
                        }
                    }
                }
                let r = layout.clock;
                if let Some(slot) = layout.close {
                    let cx = dip(slot.x) + dip(slot.width) / 2.;
                    if shadow {
                        c.symbol(
                            Symbol::Close,
                            cx,
                            h / 2. + TOP_SHADOW_Y,
                            BAR_ICON,
                            shadow_rgb,
                            shadow_alpha,
                        )?;
                    }
                    c.symbol(Symbol::Close, cx, h / 2., BAR_ICON, c.p.ink, 1.)?;
                }
                // The slot is the time's width plus a gap, so centering leaves half the
                // gap on each side.
                let clock_style = Text {
                    size: layout::CLOCK_TEXT,
                    bold: true,
                    align: DWRITE_TEXT_ALIGNMENT_CENTER,
                };
                if shadow {
                    c.draw_text(
                        &self.write,
                        time,
                        c.rect(dip(r.x), TOP_SHADOW_Y, dip(r.x + r.width), h + TOP_SHADOW_Y),
                        clock_style,
                        shadow_rgb,
                        shadow_alpha,
                    )?;
                }
                c.draw_text(
                    &self.write,
                    time,
                    c.rect(dip(r.x), 0., dip(r.x + r.width), h),
                    clock_style,
                    c.p.ink,
                    1.,
                )
            },
        )?;
        unsafe { self.top.root.SetContent(&surface)? };
        self.place_mark(layout.height, dpi, palette, shadow)?;
        unsafe { self.device.Commit()? };
        self.top_state = Some(state);
        Ok(())
    }

    /// Paints the DeskIsle mark, a ring as round as the controls beside it, onto its own
    /// layer above the bar. The layer is kept while the bar's height, DPI and palette are;
    /// a repainted layer keeps showing the core if the system menu is still open.
    fn place_mark(&mut self, height: i32, dpi: u32, palette: Palette, shadow: bool) -> Result<()> {
        let key = (height, dpi, palette, shadow);
        if self.mark.as_ref().is_some_and(|mark| mark.key == key) {
            return Ok(());
        }
        let open = self.mark.as_ref().is_some_and(|mark| mark.open);
        let s = dpi as f32 / 96.;
        // Whole-pixel offsets keep the ring as crisp as when it was painted into the bar.
        let (cx, cy) = (MARK_X * s, height as f32 / 2.);
        let left = (cx - MARK_BOX / 2. * s).floor();
        let top = (cy - MARK_BOX / 2. * s).floor();
        let (x, y) = (cx - left, cy - top);
        let edge = (MARK_BOX * s).ceil() as i32 + 1;
        let ring_surface = paint(&self.device, palette, edge, edge, dpi, |c| {
            if shadow {
                let (sr, sa) = c.top_shadow();
                c.ring_alpha(
                    x / s,
                    y / s + TOP_SHADOW_Y,
                    MARK_RADIUS,
                    MARK_STROKE,
                    sr,
                    sa,
                )?;
            }
            c.ring(x / s, y / s, MARK_RADIUS, MARK_STROKE, c.p.accent)
        })?;
        let core_surface = paint(&self.device, palette, edge, edge, dpi, |c| {
            let (x, y, r) = (x / s, y / s, MARK_CORE);
            if shadow {
                let (sr, sa) = c.top_shadow();
                c.fill(
                    x - r,
                    y - r + TOP_SHADOW_Y,
                    x + r,
                    y + r + TOP_SHADOW_Y,
                    r,
                    sr,
                    sa,
                )?;
            }
            c.round(x - r, y - r, x + r, y + r, r, c.p.accent)
        })?;
        let core_goal = if open { 1. } else { 0. };
        unsafe {
            if let Some(old) = self.mark.take() {
                self.top.root.RemoveVisual(&old.visual)?;
            }
            let visual = self.device.CreateVisual()?;
            visual.SetOffsetX2(left)?;
            visual.SetOffsetY2(top)?;
            let ring = scaled_child(&self.device, &visual, &ring_surface, (x, y), 1.)?;
            let core = scaled_child(&self.device, &visual, &core_surface, (x, y), core_goal)?;
            self.top.root.AddVisual(&visual, true, None)?;
            self.mark = Some(MarkLayer {
                visual,
                ring,
                core,
                core_size: Tween::new(core_goal),
                open,
                key,
            });
        }
        Ok(())
    }

    /// Turns the DeskIsle mark into an isle while its system menu is `open`: the ring
    /// presses in once as a core grows from its center, and the core recedes after the
    /// menu closes. Only compositor transforms change, so the bar is not repainted. A
    /// change midway continues from the core's current size.
    pub fn focus_mark(&mut self, open: bool) -> Result<()> {
        let Some(mark) = self.mark.as_mut() else {
            return Ok(());
        };
        if mark.open == open {
            return Ok(());
        }
        mark.open = open;
        let (goal, duration) = if open {
            (1., MARK_GROW)
        } else {
            (0., MARK_SHRINK)
        };
        unsafe {
            if open {
                let press = keyframes(&self.device, MARK_PRESS.into_iter())?;
                mark.ring.SetScaleX(&press)?;
                mark.ring.SetScaleY(&press)?;
            }
            if let Some(curve) = mark.core_size.retarget(&self.device, goal, duration)? {
                mark.core.SetScaleX(&curve)?;
                mark.core.SetScaleY(&curve)?;
            }
            self.device.Commit()
        }
    }

    /// A short bounce after the launchpad has saved a new entry, also visible when
    /// its window is closed. The vertical offset is independent of the hover scale.
    pub fn confirm_launchpad_drop(&self) -> Result<()> {
        let Some(icon) = self.icons.iter().find(|icon| icon.path.is_none()) else {
            return Ok(());
        };
        let top = (icon.center.1 - icon.size / 2) as f32;
        let curve = keyframes(
            &self.device,
            [(0., 0.), (0.07, 0.07), (0.18, -0.14), (0.32, 0.)]
                .into_iter()
                .map(|(time, offset)| (time, top + offset * icon.size as f32)),
        )?;
        unsafe {
            icon.visual.SetOffsetY(&curve)?;
            self.device.Commit()
        }
    }

    /// `hover` lifts the icon under the pointer. `deep` is the launchpad's slot while an
    /// app is held over it, which lifts a little further and darkens. While the Dock keeps
    /// its size, icons whose slot changed slide over, so a drag pushes its neighbors aside.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_dock(
        &mut self,
        width: i32,
        height: i32,
        dpi: u32,
        items: &[DockIcon<'_>],
        hover: Option<usize>,
        deep: Option<usize>,
        lift: f32,
        resting_scale: f32,
    ) -> Result<()> {
        let size = (width, height, dpi);
        let layout_changed = self.icons.len() != items.len()
            || self.icons.iter().zip(items).any(|(layer, icon)| {
                !layer.matches(icon) || layer.center != (icon.center_x, icon.center_y)
            });
        // A resized Dock re-centers on the screen, so the old offsets do not line up with
        // the new slots. Icons then take their places at once.
        let slide = self.bottom_size == Some(size);
        let panel_changed = !slide;
        if panel_changed {
            let s = dpi as f32 / 96.;
            let w = width as f32 / s;
            let h = height as f32 / s;
            let surface = paint(&self.device, self.palette, width, height, dpi, |c| {
                c.bar_glass(Area::new(0., 0., w, h), 18.)?;
                c.stroke(0., 0., w, h, 18., c.p.line, 0.16)
            })?;
            unsafe { self.bottom.root.SetContent(&surface)? };
            self.bottom_size = Some(size);
        }
        if layout_changed {
            unsafe { self.bottom.root.RemoveAllVisuals()? };
            let mut previous = std::mem::take(&mut self.icons);
            for item in items {
                let known = previous.iter().position(|layer| layer.matches(item));
                let mut layer = match known {
                    Some(i) => previous.swap_remove(i),
                    None => self.icon(item)?,
                };
                let x = (item.center_x - item.size / 2) as f32;
                let y = (item.center_y - item.size / 2) as f32;
                unsafe {
                    if slide && known.is_some() {
                        if let Some(curve) = layer.x.retarget(&self.device, x, SLIDE)? {
                            layer.visual.SetOffsetX(&curve)?;
                        }
                    } else {
                        layer.x = Tween::new(x);
                        layer.visual.SetOffsetX2(x)?;
                    }
                    layer.visual.SetOffsetY2(y)?;
                    self.bottom.root.AddVisual(&layer.visual, true, None)?;
                }
                layer.center = (item.center_x, item.center_y);
                self.icons.push(layer);
            }
        }
        let mut changed = panel_changed || layout_changed;
        for (index, (layer, item)) in self.icons.iter_mut().zip(items).enumerate() {
            changed |= layer.place_dot(&self.device, self.palette, item, height, dpi)?;
            let deepened = deep == Some(index);
            let goal = resting_scale
                * if deepened {
                    lift.max(1.) * DROP_LIFT
                } else if hover == Some(index) {
                    lift
                } else {
                    1.
                };
            if let Some(curve) = layer.lift.retarget(&self.device, goal, DURATION)? {
                unsafe {
                    layer.scale.SetScaleX(&curve)?;
                    layer.scale.SetScaleY(&curve)?;
                }
                changed = true;
            }
            let shade = if deepened { 1. } else { 0. };
            if let Some((fade, tween)) = &mut layer.shade
                && let Some(curve) = tween.retarget(&self.device, shade, DURATION)?
            {
                unsafe { fade.SetOpacity(&curve)? };
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
            let image = self.device.CreateVisual()?;
            image.SetContent(&surface)?;
            let scale = self.device.CreateScaleTransform()?;
            scale.SetCenterX2(size * 0.5)?;
            scale.SetCenterY2(size * 0.75)?;
            scale.SetScaleX2(1.)?;
            scale.SetScaleY2(1.)?;
            image.SetTransform(&scale)?;
            // Of the Dock icons only the launchpad accepts a dropped app.
            let shade = if item.path.is_none() {
                let tint = paint(&self.device, self.palette, item.size, item.size, 96, |c| {
                    c.fill(0., 0., size, size, size * 0.24, (0, 0, 0), DROP_SHADE)
                })?;
                let veil = self.device.CreateVisual()?;
                veil.SetContent(&tint)?;
                let fade = self.device.CreateEffectGroup()?;
                fade.SetOpacity2(0.)?;
                veil.SetEffect(&fade)?;
                image.AddVisual(&veil, false, None)?;
                Some((fade, Tween::new(0.)))
            } else {
                None
            };
            let visual = self.device.CreateVisual()?;
            visual.AddVisual(&image, true, None)?;
            Ok(IconLayer {
                path: item.path.map(Path::to_owned),
                label: item.label.into(),
                size: item.size,
                center: (0, 0),
                visual,
                scale,
                lift: Tween::new(1.),
                x: Tween::new(0.),
                shade,
                dot: None,
                dot_key: None,
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

/// The premultiplied BGRA pixels of an image from [`Renderer::hicon_bitmap`].
fn pixels(bitmap: &IWICBitmapSource) -> Result<Vec<u8>> {
    unsafe {
        let (mut width, mut height) = (0, 0);
        bitmap.GetSize(&mut width, &mut height)?;
        let stride = width * 4;
        let mut pixels = vec![0; stride as usize * height as usize];
        bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)?;
        Ok(pixels)
    }
}

/// Whether a tray image barely shows on the bar: less than a third of what it covers is
/// darker than mid gray on a light bar, or lighter than it on a dark one. Such images
/// are usually drawn for a taskbar of the other theme. `pixels` are premultiplied BGRA.
fn faint(pixels: &[u8], light_ground: bool) -> bool {
    let (mut covered, mut standing) = (0, 0);
    for pixel in pixels.chunks_exact(4) {
        let [b, g, r, a] = [pixel[0], pixel[1], pixel[2], pixel[3]].map(u32::from);
        // Premultiplied luma against half the coverage, without dividing.
        let light = 2 * (299 * r + 587 * g + 114 * b) > 1000 * a;
        covered += a;
        if light != light_ground {
            standing += a;
        }
    }
    3 * standing < covered
}

#[cfg(test)]
mod tests {
    use super::faint;

    /// Premultiplied BGRA pixels: `count` of gray `level` at coverage `alpha`.
    fn gray(level: u8, alpha: u8, count: usize) -> Vec<u8> {
        let v = (u16::from(level) * u16::from(alpha) / 255) as u8;
        [v, v, v, alpha].repeat(count)
    }

    #[test]
    fn only_icons_that_blend_into_the_bar_are_faint() {
        // A light gray glyph drawn for a dark taskbar, with transparent surroundings.
        let light = [gray(200, 255, 40), gray(0, 0, 200)].concat();
        assert!(faint(&light, true));
        assert!(!faint(&light, false));
        // A black tile with a white glyph shows on a light bar; on a dark one only the
        // glyph does, which is too little of it.
        let tile = [gray(0, 255, 75), gray(255, 255, 25)].concat();
        assert!(!faint(&tile, true));
        assert!(faint(&tile, false));
        // Antialiased edges count by their coverage.
        let silhouette = [gray(0, 255, 30), gray(0, 64, 60)].concat();
        assert!(faint(&silhouette, false));
        assert!(!faint(&gray(0, 0, 16), true));
    }
}
