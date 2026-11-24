use super::{Bar, Renderer, Text, paint};
use crate::geometry::Rect;
use crate::platform::backdrop::Backdrop;
use crate::settings::{AppearanceKey as A, Key};
use crate::ui::panel::Area;
use windows::Win32::{Foundation::HWND, Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_CENTER};
use windows::core::Result;

pub(crate) struct LabelView {
    _glass: Option<super::glass::GlassSurface>,
    _bar: Bar,
}

const LABEL_RADIUS: f32 = 9.;

impl Renderer {
    pub(crate) fn draw_label(
        &self,
        hwnd: HWND,
        rect: Rect,
        dpi: u32,
        text: &str,
        image: Option<&Backdrop>,
    ) -> Result<LabelView> {
        let bar = Bar::new(&self.device, hwnd)?;
        let scale = dpi as f32 / 96.;
        let strength = self.appearance.get(Key::Backdrop);
        let opacity = self.appearance.get(Key::Appearance(A::LabelOpacity)) as f32 / 100.;
        let border = self.appearance.get(Key::Appearance(A::LabelBorder)) as f32 / 100.;
        let glass_enabled = self.transparency && opacity < 1.;
        let mut glass_surface = None;
        if glass_enabled
            && let Some(engine) = &self.glass
            && let Ok(mut surface) = engine.attach(hwnd)
        {
            let spec = super::glass::GlassSpec {
                x: 0.,
                y: 0.,
                width: rect.width as f32,
                height: rect.height as f32,
                radius: LABEL_RADIUS * scale,
                strength,
                rim: super::glass::rim::LABEL,
            };
            if surface.update(engine, spec).is_ok() {
                glass_surface = Some(surface);
            }
        }
        let glass_active = glass_surface.is_some() || (glass_enabled && image.is_some());
        let (top_tint, bottom_tint) = if self.palette.dark {
            ((38, 46, 62), (18, 22, 32))
        } else {
            ((252, 254, 255), (235, 243, 252))
        };
        let surface = paint(
            &self.device,
            self.palette,
            rect.width,
            rect.height,
            dpi,
            |c| {
                let w = rect.width as f32 / c.scale;
                let h = rect.height as f32 / c.scale;
                let area = Area::new(0., 0., w, h);
                if glass_surface.is_none()
                    && glass_enabled
                    && let Some(image) = image
                {
                    c.liquid_backdrop(
                        image,
                        area,
                        LABEL_RADIUS,
                        super::glass::rim::LABEL,
                        strength as f32 / 100.,
                    )?;
                }
                let eff_opacity = if glass_active {
                    opacity * (1. - 0.50 * (strength as f32 / 100.) * (1. - opacity))
                } else {
                    opacity.max(0.88)
                };
                c.liquid_glass_shell(
                    area,
                    LABEL_RADIUS,
                    super::glass::LiquidShell {
                        top_tint,
                        bottom_tint,
                        opacity: eff_opacity,
                        rim: 0.38 + 0.95 * border,
                        glass_active,
                    },
                )?;
                c.draw_text(
                    &self.write,
                    text,
                    c.rect(6., 0., w - 6., h),
                    Text {
                        size: 13.,
                        align: DWRITE_TEXT_ALIGNMENT_CENTER,
                        ..Text::default()
                    },
                    c.p.ink,
                    1.,
                )
            },
        )?;
        unsafe {
            bar.root.SetContent(&surface)?;
            self.device.Commit()?;
        }
        Ok(LabelView {
            _glass: glass_surface,
            _bar: bar,
        })
    }
}
