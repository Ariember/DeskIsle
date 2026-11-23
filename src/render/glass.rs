//! Optical Liquid Glass engine: live DWM host-backdrop refraction, diffusion and
//! vibrancy via `Windows.UI.Composition` (`isTopmost = false`, underneath DeskIsle's
//! DirectComposition content tree), Direct2D per-pixel SDF displacement refraction for
//! redirected windows, and 3D Fresnel specular rims with meniscus volume highlights.

use std::cell::RefCell;
use std::mem::size_of;
use std::time::Duration;

use windows::Foundation::{Numerics::Plane, PropertyValue, Size, TimeSpan};
use windows::Graphics::{
    DirectX::{DirectXAlphaMode, DirectXPixelFormat},
    Effects::{
        IGraphicsEffect, IGraphicsEffect_Impl, IGraphicsEffectSource, IGraphicsEffectSource_Impl,
    },
};
use windows::System::{DispatcherQueue, DispatcherQueueController};
use windows::UI::Composition::{
    CompositionDrawingSurface, CompositionEffectBrush, CompositionEffectSourceParameter,
    CompositionGraphicsDevice, CompositionRoundedRectangleGeometry, Compositor, ContainerVisual,
    Desktop::DesktopWindowTarget, SpriteVisual,
};
use windows::Win32::{
    Foundation::{E_INVALIDARG, E_NOTIMPL, HWND, POINT, RECT},
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
        Graphics::Direct2D::{
            GRAPHICS_EFFECT_PROPERTY_MAPPING, IGraphicsEffectD2D1Interop,
            IGraphicsEffectD2D1Interop_Impl,
        },
    },
};
use windows::core::{Error, GUID, HSTRING, Interface, PCWSTR, Result, h, implement};

use crate::platform::backdrop::Backdrop;
use crate::ui::panel::Area;

use super::canvas::{Canvas, Rgb, color};

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

/// A lightweight D2D1 effect node implementing WinRT's `IGraphicsEffectD2D1Interop` so
/// `Compositor::CreateEffectFactory` can compile effect graphs directly inside DWM.
#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct D2DEffect {
    id: GUID,
    sources: Vec<IGraphicsEffectSource>,
    properties: Vec<windows::Foundation::IPropertyValue>,
}

impl D2DEffect {
    fn into_effect(self) -> IGraphicsEffect {
        self.into()
    }

    fn into_source(self) -> IGraphicsEffectSource {
        let effect: IGraphicsEffect = self.into();
        effect
            .cast()
            .expect("IGraphicsEffect requires IGraphicsEffectSource")
    }
}

impl IGraphicsEffectSource_Impl for D2DEffect_Impl {}

impl IGraphicsEffect_Impl for D2DEffect_Impl {
    fn Name(&self) -> Result<HSTRING> {
        Ok(HSTRING::new())
    }

    fn SetName(&self, _name: &HSTRING) -> Result<()> {
        Ok(())
    }
}

impl IGraphicsEffectD2D1Interop_Impl for D2DEffect_Impl {
    fn GetEffectId(&self) -> Result<GUID> {
        Ok(self.id)
    }

    fn GetNamedPropertyMapping(
        &self,
        _name: &PCWSTR,
        _index: *mut u32,
        _mapping: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING,
    ) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn GetPropertyCount(&self) -> Result<u32> {
        Ok(self.properties.len() as u32)
    }

    fn GetProperty(&self, index: u32) -> Result<windows::Foundation::IPropertyValue> {
        self.properties
            .get(index as usize)
            .cloned()
            .ok_or_else(|| Error::from_hresult(E_INVALIDARG))
    }

    fn GetSource(&self, index: u32) -> Result<IGraphicsEffectSource> {
        self.sources
            .get(index as usize)
            .cloned()
            .ok_or_else(|| Error::from_hresult(E_INVALIDARG))
    }

    fn GetSourceCount(&self) -> Result<u32> {
        Ok(self.sources.len() as u32)
    }
}

fn prop_f32(value: f32) -> Result<windows::Foundation::IPropertyValue> {
    PropertyValue::CreateSingle(value)?.cast()
}

fn prop_u32(value: u32) -> Result<windows::Foundation::IPropertyValue> {
    PropertyValue::CreateUInt32(value)?.cast()
}

fn prop_bool(value: bool) -> Result<windows::Foundation::IPropertyValue> {
    PropertyValue::CreateBoolean(value)?.cast()
}

fn prop_f32_array(values: &[f32]) -> Result<windows::Foundation::IPropertyValue> {
    PropertyValue::CreateSingleArray(values)?.cast()
}

fn gaussian_blur(source: IGraphicsEffectSource, sigma: f32) -> Result<D2DEffect> {
    Ok(D2DEffect {
        id: CLSID_D2D1GaussianBlur,
        sources: vec![source],
        properties: vec![
            prop_f32(sigma.clamp(0., 72.))?,
            // D2D1_GAUSSIANBLUR_OPTIMIZATION_BALANCED = 1
            prop_u32(1)?,
            // D2D1_BORDER_MODE_HARD = 1: prevents dark edge halos at monitor borders.
            prop_u32(1)?,
        ],
    })
}

fn saturation_effect(source: IGraphicsEffectSource, saturation: f32) -> Result<D2DEffect> {
    Ok(D2DEffect {
        id: CLSID_D2D1Saturation,
        sources: vec![source],
        properties: vec![prop_f32(saturation.clamp(0., 2.))?],
    })
}

fn color_matrix_effect(source: IGraphicsEffectSource, matrix: &[f32; 20]) -> Result<D2DEffect> {
    Ok(D2DEffect {
        id: CLSID_D2D1ColorMatrix,
        sources: vec![source],
        properties: vec![
            prop_f32_array(matrix)?,
            // D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED = 1
            prop_u32(1)?,
            prop_bool(true)?,
        ],
    })
}

/// Premultiplied 5x4 color matrix for the crystal core: preserves rich color saturation
/// while gently compressing blinding white or pitch-black backgrounds into a comfortable
/// luminous glass range so foreground labels and icons stay legible on any window.
fn core_glass_matrix(dark: bool, rim_boost: bool) -> [f32; 20] {
    if dark {
        let (k, lift_r, lift_g, lift_b) = if rim_boost {
            (0.76, 0.065, 0.082, 0.112)
        } else {
            (0.64, 0.042, 0.050, 0.068)
        };
        [
            k,
            0.,
            0.,
            0., //
            0.,
            k,
            0.,
            0., //
            0.,
            0.,
            k * 1.03,
            0., //
            0.,
            0.,
            0.,
            1., //
            lift_r,
            lift_g,
            lift_b,
            0.,
        ]
    } else {
        let (k, lift_r, lift_g, lift_b) = if rim_boost {
            (0.78, 0.195, 0.210, 0.232)
        } else {
            (0.68, 0.235, 0.245, 0.262)
        };
        [
            k,
            0.,
            0.,
            0., //
            0.,
            k,
            0.,
            0., //
            0.,
            0.,
            k * 1.02,
            0., //
            0.,
            0.,
            0.,
            1., //
            lift_r,
            lift_g,
            lift_b,
            0.,
        ]
    }
}

/// Physical pixel geometry and optical parameters for a live Liquid Glass surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GlassSpec {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    /// User blur/glass strength in `0..=100`. `0` hides the backdrop layer.
    pub strength: i32,
    pub dark: bool,
    pub dpi: u32,
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

    pub fn attach(&self, hwnd: HWND) -> Result<GlassSurface> {
        let desktop: ICompositorDesktopInterop = self.compositor.cast()?;
        // Bind to `isTopmost = false` so the live refracted backdrop sits directly beneath
        // DeskIsle's `CreateTargetForHwnd(hwnd, true)` DirectComposition content tree.
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
        let meniscus = self.compositor.CreateSpriteVisual()?;
        let children = frame.Children()?;
        children.InsertAtTop(&core)?;
        children.InsertAtTop(&meniscus)?;

        Ok(GlassSurface {
            target,
            root,
            frame,
            clip_geom,
            core,
            meniscus,
            mask_surface: None,
            mask_key: None,
            current: None,
        })
    }

    fn core_brush(&self, sigma: f32, vibrancy: f32, dark: bool) -> Result<CompositionEffectBrush> {
        let backdrop_param = CompositionEffectSourceParameter::Create(h!("backdrop"))?;
        let source: IGraphicsEffectSource = backdrop_param.cast()?;
        let blurred = gaussian_blur(source, sigma)?.into_source();
        let saturated = saturation_effect(blurred, vibrancy)?.into_source();
        let toned = color_matrix_effect(saturated, &core_glass_matrix(dark, false))?.into_effect();
        let factory = self.compositor.CreateEffectFactory(&toned)?;
        let brush = factory.CreateBrush()?;
        let host = self.compositor.CreateHostBackdropBrush()?;
        brush.SetSourceParameter(h!("backdrop"), &host)?;
        Ok(brush)
    }

    /// Curved meniscus lens brush: sharp low-sigma diffusion with heightened chromatic
    /// brilliance and subtle cool-prism lift, masked to the outer curved bevel while its
    /// `SpriteVisual` applies inward lens magnification.
    fn meniscus_brush(&self, rim_sigma: f32, dark: bool) -> Result<CompositionEffectBrush> {
        let backdrop_param = CompositionEffectSourceParameter::Create(h!("backdrop"))?;
        let source: IGraphicsEffectSource = backdrop_param.cast()?;
        let crisp = gaussian_blur(source, rim_sigma)?.into_source();
        let vibrant = saturation_effect(crisp, 1.46)?.into_source();
        let lit = color_matrix_effect(vibrant, &core_glass_matrix(dark, true))?.into_effect();
        let factory = self.compositor.CreateEffectFactory(&lit)?;
        let brush = factory.CreateBrush()?;
        let host = self.compositor.CreateHostBackdropBrush()?;
        brush.SetSourceParameter(h!("backdrop"), &host)?;
        Ok(brush)
    }

    /// Paints a smooth SDF bevel mask into `surface` (`alpha = 1` at the outer rounded
    /// rim, feathering quadratically to `0` across `bevel_px` inward).
    fn paint_bevel_mask(
        &self,
        surface: &CompositionDrawingSurface,
        width: i32,
        height: i32,
        radius: f32,
        bevel_px: f32,
    ) -> Result<()> {
        let interop: ICompositionDrawingSurfaceInterop = surface.cast()?;
        let rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        unsafe {
            let mut offset = POINT::default();
            let ctx: ID2D1DeviceContext = interop.BeginDraw(Some(&rect), &mut offset)?;
            ctx.SetDpi(96., 96.);
            let bounds = D2D_RECT_F {
                left: offset.x as f32,
                top: offset.y as f32,
                right: (offset.x + width) as f32,
                bottom: (offset.y + height) as f32,
            };
            ctx.PushAxisAlignedClip(&bounds, D2D1_ANTIALIAS_MODE_ALIASED);
            ctx.Clear(Some(&D2D1_COLOR_F::default()));

            let draw = (|| -> Result<()> {
                let steps = 12usize;
                let step_w = (bevel_px / steps as f32).max(1.) * 1.45;
                for i in 0..steps {
                    let t = i as f32 / steps as f32;
                    let inset = t * bevel_px;
                    let w = width as f32 - 2. * inset;
                    let h = height as f32 - 2. * inset;
                    if w <= 1. || h <= 1. {
                        break;
                    }
                    // Steep meniscus falloff: strong optical bend right at the rim,
                    // blending seamlessly into the diffused crystal core.
                    let alpha = (1. - t).powi(2) * 0.34;
                    let brush = ctx.CreateSolidColorBrush(&color(255, 255, 255, alpha), None)?;
                    let r = (radius - inset).max(0.);
                    let shape = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: offset.x as f32 + inset,
                            top: offset.y as f32 + inset,
                            right: offset.x as f32 + width as f32 - inset,
                            bottom: offset.y as f32 + height as f32 - inset,
                        },
                        radiusX: r,
                        radiusY: r,
                    };
                    ctx.DrawRoundedRectangle(&shape, &brush, step_w, None);
                }
                Ok(())
            })();

            ctx.PopAxisAlignedClip();
            let end = interop.EndDraw();
            draw.and(end)
        }
    }
}

/// Live per-HWND Liquid Glass backdrop tree. Dropping it explicitly closes the WinRT
/// composition target so the same `HWND` can immediately rebind if needed.
pub(super) struct GlassSurface {
    target: DesktopWindowTarget,
    root: ContainerVisual,
    frame: ContainerVisual,
    clip_geom: CompositionRoundedRectangleGeometry,
    core: SpriteVisual,
    meniscus: SpriteVisual,
    mask_surface: Option<CompositionDrawingSurface>,
    mask_key: Option<(i32, i32, u32, u32)>,
    current: Option<GlassSpec>,
}

impl GlassSurface {
    pub fn hide(&mut self) -> Result<()> {
        if self.current.take().is_some() {
            self.frame.SetIsVisible(false)?;
        }
        Ok(())
    }

    pub fn update(&mut self, engine: &GlassEngine, spec: GlassSpec) -> Result<()> {
        if spec.strength <= 0 || spec.width < 1. || spec.height < 1. {
            return self.hide();
        }
        if self.current == Some(spec) {
            return Ok(());
        }
        let scale = spec.dpi.max(96) as f32 / 96.;
        let strength = spec.strength.clamp(1, 100) as f32 / 100.;
        let radius = spec
            .radius
            .min(spec.width * 0.5)
            .min(spec.height * 0.5)
            .max(0.);

        let mut v2 = self.clip_geom.Offset().unwrap_or_default();
        v2.X = spec.x;
        v2.Y = spec.y;
        self.clip_geom.SetOffset(v2)?;
        v2.X = spec.width;
        v2.Y = spec.height;
        self.clip_geom.SetSize(v2)?;
        let size = v2;
        v2.X = radius;
        v2.Y = radius;
        self.clip_geom.SetCornerRadius(v2)?;

        let mut plane = Plane::default();
        plane.Normal.X = spec.x;
        plane.Normal.Y = spec.y;
        plane.Normal.Z = 0.;
        let offset = plane.Normal;
        self.core.SetOffset(offset)?;
        self.core.SetSize(size)?;
        self.meniscus.SetOffset(offset)?;
        self.meniscus.SetSize(size)?;

        // Optical parameters: silky crystal blur in the core, sharp refracted lens at the bevel.
        let core_sigma = (5. + 21. * strength) * scale;
        let vibrancy = 1.18 + 0.24 * strength;
        let core_brush = engine.core_brush(core_sigma, vibrancy, spec.dark)?;
        self.core.SetBrush(&core_brush)?;

        let bevel_px = ((10.5 * scale)
            .min(spec.width * 0.35)
            .min(spec.height * 0.44))
        .max(3.);
        let refract_px = (3.5 + 6.5 * strength) * scale;
        let rim_sigma = (1.4 + 2.2 * strength) * scale;

        let w_px = spec.width.round().max(1.) as i32;
        let h_px = spec.height.round().max(1.) as i32;
        let mask_key = (w_px, h_px, radius.to_bits(), bevel_px.to_bits());
        if self.mask_key != Some(mask_key) {
            let surface = engine.graphics.CreateDrawingSurface(
                Size {
                    Width: w_px as f32,
                    Height: h_px as f32,
                },
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                DirectXAlphaMode::Premultiplied,
            )?;
            engine.paint_bevel_mask(&surface, w_px, h_px, radius, bevel_px)?;
            self.mask_surface = Some(surface);
            self.mask_key = Some(mask_key);
        }

        if let Some(surface) = &self.mask_surface {
            // Apply convex lens magnification on the meniscus SpriteVisual itself
            // (clipped to `frame`), since DWM forbids affine transforms on BackdropBrushes.
            let dx = refract_px.min(spec.width * 0.18).max(0.);
            let dy = refract_px.min(spec.height * 0.24).max(0.);
            let sx = spec.width / (spec.width - 2. * dx).max(1.);
            let sy = spec.height / (spec.height - 2. * dy).max(1.);
            let mut center = Plane::default();
            center.Normal.X = spec.width * 0.5;
            center.Normal.Y = spec.height * 0.5;
            center.Normal.Z = 0.;
            self.meniscus.SetCenterPoint(center.Normal)?;
            center.Normal.X = sx;
            center.Normal.Y = sy;
            center.Normal.Z = 1.;
            self.meniscus.SetScale(center.Normal)?;

            let mask_brush = engine.compositor.CreateMaskBrush()?;
            let alpha_mask = engine.compositor.CreateSurfaceBrushWithSurface(surface)?;
            let lens_source = engine.meniscus_brush(rim_sigma, spec.dark)?;
            mask_brush.SetMask(&alpha_mask)?;
            mask_brush.SetSource(&lens_source)?;
            self.meniscus.SetBrush(&mask_brush)?;
        }

        self.frame.SetIsVisible(true)?;
        self.current = Some(spec);
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
    /// Paints a captured `Backdrop` through a GPU optical Liquid Glass pipeline:
    /// analytical rounded-rectangle SDF normal refraction (`CLSID_D2D1DisplacementMap`)
    /// + edge-clamped Gaussian diffusion + vibrancy saturation + tonal glass balance.
    pub(super) fn liquid_backdrop(
        &self,
        image: &Backdrop,
        area: Area,
        radius: f32,
        strength: f32,
        dark: bool,
    ) -> Result<()> {
        let stride = image
            .width
            .checked_mul(4)
            .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
        if stride == 0
            || image.height == 0
            || (stride as usize).checked_mul(image.height as usize) != Some(image.pixels.len())
        {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let w_dip = (area.right - area.left).max(1.);
        let h_dip = (area.bottom - area.top).max(1.);
        let dest = self.rect(area.left, area.top, area.right, area.bottom);
        let dest_w = (dest.right - dest.left).max(1.);
        let dest_h = (dest.bottom - dest.top).max(1.);
        let strength = strength.clamp(0., 1.);

        let map_w = (dest_w.round() as u32).clamp(16, 256);
        let map_h = (dest_h.round() as u32).clamp(16, 256);
        let disp_pixels = sdf_refraction_map(
            map_w,
            map_h,
            w_dip,
            h_dip,
            radius.min(w_dip * 0.5).min(h_dip * 0.5),
            10.5_f32.min(w_dip * 0.35).min(h_dip * 0.44),
        );

        // GDI 32-bit BI_RGB DIBs leave the high byte zeroed; set full coverage before
        // binding as a premultiplied DXGI texture in the D2D1 effect shader graph.
        let mut opaque_pixels = image.pixels.clone();
        for px in opaque_pixels.chunks_exact_mut(4) {
            px[3] = 255;
        }
        let map_props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.,
            dpiY: 96.,
            ..Default::default()
        };

        self.clipped_round(
            area.left,
            area.top,
            area.right,
            area.bottom,
            radius,
            || unsafe {
                let src_bmp = self.ctx.CreateBitmap(
                    D2D_SIZE_U {
                        width: image.width,
                        height: image.height,
                    },
                    Some(opaque_pixels.as_ptr().cast()),
                    stride,
                    &map_props,
                )?;
                let map_bmp = self.ctx.CreateBitmap(
                    D2D_SIZE_U {
                        width: map_w,
                        height: map_h,
                    },
                    Some(disp_pixels.as_ptr().cast()),
                    map_w * 4,
                    &map_props,
                )?;

                // Scale both inputs into physical destination pixels before refraction & blur.
                let scale_src = self.ctx.CreateEffect(&CLSID_D2D12DAffineTransform)?;
                scale_src.SetInput(0, &src_bmp, true);
                let interp = 1u32; // Linear
                let hard = 1u32; // Hard border
                scale_src.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_INTERPOLATION_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &interp.to_ne_bytes(),
                )?;
                scale_src.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_BORDER_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &hard.to_ne_bytes(),
                )?;
                let src_m: [f32; 6] = [
                    dest_w / image.width as f32,
                    0.,
                    0.,
                    dest_h / image.height as f32,
                    0.,
                    0.,
                ];
                let src_m_bytes: Vec<u8> = src_m.iter().flat_map(|v| v.to_ne_bytes()).collect();
                scale_src.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                    D2D1_PROPERTY_TYPE_MATRIX_3X2,
                    &src_m_bytes,
                )?;

                let scale_map = self.ctx.CreateEffect(&CLSID_D2D12DAffineTransform)?;
                scale_map.SetInput(0, &map_bmp, true);
                scale_map.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_INTERPOLATION_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &interp.to_ne_bytes(),
                )?;
                scale_map.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_BORDER_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &hard.to_ne_bytes(),
                )?;
                let map_m: [f32; 6] =
                    [dest_w / map_w as f32, 0., 0., dest_h / map_h as f32, 0., 0.];
                let map_m_bytes: Vec<u8> = map_m.iter().flat_map(|v| v.to_ne_bytes()).collect();
                scale_map.SetValue(
                    D2D1_2DAFFINETRANSFORM_PROP_TRANSFORM_MATRIX.0 as u32,
                    D2D1_PROPERTY_TYPE_MATRIX_3X2,
                    &map_m_bytes,
                )?;

                // Blur the interior first (with hard border clamping to finite bounds),
                // then refract along the curved meniscus normal map.
                let blur = self.ctx.CreateEffect(&CLSID_D2D1GaussianBlur)?;
                blur.SetInput(0, &scale_src.GetOutput()?, true);
                let sigma = (3.5 + 16.5 * strength) * self.scale;
                blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &sigma.to_ne_bytes(),
                )?;
                blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &hard.to_ne_bytes(),
                )?;

                let displace = self.ctx.CreateEffect(&CLSID_D2D1DisplacementMap)?;
                displace.SetInput(0, &blur.GetOutput()?, true);
                displace.SetInput(1, &scale_map.GetOutput()?, true);
                let disp_scale = (6. + 12. * strength) * self.scale;
                let ch_r = D2D1_CHANNEL_SELECTOR_R.0 as u32;
                let ch_g = D2D1_CHANNEL_SELECTOR_G.0 as u32;
                displace.SetValue(
                    D2D1_DISPLACEMENTMAP_PROP_SCALE.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &disp_scale.to_ne_bytes(),
                )?;
                displace.SetValue(
                    D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &ch_r.to_ne_bytes(),
                )?;
                displace.SetValue(
                    D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT.0 as u32,
                    D2D1_PROPERTY_TYPE_ENUM,
                    &ch_g.to_ne_bytes(),
                )?;

                let sat = self.ctx.CreateEffect(&CLSID_D2D1Saturation)?;
                sat.SetInput(0, &displace.GetOutput()?, true);
                let vibrancy = 1.18 + 0.22 * strength;
                sat.SetValue(
                    D2D1_SATURATION_PROP_SATURATION.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &vibrancy.to_ne_bytes(),
                )?;

                let tone = self.ctx.CreateEffect(&CLSID_D2D1ColorMatrix)?;
                tone.SetInput(0, &sat.GetOutput()?, true);
                let matrix_bytes: Vec<u8> = core_glass_matrix(dark, false)
                    .iter()
                    .flat_map(|v| v.to_ne_bytes())
                    .collect();
                tone.SetValue(
                    D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32,
                    D2D1_PROPERTY_TYPE_MATRIX_5X4,
                    &matrix_bytes,
                )?;

                let output = tone.GetOutput()?;
                let mut prev = Default::default();
                self.ctx.GetTransform(&mut prev);
                let mut shifted = prev;
                shifted.M31 = dest.left;
                shifted.M32 = dest.top;
                self.ctx.SetTransform(&shifted);
                let crop = D2D_RECT_F {
                    left: 0.,
                    top: 0.,
                    right: dest_w,
                    bottom: dest_h,
                };
                self.ctx.DrawImage(
                    &output,
                    None,
                    Some(&crop),
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
                self.ctx.SetTransform(&prev);
                Ok(())
            },
        )
    }

    /// Draws the 3D optical Liquid Glass shell: curved volume sheen, feathered meniscus
    /// inner luminescence, prismatic cyan/violet dispersion sub-rim, and a directional
    /// 5-stop Fresnel specular outer rim.
    pub(super) fn liquid_glass_shell(
        &self,
        area: Area,
        radius: f32,
        spec: LiquidShell,
    ) -> Result<()> {
        let (l, t, r, b) = (area.left, area.top, area.right, area.bottom);
        if r <= l || b <= t {
            return Ok(());
        }
        let radius = radius.min((r - l) * 0.5).min((b - t) * 0.5).max(0.);
        let opacity = spec.opacity.clamp(0., 1.);
        let rim = spec.rim.clamp(0., 1.8);

        if opacity > 0. {
            if spec.glass_active && opacity < 0.96 {
                let (sheen_top, caustic_bottom) = if self.p.dark {
                    ((96, 114, 145), (72, 92, 126))
                } else {
                    ((255, 255, 255), (242, 249, 255))
                };
                let top_a = (opacity * 1.18 + 0.06 * rim.min(1.)).min(1.);
                let upper_a = (opacity * 0.90).min(1.);
                let core_a = (opacity * 0.72).min(1.);
                let lower_a = (opacity * 0.84).min(1.);
                let bot_a = (opacity * 1.06 + 0.04 * rim.min(1.)).min(1.);
                self.gradient_stops(
                    l,
                    t,
                    r,
                    b,
                    radius,
                    &[
                        (0.00, (sheen_top, top_a)),
                        (0.18, (spec.top_tint, upper_a)),
                        (0.56, (spec.bottom_tint, core_a)),
                        (0.86, (spec.bottom_tint, lower_a)),
                        (1.00, (caustic_bottom, bot_a)),
                    ],
                )?;
            } else {
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
        }

        if rim <= 0.001 {
            return Ok(());
        }

        // Inner curved meniscus volume luminescence + prismatic sub-rim.
        self.clipped_round(l, t, r, b, radius, || {
            let glow_scale = if self.p.dark { 0.82 } else { 1.0 } * rim;
            for (inset, width, top_a, mid_a, bot_a) in [
                (0.9, 1.4, 0.24, 0.05, 0.17),
                (2.2, 2.0, 0.13, 0.025, 0.09),
                (4.0, 2.8, 0.065, 0.01, 0.045),
            ] {
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

/// Generates a BGRA displacement map encoding inward rounded-rectangle SDF normals
/// (`R` = X displacement, `G` = Y displacement, `128` = neutral) across `bevel_dip`.
fn sdf_refraction_map(
    map_w: u32,
    map_h: u32,
    w_dip: f32,
    h_dip: f32,
    radius_dip: f32,
    bevel_dip: f32,
) -> Vec<u8> {
    let mut pixels = vec![128u8; (map_w * map_h * 4) as usize];
    let hx = w_dip * 0.5;
    let hy = h_dip * 0.5;
    let r = radius_dip.min(hx).min(hy).max(0.);
    let bx = (hx - r).max(0.);
    let by = (hy - r).max(0.);
    let bevel = bevel_dip.max(0.5);

    for py in 0..map_h {
        let y = (py as f32 + 0.5) / map_h as f32 * h_dip - hy;
        let sy = y.signum();
        let ay = y.abs();
        let qy = ay - by;
        for px in 0..map_w {
            let x = (px as f32 + 0.5) / map_w as f32 * w_dip - hx;
            let sx = x.signum();
            let ax = x.abs();
            let qx = ax - bx;

            let ox = qx.max(0.);
            let oy = qy.max(0.);
            let outside_len = (ox * ox + oy * oy).sqrt();
            let inside_dist = qx.max(qy).min(0.);
            let sdf = outside_len + inside_dist - r;

            // Outward unit normal of the rounded rectangle SDF.
            let (nx, ny) = if outside_len > 1e-4 {
                (sx * ox / outside_len, sy * oy / outside_len)
            } else if qx > qy {
                (sx, 0.)
            } else {
                (0., sy)
            };

            let depth = (-sdf).max(0.);
            let (dx, dy) = if depth < bevel {
                let slope = (1. - depth / bevel).powf(2.2);
                // Sample inward (-normal) so the curved glass rim magnifies background light.
                (-nx * slope, -ny * slope)
            } else {
                (0., 0.)
            };

            let idx = ((py * map_w + px) * 4) as usize;
            // BGRA byte layout: B=128, G=dy, R=dx, A=255.
            pixels[idx] = 128;
            pixels[idx + 1] = (128. + dy * 127.).round().clamp(1., 254.) as u8;
            pixels[idx + 2] = (128. + dx * 127.).round().clamp(1., 254.) as u8;
            pixels[idx + 3] = 255;
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::canvas::{Palette, paint_pixels};
    use crate::settings::Settings;
    use windows::Win32::Graphics::{Direct3D::D3D_DRIVER_TYPE_WARP, Dxgi::IDXGIDevice};

    #[test]
    fn sdf_refraction_pulls_inward_along_all_four_bevels_and_stays_neutral_at_center() {
        let map = sdf_refraction_map(64, 32, 128., 64., 24., 12.);
        let sample = |x: u32, y: u32| {
            let i = ((y * 64 + x) * 4) as usize;
            // (dx_byte, dy_byte)
            (map[i + 2], map[i + 1])
        };
        // Center is flat glass: zero refraction displacement.
        assert_eq!(sample(32, 16), (128, 128));
        // Top rim refracts downward (+Y); bottom rim refracts upward (-Y).
        assert!(sample(32, 0).1 > 180);
        assert!(sample(32, 31).1 < 76);
        // Left cap refracts rightward (+X); right cap refracts leftward (-X).
        assert!(sample(0, 16).0 > 180);
        assert!(sample(63, 16).0 < 76);
    }

    #[test]
    fn liquid_backdrop_and_shell_clip_to_capsule_and_light_fresnel_rim() -> Result<()> {
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
            c.liquid_backdrop(&backdrop, area, 20., 0.65, true)?;
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
        // Top Fresnel specular rim is brighter than the interior just beneath it.
        let rim_luma: u32 = at(48, 4)[..3].iter().map(|&v| u32::from(v)).sum();
        let body_luma: u32 = at(48, 14)[..3].iter().map(|&v| u32::from(v)).sum();
        assert!(rim_luma > body_luma);
        Ok(())
    }

    #[test]
    fn live_glass_engine_coexists_with_dcomp_and_rebinds_cleanly() -> Result<()> {
        use windows::Win32::{
            System::{
                LibraryLoader::GetModuleHandleW,
                WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize},
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
                WS_EX_TOOLWINDOW, WS_POPUP,
            },
        };
        use windows::core::w;

        struct WinRtGuard(bool);
        impl Drop for WinRtGuard {
            fn drop(&mut self) {
                if self.0 {
                    unsafe { RoUninitialize() };
                }
            }
        }
        struct WindowGuard(HWND);
        impl Drop for WindowGuard {
            fn drop(&mut self) {
                let _ = unsafe { DestroyWindow(self.0) };
            }
        }

        let _winrt = WinRtGuard(unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }.is_ok());
        let hwnd = WindowGuard(unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                w!("STATIC"),
                None,
                WS_POPUP,
                0,
                0,
                320,
                48,
                None,
                None,
                Some(GetModuleHandleW(None)?.into()),
                None,
            )?
        });
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
                    dark: true,
                    dpi: 96,
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
                dark: false,
                dpi: 144,
            },
        )
    }
}
