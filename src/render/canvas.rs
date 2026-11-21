use windows::Win32::{
    Foundation::{POINT, RECT},
    Graphics::{
        Direct2D::{Common::*, *},
        DirectComposition::{IDCompositionDesktopDevice, IDCompositionSurface},
        DirectWrite::*,
        Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM},
        Imaging::IWICBitmapSource,
    },
};
use windows::core::{Result, w};

use crate::settings::{Key, Settings};
use crate::ui::icon::{self, Paint, Step, Symbol, Tone};
use crate::ui::panel::Area;

pub(super) type Rgb = (u8, u8, u8);

pub(super) fn accent_color(index: i32, dark: bool) -> Rgb {
    let (light, night) = [
        ((0, 104, 218), (14, 108, 213)),
        ((141, 67, 172), (145, 73, 180)),
        ((187, 43, 117), (192, 50, 125)),
        ((191, 52, 50), (195, 57, 58)),
        ((177, 79, 6), (178, 83, 11)),
        ((50, 123, 53), (59, 125, 58)),
        ((99, 103, 111), (107, 110, 117)),
    ][index as usize];
    if dark { night } else { light }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Typography {
    family: i32,
    bold: bool,
    scale: i32,
    smooth: bool,
}

/// Shared drawing settings for a surface, included in painted layer cache keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub(super) typography: Typography,
    /// Whether this is the dark appearance, for surfaces that pick their own tones.
    pub(super) dark: bool,
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
    pub fn new(dark: bool, settings: &Settings) -> Self {
        let typography = Typography {
            family: settings.get(Key::FontFamily),
            bold: settings.enabled(Key::BoldFont),
            scale: settings.get(Key::FontScale),
            smooth: settings.enabled(Key::SmoothText),
        };
        let accent = accent_color(settings.get(Key::Accent), dark);
        if dark {
            Self {
                typography,
                dark,
                bar: (16, 16, 16),
                panel: (26, 26, 26),
                panel_top: (40, 40, 40),
                edge: (66, 66, 66),
                ink: (244, 244, 244),
                muted: (168, 168, 168),
                accent,
                on_accent: (255, 255, 255),
                line: (255, 255, 255),
                danger: (255, 128, 116),
            }
        } else {
            Self {
                typography,
                dark,
                bar: (246, 246, 246),
                panel: (238, 238, 238),
                panel_top: (252, 252, 252),
                edge: (204, 204, 204),
                ink: (22, 22, 22),
                muted: (102, 102, 102),
                accent,
                on_accent: (255, 255, 255),
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
    let surface = unsafe {
        device.CreateSurface(
            width as u32,
            height as u32,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_ALPHA_MODE_PREMULTIPLIED,
        )?
    };
    let rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    paint_region(&surface, palette, rect, dpi, draw)?;
    Ok(surface)
}

/// Clears and repaints a physical pixel rectangle using the full surface's coordinates.
pub(super) fn paint_region(
    surface: &IDCompositionSurface,
    palette: Palette,
    rect: RECT,
    dpi: u32,
    draw: impl FnOnce(&Canvas) -> Result<()>,
) -> Result<()> {
    unsafe {
        let mut origin = POINT::default();
        let ctx: ID2D1DeviceContext = surface.BeginDraw(Some(&rect), &mut origin)?;
        // BeginDraw returns the atlas position of the requested rectangle's top left.
        ctx.SetDpi(96., 96.);
        ctx.SetTextAntialiasMode(if palette.typography.smooth {
            D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE
        } else {
            D2D1_TEXT_ANTIALIAS_MODE_ALIASED
        });
        let bounds = D2D_RECT_F {
            left: origin.x as f32,
            top: origin.y as f32,
            right: (origin.x + (rect.right - rect.left)) as f32,
            bottom: (origin.y + (rect.bottom - rect.top)) as f32,
        };
        ctx.PushAxisAlignedClip(&bounds, D2D1_ANTIALIAS_MODE_ALIASED);
        ctx.Clear(Some(&D2D1_COLOR_F::default()));
        let canvas = Canvas {
            ctx,
            p: palette,
            origin: POINT {
                x: origin.x - rect.left,
                y: origin.y - rect.top,
            },
            scale: dpi as f32 / 96.,
        };
        let result = draw(&canvas);
        canvas.ctx.PopAxisAlignedClip();
        drop(canvas);
        // End the DComp update even when a brush, icon or font fails to load.
        let end = surface.EndDraw();
        result.and(end)
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

    /// A circle outline `width` DIPs thick, centered on `x`, `y`.
    pub(super) fn ring(&self, x: f32, y: f32, radius: f32, width: f32, color: Rgb) -> Result<()> {
        let mut ellipse = D2D1_ELLIPSE {
            radiusX: radius * self.scale,
            radiusY: radius * self.scale,
            ..Default::default()
        };
        ellipse.point.X = self.origin.x as f32 + x * self.scale;
        ellipse.point.Y = self.origin.y as f32 + y * self.scale;
        let brush = self.brush_alpha(color, 1.)?;
        unsafe {
            self.ctx
                .DrawEllipse(&ellipse, &brush, width * self.scale, None)
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

    pub(super) fn bar_glass(&self, w: f32, h: f32, radius: f32) -> Result<()> {
        self.gradient(
            0.,
            0.,
            w,
            h,
            radius,
            (self.p.panel_top, 0.74),
            (self.p.bar, 0.64),
        )
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

    /// Draws `symbol` from DeskIsle's icon set in a `size` DIP square centered on `x`, `y`,
    /// stroked as heavily as [`icon::weight`] gives for that size.
    pub(super) fn symbol(
        &self,
        symbol: Symbol,
        x: f32,
        y: f32,
        size: f32,
        color: Rgb,
        alpha: f32,
    ) -> Result<()> {
        self.symbol_weighted(symbol, x, y, size, icon::weight(size), color, alpha)
    }

    /// Draws `symbol` centered in `area`.
    pub(super) fn symbol_in(
        &self,
        symbol: Symbol,
        area: Area,
        size: f32,
        color: Rgb,
        alpha: f32,
    ) -> Result<()> {
        let (x, y) = ((area.left + area.right) / 2., area.middle());
        self.symbol(symbol, x, y, size, color, alpha)
    }

    /// Draws `symbol` with strokes `weight` DIPs wide, so icons of different sizes that
    /// sit side by side can share one weight.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn symbol_weighted(
        &self,
        symbol: Symbol,
        x: f32,
        y: f32,
        size: f32,
        weight: f32,
        color: Rgb,
        alpha: f32,
    ) -> Result<()> {
        self.draw_shapes(icon::sketch(symbol), x, y, size, weight, color, alpha)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_shapes(
        &self,
        shapes: Vec<icon::Shape>,
        x: f32,
        y: f32,
        size: f32,
        weight: f32,
        color: Rgb,
        alpha: f32,
    ) -> Result<()> {
        let unit = size / icon::GRID * self.scale;
        let left = self.origin.x as f32 + (x - size / 2.) * self.scale;
        let top = self.origin.y as f32 + (y - size / 2.) * self.scale;
        // The point type lives in a crate DeskIsle does not depend on directly.
        let at = |(gx, gy): icon::Point| {
            let mut point = D2D1_ELLIPSE::default().point;
            point.X = left + gx * unit;
            point.Y = top + gy * unit;
            point
        };
        unsafe {
            // Geometry must come from the factory that created the device context.
            let factory = self.ctx.GetFactory()?;
            let style = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.,
                },
                None,
            )?;
            for shape in shapes {
                let geometry = factory.CreatePathGeometry()?;
                let sink = geometry.Open()?;
                let begin = match shape.paint {
                    Paint::Stroke => D2D1_FIGURE_BEGIN_HOLLOW,
                    Paint::Fill => D2D1_FIGURE_BEGIN_FILLED,
                };
                sink.BeginFigure(at(shape.start), begin);
                for step in &shape.steps {
                    match *step {
                        Step::Line(to) => sink.AddLine(at(to)),
                        Step::Quad { control, to } => {
                            sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
                                point1: at(control),
                                point2: at(to),
                            })
                        }
                        Step::Arc {
                            to,
                            radius,
                            clockwise,
                            large,
                        } => sink.AddArc(&D2D1_ARC_SEGMENT {
                            point: at(to),
                            size: D2D_SIZE_F {
                                width: radius * unit,
                                height: radius * unit,
                            },
                            rotationAngle: 0.,
                            sweepDirection: if clockwise {
                                D2D1_SWEEP_DIRECTION_CLOCKWISE
                            } else {
                                D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                            },
                            arcSize: if large {
                                D2D1_ARC_SIZE_LARGE
                            } else {
                                D2D1_ARC_SIZE_SMALL
                            },
                        }),
                    }
                }
                sink.EndFigure(if shape.closed {
                    D2D1_FIGURE_END_CLOSED
                } else {
                    D2D1_FIGURE_END_OPEN
                });
                sink.Close()?;
                let tint = match shape.tone {
                    Tone::Ink => color,
                    Tone::Alert => self.p.danger,
                };
                let brush = self.brush_alpha(tint, alpha * shape.alpha)?;
                match shape.paint {
                    Paint::Stroke => {
                        self.ctx
                            .DrawGeometry(&geometry, &brush, weight * self.scale, &style)
                    }
                    Paint::Fill => self.ctx.FillGeometry(&geometry, &brush, None),
                }
            }
        }
        Ok(())
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
            let format = text_format(factory, self.p.typography, style, self.scale)?;
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
    pub(super) align: DWRITE_TEXT_ALIGNMENT,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            size: 13.,
            bold: false,
            align: DWRITE_TEXT_ALIGNMENT_LEADING,
        }
    }
}

/// The same format for drawing in pixels and measuring in DIPs.
pub(super) fn text_format(
    factory: &IDWriteFactory,
    typography: Typography,
    style: Text,
    scale: f32,
) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = factory.CreateTextFormat(
            [w!("Segoe UI"), w!("Microsoft YaHei UI"), w!("DengXian")][typography.family as usize],
            None,
            if typography.bold {
                DWRITE_FONT_WEIGHT_BOLD
            } else if style.bold {
                DWRITE_FONT_WEIGHT_SEMI_BOLD
            } else {
                DWRITE_FONT_WEIGHT_NORMAL
            },
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            style.size * scale * typography.scale as f32 / 100.,
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
