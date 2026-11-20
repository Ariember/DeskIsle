use windows::Win32::{
    Foundation::POINT,
    Graphics::{
        Direct2D::{Common::*, *},
        DirectComposition::{IDCompositionDesktopDevice, IDCompositionSurface},
        DirectWrite::*,
        Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM},
        Imaging::IWICBitmapSource,
    },
};
use windows::core::{Result, w};

pub(super) type Rgb = (u8, u8, u8);

/// Monochrome colors for one appearance. Washes and hairlines use `line` at low alpha,
/// so the same alphas read on both light and dark surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub(super) bar: Rgb,
    pub(super) panel: Rgb,
    pub(super) panel_top: Rgb,
    pub(super) edge: Rgb,
    pub(super) ink: Rgb,
    pub(super) muted: Rgb,
    /// Selection, active state and switched-on controls.
    pub(super) accent: Rgb,
    /// Text and knobs drawn on an `accent` fill.
    pub(super) on_accent: Rgb,
    pub(super) line: Rgb,
    pub(super) danger: Rgb,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                bar: (16, 16, 16),
                panel: (26, 26, 26),
                panel_top: (40, 40, 40),
                edge: (66, 66, 66),
                ink: (244, 244, 244),
                muted: (168, 168, 168),
                accent: (244, 244, 244),
                on_accent: (18, 18, 18),
                line: (255, 255, 255),
                danger: (255, 128, 116),
            }
        } else {
            Self {
                bar: (246, 246, 246),
                panel: (238, 238, 238),
                panel_top: (252, 252, 252),
                edge: (204, 204, 204),
                ink: (22, 22, 22),
                muted: (102, 102, 102),
                accent: (22, 22, 22),
                on_accent: (250, 250, 250),
                line: (0, 0, 0),
                danger: (196, 43, 28),
            }
        }
    }
}

pub(super) struct Canvas {
    ctx: ID2D1DeviceContext,
    pub(super) p: Palette,
    origin: POINT,
    pub(super) scale: f32,
}

pub(super) fn paint(
    device: &IDCompositionDesktopDevice,
    palette: Palette,
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
            p: palette,
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
    pub(super) fn rect(&self, left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.origin.x as f32 + left * self.scale,
            top: self.origin.y as f32 + top * self.scale,
            right: self.origin.x as f32 + right * self.scale,
            bottom: self.origin.y as f32 + bottom * self.scale,
        }
    }

    /// A square in physical pixels relative to the surface.
    pub(super) fn pixels(&self, left: f32, top: f32, size: f32) -> D2D_RECT_F {
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

    pub(super) fn round(
        &self,
        l: f32,
        t: f32,
        r: f32,
        b: f32,
        radius: f32,
        color: Rgb,
    ) -> Result<()> {
        self.fill(l, t, r, b, radius, color, 1.)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn fill(
        &self,
        l: f32,
        t: f32,
        r: f32,
        b: f32,
        radius: f32,
        color: Rgb,
        a: f32,
    ) -> Result<()> {
        let shape = self.shape(l, t, r, b, radius);
        unsafe {
            self.ctx
                .FillRoundedRectangle(&shape, &self.brush_alpha(color, a)?)
        };
        Ok(())
    }

    /// A one-pixel line just inside the edges, so it stays crisp at every scale.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn stroke(
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
    pub(super) fn gradient(
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
    pub(super) fn shadow(
        &self,
        l: f32,
        t: f32,
        r: f32,
        b: f32,
        radius: f32,
        spread: f32,
    ) -> Result<()> {
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
                0.02,
            )?;
        }
        Ok(())
    }

    pub(super) fn panel(&self, w: f32, h: f32, radius: f32) -> Result<()> {
        let p = self.p;
        self.gradient(0., 0., w, h, radius, (p.panel_top, 0.97), (p.panel, 0.97))?;
        self.stroke(0., 0., w, h, radius, p.line, 0.11)
    }

    /// Runs `draw` with output limited to `rect`.
    pub(super) fn clipped(
        &self,
        rect: D2D_RECT_F,
        draw: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        unsafe {
            self.ctx
                .PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_ALIASED)
        };
        let result = draw();
        unsafe { self.ctx.PopAxisAlignedClip() };
        result
    }

    pub(super) fn bitmap(&self, bitmap: &IWICBitmapSource, rect: D2D_RECT_F) -> Result<()> {
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

    pub(super) fn text(
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
    pub(super) fn glyph(
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

    pub(super) fn draw_text(
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
pub(super) struct Text {
    pub(super) size: f32,
    pub(super) bold: bool,
    pub(super) glyph: bool,
    pub(super) align: DWRITE_TEXT_ALIGNMENT,
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
pub(super) fn text_format(
    factory: &IDWriteFactory,
    style: Text,
    size: f32,
) -> Result<IDWriteTextFormat> {
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
