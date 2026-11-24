//! Liquid Glass surfaces modelled on PrismGlass Desktop's classic material. The glass
//! is clear: it refracts the scene behind the surface through a curved bevel with RGB
//! dispersion, diffuses the body lightly, and adds Fresnel reflection and a lit rim
//! (`prism.rs`). Live surfaces show the result in a `Windows.UI.Composition` visual
//! beneath DeskIsle's DirectComposition content; captured backdrops are painted the
//! same way through Direct2D.

use std::cell::RefCell;
use std::mem::size_of;
use std::time::Duration;

use windows::Foundation::{Numerics::Plane, Size, TimeSpan};
use windows::Graphics::DirectX::{DirectXAlphaMode, DirectXPixelFormat};
use windows::System::{DispatcherQueue, DispatcherQueueController};
use windows::UI::Composition::{
    CompositionDrawingSurface, CompositionGraphicsDevice, CompositionRoundedRectangleGeometry,
    Compositor, ContainerVisual, Desktop::DesktopWindowTarget, SpriteVisual,
};
use windows::Win32::{
    Foundation::{E_INVALIDARG, HWND, POINT, RECT},
    Graphics::{
        Direct2D::{Common::*, *},
        Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
    },
    System::WinRT::{
        Composition::{
            ICompositionDrawingSurfaceInterop, ICompositorDesktopInterop, ICompositorInterop,
        },
        CreateDispatcherQueueController, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
        DispatcherQueueOptions,
    },
};
use windows::core::{Error, Interface, Result, h};

use crate::geometry::Rect;
use crate::platform::backdrop::{self, Backdrop};
use crate::ui::panel::Area;

use super::canvas::{Canvas, Rgb, color};
use super::prism::{self, Card, Scene};

thread_local! {
    /// Keeps the WinRT DispatcherQueue alive on the UI thread across renderer rebuilds.
    static DISPATCHER_QUEUE: RefCell<Option<DispatcherQueueController>> = const { RefCell::new(None) };

}

fn ensure_dispatcher_queue() -> Result<()> {
    if DispatcherQueue::GetForCurrentThread().is_ok() {
        return Ok(());
    }
    DISPATCHER_QUEUE.with_borrow_mut(|slot| {
        if slot.is_none() {
            let options = DispatcherQueueOptions {
                dwSize: size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            };
            *slot = Some(unsafe { CreateDispatcherQueueController(options)? });
        }
        Ok(())
    })
}

/// Scene beyond the glass that its bevel refracts and reflects, in pixels. The
/// longest PrismGlass ray is the Fresnel reflection at twice the 30 px maximum bevel.
const SCENE_MARGIN: f32 = 64.;
/// How far a card continues past a monitor edge it touches, so only its inner edges
/// show a bevel, as a pane tucked under the screen frame would.
const OFFSCREEN: f32 = 48.;

/// PrismGlass `rimStrength` presets, named by the PrismGlass surface each mirrors.
pub(super) mod rim {
    /// Dock and native popups (menus).
    pub const DOCK: f32 = 1.;
    /// The top island.
    pub const TOP: f32 = 0.45;
    /// Full-window panels such as the workspace.
    pub const PANEL: f32 = 0.6;
    /// Dock labels and tooltips.
    pub const LABEL: f32 = 0.32;
}

/// Physical pixel geometry and optical parameters for a live Liquid Glass surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GlassSpec {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    /// User glass strength in `0..=100`. `0` hides the glass; PrismGlass's clear
    /// material has no blur to scale, so other values look the same.
    pub strength: i32,
    /// PrismGlass `rimStrength`; see [`rim`].
    pub rim: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Background {
    Wallpaper(u64),
    BehindWindow,
}

/// Placement and source used to render the current surface.
#[derive(Clone, Copy, PartialEq)]
struct Shown {
    spec: GlassSpec,
    origin: (i32, i32),
    monitor: Rect,
    background: Background,
}

/// Shared UI-thread `Windows.UI.Composition` host bound to the renderer's Direct2D device.
pub(super) struct GlassEngine {
    compositor: Compositor,
    graphics: CompositionGraphicsDevice,
}

impl GlassEngine {
    pub fn new(d2d: &ID2D1Device) -> Result<Self> {
        ensure_dispatcher_queue()?;
        let compositor = Compositor::new()?;
        let interop: ICompositorInterop = compositor.cast()?;
        let graphics = unsafe { interop.CreateGraphicsDevice(d2d)? };
        Ok(Self {
            compositor,
            graphics,
        })
    }

    /// `hwnd` belongs to this UI thread and outlives the returned surface.
    pub fn attach(&self, hwnd: HWND) -> Result<GlassSurface> {
        let desktop: ICompositorDesktopInterop = self.compositor.cast()?;
        // Bind to `isTopmost = false` so the glass sits directly beneath DeskIsle's
        // `CreateTargetForHwnd(hwnd, true)` DirectComposition content tree.
        let target = unsafe { desktop.CreateDesktopWindowTarget(hwnd, false)? };
        let root = self.compositor.CreateContainerVisual()?;
        target.SetRoot(&root)?;
        let clip_geom = self.compositor.CreateRoundedRectangleGeometry()?;
        let clip = self
            .compositor
            .CreateGeometricClipWithGeometry(&clip_geom)?;
        let frame = self.compositor.CreateContainerVisual()?;
        frame.SetClip(&clip)?;
        frame.SetIsVisible(false)?;
        root.Children()?.InsertAtTop(&frame)?;
        let core = self.compositor.CreateSpriteVisual()?;
        frame.Children()?.InsertAtTop(&core)?;

        Ok(GlassSurface {
            hwnd,
            target,
            root,
            frame,
            clip_geom,
            core,
            surface: None,
            current: None,
        })
    }

    /// Copies premultiplied BGRA `pixels` of `width` x `height` into `surface`.
    fn upload(
        &self,
        surface: &CompositionDrawingSurface,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<()> {
        let interop: ICompositionDrawingSurfaceInterop = surface.cast()?;
        let rect = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        unsafe {
            let mut offset = POINT::default();
            let ctx: ID2D1DeviceContext = interop.BeginDraw(Some(&rect), &mut offset)?;
            let draw = (|| -> Result<()> {
                ctx.SetDpi(96., 96.);
                ctx.Clear(Some(&D2D1_COLOR_F::default()));
                let bitmap = premultiplied_bitmap(&ctx, width, height, pixels)?;
                let at = D2D_RECT_F {
                    left: offset.x as f32,
                    top: offset.y as f32,
                    right: (offset.x + width as i32) as f32,
                    bottom: (offset.y + height as i32) as f32,
                };
                ctx.DrawBitmap(
                    &bitmap,
                    Some(&at),
                    1.,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                );
                Ok(())
            })();
            let end = interop.EndDraw();
            draw.and(end)
        }
    }
}

/// `pixels` must hold `width * height` premultiplied BGRA pixels; checked here because
/// `CreateBitmap` trusts the pointer and stride.
fn premultiplied_bitmap(
    ctx: &ID2D1DeviceContext,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<ID2D1Bitmap1> {
    if width == 0
        || height == 0
        || (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            != Some(pixels.len())
    {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.,
        dpiY: 96.,
        ..Default::default()
    };
    unsafe {
        ctx.CreateBitmap(
            D2D_SIZE_U { width, height },
            Some(pixels.as_ptr().cast()),
            width * 4,
            &props,
        )
    }
}

/// Live per-HWND Liquid Glass tree. Dropping it explicitly closes the WinRT
/// composition target so the same `HWND` can immediately rebind if needed.
pub(super) struct GlassSurface {
    hwnd: HWND,
    target: DesktopWindowTarget,
    root: ContainerVisual,
    frame: ContainerVisual,
    clip_geom: CompositionRoundedRectangleGeometry,
    core: SpriteVisual,
    surface: Option<(CompositionDrawingSurface, u32, u32)>,
    current: Option<Shown>,
}

impl GlassSurface {
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn hide(&mut self) -> Result<()> {
        self.frame.SetIsVisible(false)?;
        self.current = None;
        Ok(())
    }

    pub fn invalidate_scene(&mut self) {
        self.current = None;
    }

    pub fn update(&mut self, engine: &GlassEngine, spec: GlassSpec) -> Result<()> {
        self.show(
            engine,
            spec,
            Background::Wallpaper(backdrop::wallpaper_stamp()),
        )
    }

    /// Refract the composite behind this HWND at its final screen position. The flat
    /// center stays transparent so scrolling and video remain live between samples.
    pub fn update_behind(&mut self, engine: &GlassEngine, spec: GlassSpec) -> Result<()> {
        self.show(engine, spec, Background::BehindWindow)
    }

    fn show(
        &mut self,
        engine: &GlassEngine,
        spec: GlassSpec,
        background: Background,
    ) -> Result<()> {
        if spec.strength <= 0 || spec.width < 1. || spec.height < 1. {
            return self.hide();
        }
        let (ox, oy, monitor) = backdrop::placement(self.hwnd)?;
        let shown = Shown {
            spec,
            origin: (ox, oy),
            monitor,
            background,
        };
        if self.current == Some(shown) {
            return Ok(());
        }
        let radius = spec
            .radius
            .min(spec.width * 0.5)
            .min(spec.height * 0.5)
            .max(0.);
        let width = spec.width.round().max(1.) as u32;
        let height = spec.height.round().max(1.) as u32;

        // Include pixels outside the rectangle for rays bent outwards at the bevel.
        let gx = ox as f32 + spec.x;
        let gy = oy as f32 + spec.y;
        let (gr, gb) = (gx + width as f32, gy + height as f32);
        let (ml, mt) = (monitor.x as f32, monitor.y as f32);
        let (mr, mb) = (ml + monitor.width as f32, mt + monitor.height as f32);
        let scene_l = (gx - SCENE_MARGIN).max(ml).floor();
        let scene_t = (gy - SCENE_MARGIN).max(mt).floor();
        let scene_r = (gr + SCENE_MARGIN).min(mr).ceil();
        let scene_b = (gb + SCENE_MARGIN).min(mb).ceil();
        let pixels = if scene_r > scene_l && scene_b > scene_t {
            let rect = Rect {
                x: scene_l as i32,
                y: scene_t as i32,
                width: (scene_r - scene_l) as i32,
                height: (scene_b - scene_t) as i32,
            };
            let image = match background {
                Background::Wallpaper(_) => Backdrop::wallpaper(rect)?,
                Background::BehindWindow => Backdrop::behind(self.hwnd, rect, 0, false)?,
            };
            let scene = Scene {
                width: image.width,
                height: image.height,
                pixels: &image.pixels,
                scale_x: image.width as f32 / rect.width as f32,
                scale_y: image.height as f32 / rect.height as f32,
                offset_x: gx - scene_l,
                offset_y: gy - scene_t,
            };
            let touches = |a: f32, b: f32| (a - b).abs() <= 1.;
            let l = if touches(gx, ml) { -OFFSCREEN } else { 0. };
            let t = if touches(gy, mt) { -OFFSCREEN } else { 0. };
            let r = width as f32 + if touches(gr, mr) { OFFSCREEN } else { 0. };
            let b = height as f32 + if touches(gb, mb) { OFFSCREEN } else { 0. };
            let mut card = Card::filling(l, t, r - l, b - t, radius, spec.rim);
            card.bevel_half_h = height as f32 * 0.5;
            match background {
                Background::Wallpaper(_) => prism::render(&scene, width, height, card),
                Background::BehindWindow => prism::render_bevel(&scene, width, height, card),
            }
        } else {
            // Off every monitor: nothing to refract and nothing visible.
            vec![0; width as usize * height as usize * 4]
        };

        let surface = match &self.surface {
            Some((surface, w, h)) if (*w, *h) == (width, height) => surface.clone(),
            _ => {
                let surface = engine.graphics.CreateDrawingSurface(
                    Size {
                        Width: width as f32,
                        Height: height as f32,
                    },
                    DirectXPixelFormat::B8G8R8A8UIntNormalized,
                    DirectXAlphaMode::Premultiplied,
                )?;
                let brush = engine.compositor.CreateSurfaceBrushWithSurface(&surface)?;
                self.core.SetBrush(&brush)?;
                self.surface = Some((surface.clone(), width, height));
                surface
            }
        };
        engine.upload(&surface, width, height, &pixels)?;

        let mut v2 = self.clip_geom.Offset().unwrap_or_default();
        v2.X = spec.x;
        v2.Y = spec.y;
        self.clip_geom.SetOffset(v2)?;
        v2.X = width as f32;
        v2.Y = height as f32;
        self.clip_geom.SetSize(v2)?;
        let size = v2;
        v2.X = radius;
        v2.Y = radius;
        self.clip_geom.SetCornerRadius(v2)?;
        let mut plane = Plane::default();
        plane.Normal.X = spec.x;
        plane.Normal.Y = spec.y;
        self.core.SetOffset(plane.Normal)?;
        self.core.SetSize(size)?;

        self.frame.SetIsVisible(true)?;
        self.current = Some(shown);
        Ok(())
    }

    /// Synchronizes the backdrop's pop-in fade and zoom with `MenuView`'s DirectComposition
    /// opening animation so the glass rim and refracted body stay locked together.
    pub fn animate_open(
        &self,
        engine: &GlassEngine,
        width: i32,
        height: i32,
        secs: f32,
    ) -> Result<()> {
        let duration = TimeSpan::from(Duration::from_secs_f32(secs));
        let mut c1 = self.clip_geom.Offset().unwrap_or_default();
        c1.X = 0.;
        c1.Y = 0.;
        let mut c2 = c1;
        c2.X = 0.;
        c2.Y = 1.;
        let ease = engine.compositor.CreateCubicBezierEasingFunction(c1, c2)?;
        let mut p = Plane::default();
        p.Normal.X = width as f32 * 0.5;
        p.Normal.Y = height as f32 * 0.5;
        p.Normal.Z = 0.;
        self.root.SetCenterPoint(p.Normal)?;

        let fade = engine.compositor.CreateScalarKeyFrameAnimation()?;
        fade.InsertKeyFrame(0., 0.)?;
        fade.InsertKeyFrameWithEasingFunction(1., 1., &ease)?;
        fade.SetDuration(duration)?;
        self.root.StartAnimation(h!("Opacity"), &fade)?;

        let zoom = engine.compositor.CreateVector3KeyFrameAnimation()?;
        p.Normal.X = 0.96;
        p.Normal.Y = 0.96;
        p.Normal.Z = 1.;
        zoom.InsertKeyFrame(0., p.Normal)?;
        p.Normal.X = 1.;
        p.Normal.Y = 1.;
        p.Normal.Z = 1.;
        zoom.InsertKeyFrameWithEasingFunction(1., p.Normal, &ease)?;
        zoom.SetDuration(duration)?;
        self.root.StartAnimation(h!("Scale"), &zoom)?;
        Ok(())
    }
}

impl Drop for GlassSurface {
    fn drop(&mut self) {
        let _ = self.target.SetRoot(None);
        let _ = self.frame.Close();
        let _ = self.root.Close();
        let _ = self.target.Close();
    }
}

/// Visual recipe for the Direct2D specular shell painted above a Liquid Glass backdrop.
#[derive(Clone, Copy, Debug)]
pub(super) struct LiquidShell {
    pub top_tint: Rgb,
    pub bottom_tint: Rgb,
    /// Tint wash opacity (`0..=1`). Low values (`0.10..0.24`) reveal crystal glass when
    /// `glass_active` is true; `1.0` paints an opaque fallback when transparency is off.
    pub opacity: f32,
    /// Intensity of the Fresnel specular rim, inner meniscus glow and chromatic sub-rim.
    pub rim: f32,
    /// Whether a live or captured optical glass backdrop lies beneath this shell.
    pub glass_active: bool,
}

impl Canvas {
    /// Paints a captured `Backdrop` of `area` through the PrismGlass material, with
    /// `rim` as its `rimStrength`. Refraction beyond the capture repeats its edge pixels.
    pub(super) fn liquid_backdrop(
        &self,
        image: &Backdrop,
        area: Area,
        radius: f32,
        rim: f32,
    ) -> Result<()> {
        self.paint_prism(image, area, radius, rim, prism::render)
    }

    pub(super) fn liquid_bevel(
        &self,
        image: &Backdrop,
        area: Area,
        radius: f32,
        rim: f32,
    ) -> Result<()> {
        self.paint_prism(image, area, radius, rim, prism::render_bevel)
    }

    fn paint_prism(
        &self,
        image: &Backdrop,
        area: Area,
        radius: f32,
        rim: f32,
        render: fn(&Scene, u32, u32, Card) -> Vec<u8>,
    ) -> Result<()> {
        let dest = self.rect(area.left, area.top, area.right, area.bottom);
        let left = dest.left.round();
        let top = dest.top.round();
        let width = (dest.right - left).round().max(1.) as u32;
        let height = (dest.bottom - top).round().max(1.) as u32;
        let scene = Scene {
            width: image.width,
            height: image.height,
            pixels: &image.pixels,
            scale_x: image.width as f32 / width as f32,
            scale_y: image.height as f32 / height as f32,
            offset_x: 0.,
            offset_y: 0.,
        };
        let card = Card::filling(
            0.,
            0.,
            width as f32,
            height as f32,
            radius * self.scale,
            rim,
        );
        let pixels = render(&scene, width, height, card);
        let bitmap = premultiplied_bitmap(&self.ctx, width, height, &pixels)?;
        let at = D2D_RECT_F {
            left,
            top,
            right: left + width as f32,
            bottom: top + height as f32,
        };
        unsafe {
            self.ctx.DrawBitmap(
                &bitmap,
                Some(&at),
                1.,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                None,
                None,
            );
        }
        Ok(())
    }

    /// Draws the opaque fallback shell used without glass: a tint gradient with an inner
    /// glow, a faint prismatic sub-rim and a directional specular rim. With glass active
    /// it draws nothing.
    pub(super) fn liquid_glass_shell(
        &self,
        area: Area,
        radius: f32,
        spec: LiquidShell,
    ) -> Result<()> {
        let (l, t, r, b) = (area.left, area.top, area.right, area.bottom);
        // PrismGlass glass is clear and lights its own rim; a wash or second rim over it
        // would gray the refracted scene.
        if r <= l || b <= t || spec.glass_active {
            return Ok(());
        }
        let radius = radius.min((r - l) * 0.5).min((b - t) * 0.5).max(0.);
        let opacity = spec.opacity.clamp(0., 1.);
        let rim = spec.rim.clamp(0., 1.8);

        if opacity > 0. {
            self.gradient(
                l,
                t,
                r,
                b,
                radius,
                (spec.top_tint, opacity),
                (spec.bottom_tint, opacity),
            )?;
        }

        if rim <= 0.001 {
            return Ok(());
        }

        // Inner curved meniscus volume luminescence + prismatic sub-rim.
        self.clipped_round(l, t, r, b, radius, || {
            let glow_scale = if self.p.dark { 0.82 } else { 1.0 } * rim;
            for (inset, width, top_a, mid_a, bot_a) in
                [(0.9, 1.2, 0.20, 0.03, 0.12), (2.2, 1.8, 0.08, 0.01, 0.05)]
            {
                if r - l > 2. * inset && b - t > 2. * inset {
                    self.gradient_stroke(
                        Area::new(l + inset, t + inset, r - inset, b - inset),
                        (radius - inset).max(0.),
                        width,
                        (l, t),
                        (l, b),
                        &[
                            (0.00, ((255, 255, 255), (top_a * glow_scale).min(1.))),
                            (0.45, ((235, 246, 255), (mid_a * glow_scale).min(1.))),
                            (1.00, ((216, 238, 255), (bot_a * glow_scale).min(1.))),
                        ],
                    )?;
                }
            }

            // Subtle chromatic dispersion sub-rim (cyan at top-left, soft violet at bottom-right).
            let prism_inset = 1.15;
            if r - l > 2. * prism_inset && b - t > 2. * prism_inset {
                let p_alpha = (if self.p.dark { 0.20 } else { 0.16 } * rim).min(1.);
                self.gradient_stroke(
                    Area::new(
                        l + prism_inset,
                        t + prism_inset,
                        r - prism_inset,
                        b - prism_inset,
                    ),
                    (radius - prism_inset).max(0.),
                    0.95,
                    (l, t),
                    (r, b),
                    &[
                        (0.00, ((148, 228, 255), p_alpha)),
                        (0.50, ((255, 255, 255), p_alpha * 0.25)),
                        (1.00, ((198, 174, 255), p_alpha * 0.90)),
                    ],
                )?;
            }
            Ok(())
        })?;

        // Razor-sharp directional Fresnel specular outer rim.
        let half = 0.5 / self.scale;
        let (peak, shoulder, waist, caustic, tail) = if self.p.dark {
            (0.76, 0.44, 0.20, 0.40, 0.64)
        } else {
            (0.92, 0.58, 0.28, 0.52, 0.80)
        };
        self.gradient_stroke(
            Area::new(l + half, t + half, r - half, b - half),
            (radius - half).max(0.),
            1.1 / self.scale.max(1.) + 0.35,
            (l, t),
            (r, b),
            &[
                (0.00, ((255, 255, 255), (peak * rim).min(1.))),
                (0.26, ((248, 252, 255), (shoulder * rim).min(1.))),
                (0.52, ((224, 238, 255), (waist * rim).min(1.))),
                (0.78, ((244, 250, 255), (caustic * rim).min(1.))),
                (1.00, ((255, 255, 255), (tail * rim).min(1.))),
            ],
        )
    }

    /// A compact translucent Liquid Glass pill or card for inner controls (hover pills,
    /// quick-toggle tiles, search fields and media cards).
    pub(super) fn liquid_pill(
        &self,
        area: Area,
        radius: f32,
        tint: Rgb,
        fill_alpha: f32,
        rim_alpha: f32,
    ) -> Result<()> {
        let (l, t, r, b) = (area.left, area.top, area.right, area.bottom);
        if r <= l || b <= t {
            return Ok(());
        }
        let radius = radius.min((r - l) * 0.5).min((b - t) * 0.5).max(0.);
        let top_sheen = mix_rgb(tint, (255, 255, 255), if self.p.dark { 0.28 } else { 0.55 });
        self.gradient_stops(
            l,
            t,
            r,
            b,
            radius,
            &[
                (0.00, (top_sheen, (fill_alpha * 1.35).min(1.))),
                (0.55, (tint, fill_alpha.clamp(0., 1.))),
                (1.00, (top_sheen, (fill_alpha * 1.12).min(1.))),
            ],
        )?;
        if rim_alpha > 0.001 {
            let half = 0.5 / self.scale;
            self.gradient_stroke(
                Area::new(l + half, t + half, r - half, b - half),
                (radius - half).max(0.),
                1.0 / self.scale.max(1.) + 0.2,
                (l, t),
                (r, b),
                &[
                    (0.00, ((255, 255, 255), (rim_alpha * 1.15).min(1.))),
                    (0.50, ((225, 240, 255), (rim_alpha * 0.42).min(1.))),
                    (1.00, ((255, 255, 255), (rim_alpha * 0.88).min(1.))),
                ],
            )?;
        }
        Ok(())
    }

    /// A circular Liquid Glass droplet badge (used for Control Center tiles, media play
    /// button and menu choice icons): translucent crystal when idle, radiant glowing
    /// accent sphere when `on`.
    pub(super) fn liquid_orb(
        &self,
        cx: f32,
        cy: f32,
        diameter: f32,
        accent: Rgb,
        on: bool,
    ) -> Result<()> {
        let r = diameter * 0.5;
        let area = Area::new(cx - r, cy - r, cx + r, cy + r);
        if on {
            // Soft outer luminescence halo, like the active Wi-Fi orb in the README.
            for (spread, a) in [(3.2, 0.08), (1.8, 0.14)] {
                self.fill(
                    area.left - spread,
                    area.top - spread,
                    area.right + spread,
                    area.bottom + spread,
                    r + spread,
                    accent,
                    a,
                )?;
            }
            let bright = mix_rgb(accent, (220, 245, 255), 0.42);
            let deep = mix_rgb(accent, (12, 24, 48), 0.16);
            self.gradient_stops(
                area.left,
                area.top,
                area.right,
                area.bottom,
                r,
                &[
                    (0.00, (bright, 0.92)),
                    (0.52, (accent, 0.82)),
                    (1.00, (deep, 0.88)),
                ],
            )?;
            let half = 0.5 / self.scale;
            self.gradient_stroke(
                Area::new(
                    area.left + half,
                    area.top + half,
                    area.right - half,
                    area.bottom - half,
                ),
                (r - half).max(0.),
                1.1,
                (area.left, area.top),
                (area.right, area.bottom),
                &[
                    (0.00, ((255, 255, 255), 0.88)),
                    (0.50, (bright, 0.45)),
                    (1.00, ((255, 255, 255), 0.72)),
                ],
            )
        } else {
            let base = if self.p.dark {
                (215, 228, 248)
            } else {
                (255, 255, 255)
            };
            let fill = if self.p.dark { 0.11 } else { 0.36 };
            let rim = if self.p.dark { 0.34 } else { 0.52 };
            self.liquid_pill(area, r, base, fill, rim)
        }
    }

    /// Strokes a rounded rectangle in DIPs with a linear gradient brush between `from` and `to`.
    pub(super) fn gradient_stroke(
        &self,
        area: Area,
        radius: f32,
        width_dip: f32,
        from: (f32, f32),
        to: (f32, f32),
        stops: &[(f32, (Rgb, f32))],
    ) -> Result<()> {
        let shape = self.shape(area.left, area.top, area.right, area.bottom, radius);
        let stops: Vec<_> = stops
            .iter()
            .map(|&(position, ((r, g, b), a))| D2D1_GRADIENT_STOP {
                position,
                color: color(r, g, b, a),
            })
            .collect();
        unsafe {
            let target: &ID2D1RenderTarget = &self.ctx;
            let collection = target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            let mut line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES::default();
            line.startPoint.X = self.origin.x as f32 + from.0 * self.scale;
            line.startPoint.Y = self.origin.y as f32 + from.1 * self.scale;
            line.endPoint.X = self.origin.x as f32 + to.0 * self.scale;
            line.endPoint.Y = self.origin.y as f32 + to.1 * self.scale;
            let brush = self
                .ctx
                .CreateLinearGradientBrush(&line, None, &collection)?;
            self.ctx
                .DrawRoundedRectangle(&shape, &brush, width_dip * self.scale, None);
        }
        Ok(())
    }
}

fn mix_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let ch = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::canvas::{Palette, paint_pixels};
    use crate::settings::Settings;
    use windows::Win32::Graphics::{Direct3D::D3D_DRIVER_TYPE_WARP, Dxgi::IDXGIDevice};
    use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_EX_NOACTIVATE,
        WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::w;

    struct WinRtGuard;

    impl WinRtGuard {
        fn init() -> Result<Self> {
            unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
            Ok(Self)
        }
    }

    impl Drop for WinRtGuard {
        fn drop(&mut self) {
            unsafe { RoUninitialize() };
        }
    }

    struct TestWindow(HWND);

    impl TestWindow {
        fn new(style: WINDOW_EX_STYLE, x: i32, y: i32, w: i32, h: i32) -> Result<Self> {
            unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | style,
                    w!("STATIC"),
                    None,
                    WS_POPUP,
                    x,
                    y,
                    w,
                    h,
                    None,
                    None,
                    None,
                    None,
                )
                .map(Self)
            }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn liquid_backdrop_clips_to_rounded_bounds_and_lights_its_rim() -> Result<()> {
        let dxgi: IDXGIDevice = super::super::d3d_device(D3D_DRIVER_TYPE_WARP)?.cast()?;
        let device = unsafe { D2D1CreateDevice(&dxgi, None)? };
        // Colorful 4x2 BGR0 backdrop.
        let backdrop = Backdrop {
            width: 4,
            height: 2,
            pixels: [
                255, 80, 20, 0, 220, 120, 40, 0, 40, 140, 250, 0, 20, 80, 255, 0, //
                200, 60, 30, 0, 180, 100, 60, 0, 60, 180, 220, 0, 30, 120, 240, 0,
            ]
            .into(),
        };
        let palette = Palette::new(true, &Settings::default());
        let pixels = paint_pixels(&device, palette, 96, 48, 96, |c| {
            let area = Area::new(4., 4., 92., 44.);
            c.liquid_backdrop(&backdrop, area, 20., rim::DOCK)?;
            c.liquid_glass_shell(
                area,
                20.,
                LiquidShell {
                    top_tint: (36, 42, 58),
                    bottom_tint: (18, 22, 32),
                    opacity: 0.18,
                    rim: 1.0,
                    glass_active: true,
                },
            )
        })?;
        let at = |x: usize, y: usize| &pixels[(y * 96 + x) * 4..(y * 96 + x + 1) * 4];
        // Outside the rounded capsule corners stays completely transparent.
        assert_eq!(at(0, 24), [0, 0, 0, 0]);
        assert_eq!(at(5, 5), [0, 0, 0, 0]);
        // Left vs right interior preserves distinct vibrant hues from the backdrop.
        assert!(
            at(26, 24)[0] > at(26, 24)[2],
            "left={:?} right={:?}",
            at(26, 24),
            at(70, 24)
        );
        assert!(
            at(70, 24)[2] > at(70, 24)[0],
            "left={:?} right={:?}",
            at(26, 24),
            at(70, 24)
        );
        // The lit top rim is brighter than the interior just beneath it.
        let rim_luma: u32 = at(48, 5)[..3].iter().map(|&v| u32::from(v)).sum();
        let body_luma: u32 = at(48, 14)[..3].iter().map(|&v| u32::from(v)).sum();
        assert!(rim_luma > body_luma);
        Ok(())
    }

    #[test]
    fn live_glass_engine_coexists_with_dcomp_and_rebinds_cleanly() -> Result<()> {
        let _winrt = WinRtGuard::init()?;
        let hwnd = TestWindow::new(WS_EX_NOREDIRECTIONBITMAP, 0, 0, 320, 48)?;
        let dxgi: IDXGIDevice = super::super::d3d_device(D3D_DRIVER_TYPE_WARP)?.cast()?;
        let d2d = unsafe { D2D1CreateDevice(&dxgi, None)? };
        let dcomp: windows::Win32::Graphics::DirectComposition::IDCompositionDesktopDevice = unsafe {
            windows::Win32::Graphics::DirectComposition::DCompositionCreateDevice2(&d2d)?
        };
        let _dcomp_target = unsafe { dcomp.CreateTargetForHwnd(hwnd.0, true)? };

        let engine = GlassEngine::new(&d2d)?;
        {
            let mut surface = engine.attach(hwnd.0)?;
            surface.update(
                &engine,
                GlassSpec {
                    x: 0.,
                    y: 0.,
                    width: 320.,
                    height: 48.,
                    radius: 24.,
                    strength: 65,
                    rim: rim::DOCK,
                },
            )?;
            surface.animate_open(&engine, 320, 48, 0.12)?;
            surface.hide()?;
        }
        // Dropping the surface closes its target so re-attaching to the same HWND succeeds.
        let mut rebound = engine.attach(hwnd.0)?;
        rebound.update(
            &engine,
            GlassSpec {
                x: 8.,
                y: 8.,
                width: 240.,
                height: 180.,
                radius: 16.,
                strength: 50,
                rim: rim::PANEL,
            },
        )
    }

    #[test]
    #[ignore = "shows two small non-activating windows on an interactive Windows desktop"]
    fn glass_tracks_the_window_behind_its_visible_surface() -> Result<()> {
        use windows::Win32::Graphics::{
            Dwm::DwmFlush,
            Gdi::{
                BLACK_BRUSH, FillRect, GdiFlush, GetDC, GetPixel, GetStockObject, HBRUSH,
                ReleaseDC, UpdateWindow, WHITE_BRUSH,
            },
        };
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow,
            WS_EX_TOPMOST,
        };
        unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let _winrt = WinRtGuard::init()?;
        let underlay = TestWindow::new(WS_EX_TOPMOST, 160, 160, 208, 144)?;
        let overlay = TestWindow::new(
            WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
            180,
            180,
            168,
            104,
        )?;
        let dxgi: IDXGIDevice = super::super::d3d_device(D3D_DRIVER_TYPE_WARP)?.cast()?;
        let d2d = unsafe { D2D1CreateDevice(&dxgi, None)? };
        let engine = GlassEngine::new(&d2d)?;
        let mut surface = engine.attach(overlay.0)?;
        let spec = GlassSpec {
            x: 20.,
            y: 20.,
            width: 128.,
            height: 64.,
            radius: 16.,
            strength: 65,
            rim: rim::DOCK,
        };
        let area = RECT {
            left: 0,
            top: 0,
            right: 208,
            bottom: 144,
        };
        let paint = |brush, area: RECT| -> Result<()> {
            unsafe {
                let dc = GetDC(Some(underlay.0));
                let filled = FillRect(dc, &area, HBRUSH(GetStockObject(brush).0));
                ReleaseDC(Some(underlay.0), dc);
                assert_ne!(filled, 0);
                GdiFlush().ok()?;
                DwmFlush()
            }
        };
        let sample = |x, y| unsafe {
            let dc = GetDC(None);
            let color = GetPixel(dc, x, y);
            ReleaseDC(None, dc);
            assert_ne!(color.0, u32::MAX);
            color.0.to_le_bytes()[0]
        };
        let bevel = || (201..212).map(|x| sample(x, 232)).max().unwrap();
        let wait = |reason: &str, condition: &dyn Fn() -> bool| -> Result<()> {
            let start = std::time::Instant::now();
            loop {
                let mut msg = MSG::default();
                while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                    unsafe { DispatchMessageW(&msg) };
                }
                unsafe { DwmFlush()? };
                if condition() {
                    return Ok(());
                }
                assert!(
                    start.elapsed() < Duration::from_secs(3),
                    "{reason}, underlay={}, stripe={}, rim={}, center={}, bevel={}",
                    sample(170, 230),
                    sample(195, 232),
                    sample(264, 200),
                    sample(264, 232),
                    bevel()
                );
            }
        };
        unsafe {
            let _ = ShowWindow(underlay.0, SW_SHOWNOACTIVATE);
            let _ = UpdateWindow(underlay.0);
        }
        paint(BLACK_BRUSH, area)?;
        wait("underlay should be black", &|| sample(170, 230) < 10)?;
        surface.update_behind(&engine, spec)?;
        unsafe {
            let _ = ShowWindow(overlay.0, SW_SHOWNOACTIVATE);
        }
        wait("glass should light the upper rim on black", &|| {
            sample(264, 200) > 12 && sample(264, 232) < 20
        })?;

        paint(WHITE_BRUSH, area)?;
        wait(
            "the clear center should reveal the new white background",
            &|| sample(170, 230) > 235 && sample(264, 232) > 215,
        )?;
        // Cover the surface and its sampling margin with a black visual, then draw
        // a white stripe on the underlay just outside the glass's left edge.
        let marker = engine.compositor.CreateSpriteVisual()?;
        let black = engine
            .compositor
            .CreateColorBrushWithColor(windows::UI::Color {
                A: 255,
                R: 0,
                G: 0,
                B: 0,
            })?;
        marker.SetBrush(&black)?;
        let mut size = marker.Size()?;
        size.X = 168.;
        size.Y = 104.;
        marker.SetSize(size)?;
        surface.root.Children()?.InsertAtTop(&marker)?;
        wait("foreground marker should cover the surface", &|| {
            sample(264, 232) < 10
        })?;
        paint(BLACK_BRUSH, area)?;
        paint(
            WHITE_BRUSH,
            RECT {
                left: 24,
                right: 39,
                ..area
            },
        )?;
        surface.invalidate_scene();
        surface.update_behind(&engine, spec)?;
        surface.root.Children()?.Remove(&marker)?;
        wait(
            "the bevel should refract the stripe through the previously covered area",
            &|| bevel() > 100 && sample(264, 232) < 20,
        )?;
        let refreshed = bevel();
        unsafe {
            let _ = ShowWindow(overlay.0, SW_HIDE);
        }
        wait(
            "hiding the overlay should reveal the unbroken black interior",
            &|| bevel() < 10,
        )?;
        surface.invalidate_scene();
        surface.update_behind(&engine, spec)?;
        unsafe {
            let _ = ShowWindow(overlay.0, SW_SHOWNOACTIVATE);
        }
        wait(
            "visible refresh and capture-before-show should produce the same bevel",
            &|| bevel().abs_diff(refreshed) < 6,
        )
    }
}
