use windows::Win32::{
    Foundation::HWND,
    Graphics::{DirectComposition::*, DirectWrite::*},
};
use windows::core::Result;

use crate::machine::Form;
use crate::ui::about::{self, AboutHit, AboutLayout, Line};
use crate::ui::icon::Symbol;
use crate::ui::panel::Area;

use super::Renderer;
use super::canvas::{Canvas, Rgb, Text, paint};

const BLACK: Rgb = (0, 0, 0);
/// The macOS close light and the cross it shows under the pointer.
const CLOSE_LIGHT: Rgb = (255, 95, 87);
const CLOSE_CROSS: Rgb = (110, 12, 8);
const LIGHT_RADIUS: f32 = 6.;
/// The black glass around the screen and the anodized edge around that.
const BEZEL: Rgb = (24, 24, 26);
const EDGE: Rgb = (176, 178, 184);

/// The card's tones over the acrylic backdrop, and the opaque fill used without it.
struct Tones {
    glass: (Rgb, f32),
    top: Rgb,
    bottom: Rgb,
    screen_top: Rgb,
    screen_bottom: Rgb,
    metal_top: Rgb,
    metal_bottom: Rgb,
}

impl Tones {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                glass: ((34, 33, 36), 0.5),
                top: (48, 47, 50),
                bottom: (32, 32, 34),
                screen_top: (74, 76, 86),
                screen_bottom: (46, 48, 56),
                metal_top: (150, 152, 158),
                metal_bottom: (92, 94, 100),
            }
        } else {
            Self {
                glass: ((250, 246, 241), 0.42),
                top: (250, 247, 243),
                bottom: (236, 231, 225),
                screen_top: (206, 200, 190),
                screen_bottom: (176, 169, 158),
                metal_top: (228, 229, 232),
                metal_bottom: (156, 158, 164),
            }
        }
    }
}

/// The About card's composition target. It repaints only on hover and theme changes.
pub(super) struct AboutView {
    hwnd: HWND,
    glass: Option<super::glass::GlassSurface>,
    // The target must outlive all commits to its root.
    _target: IDCompositionTarget,
    root: IDCompositionVisual2,
}

impl Renderer {
    /// Paints `layout` into `hwnd`, the About window, binding to it on first use and again
    /// after the device was rebuilt. With `backdrop > 0` the card renders translucent
    /// Liquid Glass over the live desktop behind it.
    pub fn draw_about(
        &mut self,
        hwnd: HWND,
        layout: &AboutLayout,
        dpi: u32,
        backdrop: i32,
    ) -> Result<()> {
        if self.about.as_ref().map(|view| view.hwnd) != Some(hwnd) {
            self.close_about();
            let glass_surface = self.glass.as_ref().and_then(|g| g.attach(hwnd).ok());
            unsafe {
                let target = self.device.CreateTargetForHwnd(hwnd, true)?;
                let root = self.device.CreateVisual()?;
                target.SetRoot(&root)?;
                self.about = Some(AboutView {
                    hwnd,
                    glass: glass_surface,
                    _target: target,
                    root,
                });
            }
        }
        let (width, height) = layout.pixels();
        let scale = dpi as f32 / 96.;
        let mut live_glass = false;
        if let (Some(engine), Some(about_view)) = (&self.glass, self.about.as_mut())
            && let Some(surface) = &mut about_view.glass
        {
            live_glass = backdrop > 0
                && surface
                    .update(
                        engine,
                        super::glass::GlassSpec {
                            x: 0.,
                            y: 0.,
                            width: width as f32,
                            height: height as f32,
                            radius: 8. * scale,
                            strength: backdrop,
                            rim: super::glass::rim::PANEL,
                        },
                    )
                    .is_ok();
            if !live_glass {
                let _ = surface.hide();
            }
        }
        let eff_backdrop = if live_glass || self.glass.is_none() {
            backdrop
        } else {
            0
        };
        let surface = paint(&self.device, self.palette, width, height, dpi, |c| {
            self.paint_about(c, layout, eff_backdrop)
        })?;
        let view = self.about.as_ref().expect("about card bound above");
        unsafe {
            view.root.SetContent(&surface)?;
            self.device.Commit()
        }
    }

    /// Releases the card's composition target; call before destroying its window.
    pub fn close_about(&mut self) {
        if self.about.take().is_some() {
            let _ = unsafe { self.device.Commit() };
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn about_text(
        &self,
        c: &Canvas,
        line: &Line,
        size: f32,
        bold: bool,
        ink: Rgb,
        alpha: f32,
        align: DWRITE_TEXT_ALIGNMENT,
    ) -> Result<()> {
        let a = line.area;
        c.draw_text(
            &self.write,
            &line.text,
            c.rect(a.left, a.top, a.right, a.bottom),
            Text { size, bold, align },
            ink,
            alpha,
        )
    }

    fn paint_about(&self, c: &Canvas, layout: &AboutLayout, backdrop: i32) -> Result<()> {
        let t = Tones::new(c.p.dark);
        let (w, h) = (layout.width, layout.height);
        if backdrop > 0 {
            let (tint, _) = t.glass;
            let strength = backdrop as f32 / 100.;
            let opacity = 1. - 0.74 * strength;
            let top_tint = if c.p.dark {
                (42, 48, 64)
            } else {
                (252, 254, 255)
            };
            c.liquid_glass_shell(
                Area::new(0., 0., w, h),
                8.,
                super::glass::LiquidShell {
                    top_tint,
                    bottom_tint: tint,
                    opacity,
                    rim: 0.85 * strength,
                    glass_active: true,
                },
            )?;
        } else {
            c.gradient(0., 0., w, h, 0., (t.top, 1.), (t.bottom, 1.))?;
        }

        // Only the close light: the card can neither be minimized nor zoomed.
        let close = layout.close;
        let (x, y) = ((close.left + close.right) / 2., close.middle());
        let r = LIGHT_RADIUS;
        c.round(x - r, y - r, x + r, y + r, r, CLOSE_LIGHT)?;
        c.stroke(x - r, y - r, x + r, y + r, r, BLACK, 0.15)?;
        if layout.hover == Some(AboutHit::Close) {
            c.symbol_in(Symbol::Close, close, 10., CLOSE_CROSS, 0.9)?;
        }

        match layout.form {
            Form::Laptop => self.paint_laptop(c, &t, layout.figure)?,
            Form::Desktop => self.paint_display(c, &t, layout.figure)?,
        }

        let center = DWRITE_TEXT_ALIGNMENT_CENTER;
        self.about_text(c, &layout.name, about::NAME_SIZE, true, c.p.ink, 1., center)?;
        if let Some(model) = &layout.model {
            self.about_text(c, model, about::MODEL_SIZE, false, c.p.muted, 1., center)?;
        }
        for row in &layout.rows {
            let label = Line {
                text: row.label.to_owned(),
                area: row.label_area,
            };
            let trailing = DWRITE_TEXT_ALIGNMENT_TRAILING;
            self.about_text(c, &label, about::ROW_SIZE, true, c.p.ink, 1., trailing)?;
            for line in &row.lines {
                let leading = DWRITE_TEXT_ALIGNMENT_LEADING;
                self.about_text(c, line, about::ROW_SIZE, false, c.p.ink, 0.88, leading)?;
            }
        }

        // A macOS push button on the vibrant background: a translucent face and rim.
        let b = layout.more;
        let lit = layout.hover == Some(AboutHit::More);
        c.fill(
            b.left,
            b.top,
            b.right,
            b.bottom,
            6.,
            c.p.line,
            if lit { 0.14 } else { 0.08 },
        )?;
        c.stroke(b.left, b.top, b.right, b.bottom, 6., c.p.line, 0.08)?;
        let label = Line {
            text: "更多信息".to_owned(),
            area: b,
        };
        self.about_text(c, &label, about::ROW_SIZE, false, c.p.ink, 1., center)
    }

    /// A screen in its bezel and anodized edge, with the display lit by a soft gradient.
    fn paint_screen(&self, c: &Canvas, t: &Tones, s: Area, radius: f32) -> Result<()> {
        c.round(s.left, s.top, s.right, s.bottom, radius, EDGE)?;
        let inner = 1.5;
        c.round(
            s.left + inner,
            s.top + inner,
            s.right - inner,
            s.bottom - inner,
            radius - inner,
            BEZEL,
        )?;
        let bezel = 5.;
        c.gradient(
            s.left + bezel,
            s.top + bezel,
            s.right - bezel,
            s.bottom - bezel,
            2.,
            (t.screen_top, 1.),
            (t.screen_bottom, 1.),
        )?;
        // The camera in the top bezel.
        let cx = (s.left + s.right) / 2.;
        let cy = s.top + bezel / 2. + 0.5;
        c.fill(cx - 1., cy - 1., cx + 1., cy + 1., 1., (70, 74, 84), 1.)
    }

    /// An open laptop seen from the front: the lid's screen above a thin base.
    fn paint_laptop(&self, c: &Canvas, t: &Tones, f: Area) -> Result<()> {
        let cx = (f.left + f.right) / 2.;
        let lid = Area::new(cx - 74., f.top + 4., cx + 74., f.bottom - 12.);
        self.paint_screen(c, t, lid, 9.)?;
        let base = Area::new(cx - 90., lid.bottom, cx + 90., lid.bottom + 7.);
        // A soft contact shadow under the base.
        c.fill(
            base.left + 8.,
            base.bottom - 1.,
            base.right - 8.,
            base.bottom + 3.,
            2.,
            BLACK,
            0.12,
        )?;
        c.gradient(
            base.left,
            base.top,
            base.right,
            base.bottom,
            3.5,
            (t.metal_top, 1.),
            (t.metal_bottom, 1.),
        )?;
        c.stroke(
            base.left,
            base.top,
            base.right,
            base.bottom,
            3.5,
            BLACK,
            0.18,
        )?;
        // The groove that lets a thumb lift the lid.
        c.fill(
            cx - 16.,
            base.top,
            cx + 16.,
            base.top + 2.5,
            1.2,
            BLACK,
            0.22,
        )
    }

    /// A desktop display on a stand.
    fn paint_display(&self, c: &Canvas, t: &Tones, f: Area) -> Result<()> {
        let cx = (f.left + f.right) / 2.;
        let screen = Area::new(cx - 76., f.top + 2., cx + 76., f.bottom - 22.);
        let neck = Area::new(cx - 12., screen.bottom, cx + 12., f.bottom - 5.);
        c.gradient(
            neck.left,
            neck.top,
            neck.right,
            neck.bottom,
            0.,
            (t.metal_bottom, 1.),
            (t.metal_top, 1.),
        )?;
        c.fill(
            cx - 34.,
            f.bottom - 5.,
            cx + 34.,
            f.bottom - 1.,
            2.,
            BLACK,
            0.1,
        )?;
        c.gradient(
            cx - 32.,
            f.bottom - 6.,
            cx + 32.,
            f.bottom - 2.,
            2.,
            (t.metal_top, 1.),
            (t.metal_bottom, 1.),
        )?;
        self.paint_screen(c, t, screen, 6.)
    }
}
