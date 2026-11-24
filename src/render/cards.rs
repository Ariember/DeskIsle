use super::canvas::{Canvas, Palette, Text, paint, text_format};
use super::{Bar, Renderer};
use crate::cards::{Card, CardState};
use crate::ui::{cards::CardLayout, icon::Symbol};
use windows::Win32::{
    Foundation::HWND,
    Graphics::{
        Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP,
        DirectWrite::{
            DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_WORD_WRAPPING_WRAP,
        },
    },
};
use windows::core::Result;

pub(super) struct CardView {
    hwnd: HWND,
    layer: Bar,
    painted: Option<(CardLayout, Vec<Card>, u32, Palette)>,
}

impl Renderer {
    pub(crate) fn draw_cards(
        &mut self,
        hwnd: HWND,
        layout: &CardLayout,
        visible: &[Card],
        dpi: u32,
    ) -> Result<()> {
        if self.cards.as_ref().is_none_or(|v| v.hwnd != hwnd) {
            self.cards = Some(CardView {
                hwnd,
                layer: Bar::new(&self.device, hwnd)?,
                painted: None,
            });
        }
        let view = self.cards.as_ref().unwrap();
        if view.painted.as_ref().is_some_and(|(l, c, d, p)| {
            l == layout && c == visible && *d == dpi && *p == self.palette
        }) {
            return Ok(());
        }
        let surface = paint(
            &self.device,
            self.palette,
            layout.bounds.width,
            layout.bounds.height,
            dpi,
            |c| {
                let w = layout.bounds.width as f32 / c.scale;
                let (top, bottom) = if c.p.dark {
                    ((44, 48, 55), (30, 33, 39))
                } else {
                    ((255, 255, 255), (239, 242, 247))
                };
                for (tile, card) in layout.tiles.iter().zip(visible) {
                    let y = tile.y as f32 / c.scale;
                    let h = tile.height as f32 / c.scale;
                    c.gradient(0., y, w, y + h, 15., (top, 0.99), (bottom, 0.98))?;
                    c.stroke(0., y, w, y + h, 15., c.p.edge, 0.50)?;
                    let (icon, tone) = match card.state {
                        CardState::Ready => (Symbol::Activity, c.p.accent),
                        CardState::Setup => (Symbol::Settings, c.p.muted),
                        CardState::Error => (Symbol::Info, c.p.danger),
                    };
                    c.fill(16., y + 13., 43., y + 40., 8., tone, 0.13)?;
                    c.symbol(icon, 29.5, y + 26.5, 15., tone, 1.)?;
                    let title_end = layout
                        .navigation
                        .filter(|n| n.y >= tile.y && n.y < tile.y + tile.height)
                        .map_or(w - 16., |n| n.x as f32 / c.scale - 6.);
                    c.draw_text(
                        &self.write,
                        &card.title,
                        c.rect(52., y + 8., title_end, y + 45.),
                        Text {
                            size: 14.,
                            bold: true,
                            ..Default::default()
                        },
                        c.p.ink,
                        1.,
                    )?;
                    c.draw_text(
                        &self.write,
                        &card.value,
                        c.rect(16., y + 42., w - 16., y + 89.),
                        Text {
                            size: if card.state == CardState::Ready {
                                30.
                            } else {
                                23.
                            },
                            bold: true,
                            ..Default::default()
                        },
                        c.p.ink,
                        1.,
                    )?;
                    self.card_detail(c, &card.detail, c.rect(17., y + 87., w - 16., y + h - 8.))?;
                }
                if let Some(nav) = layout.navigation {
                    let x = nav.x as f32 / c.scale;
                    let y = nav.y as f32 / c.scale;
                    let h = nav.height as f32 / c.scale;
                    c.round(x, y, w, y + h, h / 2., bottom)?;
                    c.stroke(x, y, w, y + h, h / 2., c.p.edge, 0.6)?;
                    let mid = y + h / 2.;
                    c.symbol(
                        Symbol::ChevronLeft,
                        x + 16.,
                        mid,
                        13.,
                        c.p.ink,
                        if layout.page > 0 { 1. } else { 0.35 },
                    )?;
                    c.symbol(
                        Symbol::ChevronRight,
                        w - 16.,
                        mid,
                        13.,
                        c.p.ink,
                        if layout.page + 1 < layout.pages {
                            1.
                        } else {
                            0.35
                        },
                    )?;
                    c.draw_text(
                        &self.write,
                        &format!("{} / {}", layout.page + 1, layout.pages),
                        c.rect(x + 24., y, w - 24., y + h),
                        Text {
                            size: 12.,
                            align: DWRITE_TEXT_ALIGNMENT_CENTER,
                            ..Default::default()
                        },
                        c.p.ink,
                        1.,
                    )?;
                }
                Ok(())
            },
        )?;
        let view = self.cards.as_mut().unwrap();
        unsafe {
            view.layer.root.SetContent(&surface)?;
            self.device.Commit()?;
        }
        view.painted = Some((layout.clone(), visible.to_vec(), dpi, self.palette));
        Ok(())
    }

    fn card_detail(
        &self,
        c: &Canvas,
        text: &str,
        rect: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
    ) -> Result<()> {
        let format = text_format(
            &self.write,
            c.p.typography,
            Text {
                size: 12.,
                ..Default::default()
            },
            c.scale,
        )?;
        unsafe {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            c.ctx.DrawText(
                &text.encode_utf16().collect::<Vec<_>>(),
                &format,
                &rect,
                &c.brush_alpha(c.p.muted, 1.)?,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }
}
