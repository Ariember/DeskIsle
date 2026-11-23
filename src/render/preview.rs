use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_LEADING;
use windows::core::Result;

use crate::dock::WindowInfo;
use crate::platform::backdrop::Backdrop;
use crate::settings::{AppearanceKey::*, Key::Appearance};
use crate::ui::icon::Symbol;
use crate::ui::panel::Area;
use crate::ui::preview::{PreviewHit, PreviewLayout};

use super::Renderer;
use super::canvas::{Text, paint_pixels};

const CLOSE_RADIUS: f32 = 5.;
const CLOSE_ICON: f32 = 10.5;
const TITLE_SIZE: f32 = 12.;

impl Renderer {
    /// Renders the preview window's chrome (panel background, hovered card highlight,
    /// inter-card dividers, window titles and top-right close buttons) into a BGRA pixel
    /// buffer. DWM composites the live thumbnails over each card's thumbnail rectangle,
    /// which sits strictly below the header bar, and clips the outer corners via
    /// `DWMWCP_ROUND`.
    pub fn paint_preview(
        &self,
        layout: &PreviewLayout,
        windows: &[WindowInfo],
        dpi: u32,
        hover: Option<PreviewHit>,
        backdrop: Option<&Backdrop>,
    ) -> Result<Vec<u8>> {
        let s = dpi as f32 / 96.;
        let dip = |px: i32| px as f32 / s;
        let w = dip(layout.bounds.width);
        let h = dip(layout.bounds.height);
        let custom = self.appearance.enabled(Appearance(WindowCustom));
        let blur_strength = if custom {
            self.appearance.get(Appearance(WindowBlur)) as f32 / 100.
        } else {
            0.70
        };
        let user_opacity = if custom {
            self.appearance.get(Appearance(WindowOpacity)) as f32 / 100.
        } else if self.is_dark() {
            0.24
        } else {
            0.22
        };
        let rim = if custom {
            0.35 + 1.15 * (self.appearance.get(Appearance(WindowBorder)) as f32 / 100.)
        } else {
            1.08
        };
        paint_pixels(
            &self.d2d,
            self.palette,
            layout.bounds.width,
            layout.bounds.height,
            dpi,
            |c| {
                let glass_active = backdrop.is_some() && blur_strength > 0. && user_opacity < 1.;
                let opacity = if glass_active { user_opacity } else { 1. };
                if let Some(image) = backdrop {
                    if glass_active {
                        c.liquid_backdrop(
                            image,
                            Area::new(0., 0., w, h),
                            8.,
                            super::glass::rim::DOCK,
                        )?;
                    } else {
                        c.backdrop(image, Area::new(0., 0., w, h), 0.)?;
                    }
                }
                let (top_tint, bottom_tint) = if c.p.dark {
                    ((38, 46, 62), (18, 22, 32))
                } else {
                    ((252, 254, 255), (235, 243, 252))
                };
                c.liquid_glass_shell(
                    Area::new(0., 0., w, h),
                    8.,
                    super::glass::LiquidShell {
                        top_tint,
                        bottom_tint,
                        opacity,
                        rim,
                        glass_active,
                    },
                )?;
                for (index, card) in layout.cards.iter().enumerate() {
                    let card_hovered = hover.is_some_and(|hit| hit.index() == index);
                    let close_hovered = hover == Some(PreviewHit::Close(index));
                    let (cl, ct, cr, cb) = (
                        dip(card.bounds.x),
                        dip(card.bounds.y),
                        dip(card.bounds.x + card.bounds.width),
                        dip(card.bounds.y + card.bounds.height),
                    );
                    if card_hovered {
                        let hover_tint = if c.p.dark {
                            (215, 232, 255)
                        } else {
                            (255, 255, 255)
                        };
                        c.liquid_pill(
                            Area::new(cl + 2., ct + 2., cr - 2., cb - 2.),
                            7.,
                            hover_tint,
                            if c.p.dark { 0.12 } else { 0.28 },
                            0.38,
                        )?;
                    }

                    let (tl, tt, tr, tb) = (
                        dip(card.thumbnail.x),
                        dip(card.thumbnail.y),
                        dip(card.thumbnail.x + card.thumbnail.width),
                        dip(card.thumbnail.y + card.thumbnail.height),
                    );
                    c.fill(
                        tl,
                        tt,
                        tr,
                        tb,
                        5.,
                        (0, 0, 0),
                        if c.p.dark { 0.30 } else { 0.08 },
                    )?;
                    c.stroke(
                        tl - 0.5,
                        tt - 0.5,
                        tr + 0.5,
                        tb + 0.5,
                        5.5,
                        (255, 255, 255),
                        if c.p.dark { 0.24 } else { 0.45 },
                    )?;

                    if let Some(info) = windows.get(index)
                        && card.title.width > 0
                    {
                        c.draw_text(
                            &self.write,
                            &info.title,
                            c.rect(
                                dip(card.title.x),
                                dip(card.title.y),
                                dip(card.title.x + card.title.width),
                                dip(card.title.y + card.title.height),
                            ),
                            Text {
                                size: TITLE_SIZE,
                                bold: false,
                                align: DWRITE_TEXT_ALIGNMENT_LEADING,
                            },
                            c.p.ink,
                            0.94,
                        )?;
                    }

                    let close = Area::new(
                        dip(card.close.x),
                        dip(card.close.y),
                        dip(card.close.x + card.close.width),
                        dip(card.close.y + card.close.height),
                    );
                    if close_hovered {
                        c.fill(
                            close.left,
                            close.top,
                            close.right,
                            close.bottom,
                            CLOSE_RADIUS,
                            c.p.danger,
                            1.,
                        )?;
                        c.symbol_in(Symbol::Close, close, CLOSE_ICON, c.p.on_accent, 1.)?;
                    } else {
                        if card_hovered {
                            c.fill(
                                close.left,
                                close.top,
                                close.right,
                                close.bottom,
                                CLOSE_RADIUS,
                                c.p.line,
                                0.07,
                            )?;
                        }
                        let (ink, alpha) = if card_hovered {
                            (c.p.ink, 0.92)
                        } else {
                            (c.p.muted, 0.85)
                        };
                        c.symbol_in(Symbol::Close, close, CLOSE_ICON, ink, alpha)?;
                    }
                }
                if let Some(first) = layout.cards.first() {
                    let cw = first.bounds.width;
                    let ch = first.bounds.height;
                    if cw > 0 {
                        let mut x = cw;
                        while x < layout.bounds.width {
                            c.fill(dip(x), 0., dip(x + 1), h, 0., c.p.line, 0.10)?;
                            x += cw;
                        }
                    }
                    if ch > 0 {
                        let mut y = ch;
                        while y < layout.bounds.height {
                            c.fill(0., dip(y), w, dip(y + 1), 0., c.p.line, 0.10)?;
                            y += ch;
                        }
                    }
                }
                Ok(())
            },
        )
    }
}
