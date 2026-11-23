use super::*;
use windows::core::Interface;

/// A reflection uses the 15-DIP inset between the icon and the lower edge of the Dock.
#[derive(Clone, Copy)]
pub(in crate::render) struct Reflection {
    space: i32,
    gap: f32,
    blur: f32,
    opacity: f32,
    fade: f32,
}

impl Reflection {
    pub(in crate::render) fn new(settings: &Settings, dpi: u32) -> Option<Self> {
        let get = |key| settings.get(Key::Appearance(key));
        if get(AppearanceKey::Reflection) == 0 || get(AppearanceKey::ReflectionOpacity) == 0 {
            return None;
        }
        let scale = dpi as f32 / 96.;
        Some(Self {
            space: (15. * scale).round() as i32,
            gap: get(AppearanceKey::ReflectionDistance) as f32 * scale,
            blur: get(AppearanceKey::ReflectionBlur) as f32 * scale * 0.04,
            opacity: get(AppearanceKey::ReflectionOpacity) as f32 / 100.,
            fade: get(AppearanceKey::ReflectionFade) as f32 / 100.,
        })
    }

    pub(in crate::render) fn reserve(self) -> i32 {
        self.space
    }
}

impl Canvas {
    /// Rasterize the icon once on this device. The upright image and its mirrored,
    /// blurred copy are baked into the icon layer together so drag and hover stay in sync.
    pub(in crate::render) fn reflected_icon(
        &self,
        size: i32,
        reflection: Reflection,
        draw: impl FnOnce(&Canvas) -> Result<()>,
    ) -> Result<()> {
        unsafe {
            let local = self
                .ctx
                .GetDevice()?
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let image = local.CreateBitmap(
                D2D_SIZE_U {
                    width: size as u32,
                    height: size as u32,
                },
                None,
                0,
                &D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.,
                    dpiY: 96.,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                    ..Default::default()
                },
            )?;
            local.SetTarget(&image);
            local.SetDpi(96., 96.);
            local.SetTextAntialiasMode(self.ctx.GetTextAntialiasMode());
            local.BeginDraw();
            local.Clear(Some(&D2D1_COLOR_F::default()));
            let result = draw(&Canvas {
                ctx: local.clone(),
                p: self.p,
                origin: POINT::default(),
                scale: 1.,
            });
            result.and(local.EndDraw(None, None))?;
            local.SetTarget(None);

            let size = size as f32;
            let top = size + reflection.gap;
            let bottom = size + reflection.space as f32;
            let mirrored: ID2D1Image = if reflection.blur > 0. {
                let blur = self.ctx.CreateEffect(&CLSID_D2D1GaussianBlur)?;
                blur.SetInput(0, &image, true);
                blur.SetValue(
                    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                    D2D1_PROPERTY_TYPE_FLOAT,
                    &reflection.blur.to_ne_bytes(),
                )?;
                blur.GetOutput()?
            } else {
                image.cast()?
            };

            let mask = self.rect(0., top, size, bottom);
            self.clipped(mask, || {
                let ink = ((255, 255, 255), reflection.opacity);
                if reflection.fade == 0. {
                    self.fill(0., top, size, bottom, 0., ink.0, ink.1)?;
                } else {
                    self.gradient_stops(
                        0.,
                        top,
                        size,
                        bottom,
                        0.,
                        &[(0., ink), (1. - reflection.fade, ink), (1., (ink.0, 0.))],
                    )?;
                }
                let mut previous = Default::default();
                self.ctx.GetTransform(&mut previous);
                let mut flip = previous;
                // Compress the reflection to half the icon height before clipping
                // it to the remaining space. The icon's bottom row meets the gap.
                flip.M22 = -0.5;
                flip.M31 = mask.left;
                flip.M32 = mask.top + size * 0.5;
                self.ctx.SetTransform(&flip);
                self.ctx.DrawImage(
                    &mirrored,
                    None,
                    None,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_IN,
                );
                self.ctx.SetTransform(&previous);
                Ok(())
            })?;
            self.ctx.DrawBitmap(
                &image,
                Some(&self.rect(0., 0., size, size)),
                1.,
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
        }
        Ok(())
    }
}
