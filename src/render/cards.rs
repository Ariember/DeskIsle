use super::canvas::{Canvas, Palette, Rgb, Text, paint, text_format};
use super::glass::{self, LiquidShell};
use super::{Bar, Renderer};
use crate::cards::{Card, CardBlock, CardState, CardTone};
use crate::geometry::Rect;
use crate::platform::backdrop::{self, Backdrop};
use crate::settings::Key;
use crate::ui::{
    cards::{
        CARD_RADIUS_DIP, CardLayout, RICH_BLOCK_GAP_DIP, RICH_DETAIL_HEIGHT_DIP,
        RICH_DIVIDER_GAP_DIP, RICH_FOOTER_HEIGHT_DIP, RICH_HEADER_END_DIP, RICH_HERO_HEIGHT_DIP,
        RICH_METER_ROW_DIP, RICH_METER_ROW_GAP_DIP, RICH_METRIC_ROW_COMPACT_DIP,
        RICH_METRIC_ROW_GAP_DIP, RICH_METRIC_ROW_TALL_DIP, RICH_SECTION_TITLE_DIP,
        RICH_SEGMENT_BAR_AREA_DIP, RICH_SEGMENT_LEGEND_ROW_DIP,
    },
    icon::Symbol,
    panel::Area,
};
use windows::Win32::{
    Foundation::HWND,
    Graphics::{
        Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP,
        DirectWrite::{
            DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
            DWRITE_WORD_WRAPPING_WRAP,
        },
    },
};
use windows::core::Result;

type PaintKey = (CardLayout, Vec<Card>, u32, Palette, u64, i32, bool);

pub(super) struct CardView {
    hwnd: HWND,
    layer: Bar,
    painted: Option<PaintKey>,
}

fn tone_rgb(tone: CardTone, palette: &Palette, white_surface: bool) -> Rgb {
    if white_surface {
        return palette.accent;
    }
    match (tone, palette.dark) {
        (CardTone::Accent, _) => palette.accent,
        (CardTone::Neutral, true) => (176, 184, 198),
        (CardTone::Neutral, false) => (106, 114, 128),
        (CardTone::Good, true) => (64, 214, 134),
        (CardTone::Good, false) => (22, 152, 82),
        (CardTone::Warn, true) => (255, 186, 58),
        (CardTone::Warn, false) => (196, 118, 12),
        (CardTone::Danger, true) => (255, 112, 106),
        (CardTone::Danger, false) => (210, 48, 42),
        (CardTone::Purple, true) => (178, 134, 255),
        (CardTone::Purple, false) => (126, 70, 206),
        (CardTone::Teal, true) => (64, 208, 216),
        (CardTone::Teal, false) => (14, 142, 156),
    }
}

fn lighten((r, g, b): Rgb, amount: f32) -> Rgb {
    let mix = |c: u8| (f32::from(c) + (255. - f32::from(c)) * amount).round() as u8;
    (mix(r), mix(g), mix(b))
}

fn flat_box(c: &Canvas, area: Area, radius: f32, background: Rgb) -> Result<()> {
    let (l, t, r, b) = (area.left, area.top, area.right, area.bottom);
    c.fill(l, t, r, b, radius, background, 1.)?;
    c.stroke(l, t, r, b, radius, (219, 219, 219), 1.)
}

fn estimate_pill_width(text: &str) -> f32 {
    let units: f32 = text
        .chars()
        .map(|c| if c.is_ascii() { 6.2 } else { 10.6 })
        .sum();
    (units + 18.).clamp(54., 126.)
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
        let stamp = if self.transparency {
            backdrop::wallpaper_stamp()
        } else {
            0
        };
        let strength = self.appearance.get(Key::Backdrop);
        let view = self.cards.as_ref().unwrap();
        if view.painted.as_ref().is_some_and(|key| {
            key.0 == *layout
                && key.1 == visible
                && key.2 == dpi
                && key.3 == self.palette
                && key.4 == stamp
                && key.5 == strength
                && key.6 == self.transparency
        }) {
            return Ok(());
        }

        let nav_on_white = layout.navigation.is_some_and(|nav| {
            layout.tiles.iter().zip(visible).any(|(tile, card)| {
                card.white_surface && nav.y >= tile.y && nav.y < tile.y + tile.height
            })
        });
        let tile_samples: Vec<Option<Backdrop>> = layout
            .tiles
            .iter()
            .zip(visible)
            .map(|(tile, card)| {
                if !self.transparency || card.white_surface {
                    return None;
                }
                Backdrop::wallpaper(Rect {
                    x: layout.bounds.x + tile.x,
                    y: layout.bounds.y + tile.y,
                    width: tile.width,
                    height: tile.height,
                })
                .ok()
            })
            .collect();
        let nav_sample = layout.navigation.and_then(|nav| {
            if !self.transparency || nav_on_white {
                return None;
            }
            Backdrop::wallpaper(Rect {
                x: layout.bounds.x + nav.x,
                y: layout.bounds.y + nav.y,
                width: nav.width,
                height: nav.height,
            })
            .ok()
        });

        let surface = paint(
            &self.device,
            self.palette,
            layout.bounds.width,
            layout.bounds.height,
            dpi,
            |c| {
                let w = layout.bounds.width as f32 / c.scale;
                for ((tile, card), sample) in
                    layout.tiles.iter().zip(visible).zip(tile_samples.iter())
                {
                    let nav_overlap = layout
                        .navigation
                        .filter(|n| n.y >= tile.y && n.y < tile.y + tile.height)
                        .map(|n| n.x as f32 / c.scale - 6.);
                    self.paint_card_tile(c, *tile, card, sample.as_ref(), strength, nav_overlap)?;
                }
                if let Some(nav) = layout.navigation {
                    self.draw_card_navigation(
                        c,
                        layout,
                        nav,
                        w,
                        nav_sample.as_ref(),
                        nav_on_white,
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
        view.painted = Some((
            layout.clone(),
            visible.to_vec(),
            dpi,
            self.palette,
            stamp,
            strength,
            self.transparency,
        ));
        Ok(())
    }

    fn paint_card_tile(
        &self,
        c: &Canvas,
        tile: Rect,
        card: &Card,
        sample: Option<&Backdrop>,
        strength: i32,
        nav_overlap: Option<f32>,
    ) -> Result<()> {
        let y = tile.y as f32 / c.scale;
        let w = tile.width as f32 / c.scale;
        let h = tile.height as f32 / c.scale;
        let area = Area::new(0., y, w, y + h);
        let palette = if card.white_surface {
            self.paint_white_shell(c, area, CARD_RADIUS_DIP)?
        } else {
            self.paint_card_shell(c, area, CARD_RADIUS_DIP, sample, strength, card.state)?
        };
        if card.is_rich() {
            self.draw_rich_card(c, &palette, card, y, w, h, nav_overlap)
        } else {
            self.draw_simple_card(c, &palette, card, y, w, h, nav_overlap)
        }
    }

    fn paint_white_shell(&self, c: &Canvas, area: Area, radius: f32) -> Result<Palette> {
        flat_box(c, area, radius, (255, 255, 255))?;
        Ok(Palette {
            dark: false,
            ink: (28, 28, 28),
            muted: (96, 96, 96),
            accent: (66, 66, 66),
            danger: (50, 50, 50),
            line: (40, 40, 40),
            ..self.palette
        })
    }

    fn paint_card_shell(
        &self,
        c: &Canvas,
        area: Area,
        radius: f32,
        sample: Option<&Backdrop>,
        strength: i32,
        state: CardState,
    ) -> Result<Palette> {
        let glass_active = self.transparency && sample.is_some();
        let light_ground = sample
            .filter(|_| glass_active)
            .and_then(glass::light_ground);
        let tile_p = match light_ground {
            Some(light) => Palette::new(!light, &self.appearance).on_glass(),
            None if glass_active => self.palette.on_glass(),
            None => self.palette,
        };
        let state_tone = match state {
            CardState::Ready => tile_p.accent,
            CardState::Setup => tile_p.muted,
            CardState::Error => tile_p.danger,
        };

        if let Some(image) = sample.filter(|_| glass_active) {
            c.liquid_backdrop(
                image,
                area,
                radius,
                glass::rim::PANEL,
                (strength as f32 / 100.).max(0.55),
            )?;
            let (scrim_top, scrim_bot) = if tile_p.dark {
                (((22, 26, 36), 0.46), ((12, 15, 22), 0.56))
            } else {
                (((255, 255, 255), 0.48), ((240, 245, 252), 0.36))
            };
            c.gradient(
                area.left,
                area.top,
                area.right,
                area.bottom,
                radius,
                scrim_top,
                scrim_bot,
            )?;
            c.clipped_round(area.left, area.top, area.right, area.bottom, radius, || {
                c.gradient(
                    area.left,
                    area.top,
                    area.right,
                    (area.top + 48.).min(area.bottom),
                    0.,
                    (state_tone, if tile_p.dark { 0.10 } else { 0.06 }),
                    (state_tone, 0.),
                )
            })?;
            c.stroke(
                area.left,
                area.top,
                area.right,
                area.bottom,
                radius,
                (255, 255, 255),
                if tile_p.dark { 0.22 } else { 0.55 },
            )?;
        } else {
            let (top_tint, bottom_tint) = if tile_p.dark {
                ((38, 43, 54), (24, 28, 36))
            } else {
                ((254, 255, 255), (236, 242, 250))
            };
            c.liquid_glass_shell(
                area,
                radius,
                LiquidShell {
                    top_tint,
                    bottom_tint,
                    opacity: 0.95,
                    rim: 0.90,
                    glass_active: false,
                },
            )?;
            c.clipped_round(area.left, area.top, area.right, area.bottom, radius, || {
                c.gradient(
                    area.left,
                    area.top,
                    area.right,
                    (area.top + 44.).min(area.bottom),
                    0.,
                    (state_tone, if tile_p.dark { 0.08 } else { 0.05 }),
                    (state_tone, 0.),
                )
            })?;
        }
        Ok(tile_p)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_card_header(
        &self,
        c: &Canvas,
        p: &Palette,
        card: &Card,
        y: f32,
        w: f32,
        nav_overlap: Option<f32>,
    ) -> Result<()> {
        let (icon, tone) = match card.state {
            CardState::Ready => (Symbol::Activity, p.accent),
            CardState::Setup => (Symbol::Settings, p.muted),
            CardState::Error => (Symbol::Info, p.danger),
        };
        let icon_area = Area::new(15., y + 13., 41., y + 39.);
        if card.white_surface {
            flat_box(c, icon_area, 8.5, (246, 246, 246))?;
        } else {
            c.liquid_pill(
                icon_area,
                8.5,
                tone,
                if p.dark { 0.24 } else { 0.16 },
                if p.dark { 0.34 } else { 0.28 },
            )?;
        }
        c.symbol(icon, 28., y + 26., 14.5, tone, 1.)?;

        let mut title_end = nav_overlap.unwrap_or(w - 15.);
        if !card.badge.is_empty() && nav_overlap.is_none() {
            let pill_w = estimate_pill_width(&card.badge).min((w - 130.).max(54.));
            let pill_left = w - 15. - pill_w;
            let pill_area = Area::new(pill_left, y + 16., w - 15., y + 36.);
            if card.white_surface {
                flat_box(c, pill_area, 10., (250, 250, 250))?;
            } else {
                let pill_tint = if p.dark { (255, 255, 255) } else { p.accent };
                c.liquid_pill(
                    pill_area,
                    10.,
                    pill_tint,
                    if p.dark { 0.12 } else { 0.11 },
                    if p.dark { 0.22 } else { 0.24 },
                )?;
            }
            c.draw_text(
                &self.write,
                &card.badge,
                c.rect(pill_left + 6., y + 15.5, w - 21., y + 36.5),
                Text {
                    size: 10.5,
                    bold: true,
                    align: DWRITE_TEXT_ALIGNMENT_CENTER,
                },
                if p.dark { p.ink } else { p.accent },
                0.92,
            )?;
            title_end = (pill_left - 8.).max(60.);
        }

        c.draw_text(
            &self.write,
            &card.title,
            c.rect(49., y + 10., title_end, y + 42.),
            Text {
                size: 13.5,
                bold: true,
                ..Default::default()
            },
            p.ink,
            1.,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_simple_card(
        &self,
        c: &Canvas,
        p: &Palette,
        card: &Card,
        y: f32,
        w: f32,
        h: f32,
        nav_overlap: Option<f32>,
    ) -> Result<()> {
        self.draw_card_header(c, p, card, y, w, nav_overlap)?;
        c.draw_text(
            &self.write,
            &card.value,
            c.rect(16., y + 43., w - 16., y + 88.),
            Text {
                size: if card.state == CardState::Ready {
                    29.
                } else {
                    22.
                },
                bold: true,
                ..Default::default()
            },
            p.ink,
            1.,
        )?;
        self.card_detail(
            c,
            p,
            &card.detail,
            c.rect(17., y + 86., w - 16., y + h - 9.),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_rich_card(
        &self,
        c: &Canvas,
        p: &Palette,
        card: &Card,
        y: f32,
        w: f32,
        h: f32,
        nav_overlap: Option<f32>,
    ) -> Result<()> {
        self.draw_card_header(c, p, card, y, w, nav_overlap)?;

        let left = 16.;
        let right = w - 16.;
        let mut cursor = y + RICH_HEADER_END_DIP as f32;

        c.draw_text(
            &self.write,
            &card.value,
            c.rect(left, cursor, right, cursor + RICH_HERO_HEIGHT_DIP as f32),
            Text {
                size: if card.state == CardState::Ready {
                    28.
                } else {
                    22.
                },
                bold: true,
                ..Default::default()
            },
            p.ink,
            1.,
        )?;
        cursor += RICH_HERO_HEIGHT_DIP as f32;

        if !card.detail.is_empty() {
            c.draw_text(
                &self.write,
                &card.detail,
                c.rect(left, cursor, right, cursor + RICH_DETAIL_HEIGHT_DIP as f32),
                Text {
                    size: 11.5,
                    ..Default::default()
                },
                p.muted,
                0.96,
            )?;
            cursor += RICH_DETAIL_HEIGHT_DIP as f32;
        }

        if !card.blocks.is_empty() {
            let line_y = cursor + 5.;
            c.fill(left, line_y, right, line_y + 1. / c.scale, 0., p.line, 0.11)?;
            cursor += RICH_DIVIDER_GAP_DIP as f32;

            for (idx, block) in card.blocks.iter().enumerate() {
                if idx > 0 {
                    cursor += RICH_BLOCK_GAP_DIP as f32;
                }
                cursor =
                    self.draw_card_block(c, p, block, (left, right), cursor, card.white_surface)?;
            }
        }

        if !card.footer.is_empty() {
            let line_y = cursor + 5.;
            c.fill(left, line_y, right, line_y + 1. / c.scale, 0., p.line, 0.10)?;
            c.draw_text(
                &self.write,
                &card.footer,
                c.rect(
                    left,
                    cursor + 7.,
                    right,
                    (cursor + RICH_FOOTER_HEIGHT_DIP as f32).min(y + h - 4.),
                ),
                Text {
                    size: 10.5,
                    ..Default::default()
                },
                p.muted,
                0.90,
            )?;
        }
        Ok(())
    }

    fn draw_card_block(
        &self,
        c: &Canvas,
        p: &Palette,
        block: &CardBlock,
        (left, right): (f32, f32),
        mut cursor: f32,
        white_surface: bool,
    ) -> Result<f32> {
        let inner_w = right - left;
        match block {
            CardBlock::Metrics { title, items } => {
                if !title.is_empty() {
                    c.draw_text(
                        &self.write,
                        title,
                        c.rect(left, cursor, right, cursor + RICH_SECTION_TITLE_DIP as f32),
                        Text {
                            size: 10.5,
                            bold: true,
                            ..Default::default()
                        },
                        p.muted,
                        0.90,
                    )?;
                    cursor += (RICH_SECTION_TITLE_DIP + 4) as f32;
                }
                let col_gap = 8.;
                let rows = items.len().div_ceil(2);
                for row in 0..rows {
                    if row > 0 {
                        cursor += RICH_METRIC_ROW_GAP_DIP as f32;
                    }
                    let start = row * 2;
                    let end = (start + 2).min(items.len());
                    let row_items = &items[start..end];
                    let has_sub = row_items.iter().any(|item| !item.sub.is_empty());
                    let row_h = if has_sub {
                        RICH_METRIC_ROW_TALL_DIP as f32
                    } else {
                        RICH_METRIC_ROW_COMPACT_DIP as f32
                    };
                    let cols = row_items.len() as f32;
                    let cell_w = (inner_w - col_gap * (cols - 1.)) / cols;

                    for (col, item) in row_items.iter().enumerate() {
                        let x0 = left + col as f32 * (cell_w + col_gap);
                        let x1 = x0 + cell_w;
                        let dot_color = tone_rgb(item.tone, p, white_surface);
                        let cell = Area::new(x0, cursor, x1, cursor + row_h);
                        if white_surface {
                            flat_box(c, cell, 11., (248, 248, 248))?;
                        } else {
                            c.liquid_pill(
                                cell,
                                11.,
                                (255, 255, 255),
                                if p.dark { 0.08 } else { 0.50 },
                                if p.dark { 0.16 } else { 0.42 },
                            )?;
                        }
                        c.fill(
                            x1 - 15.,
                            cursor + 9.5,
                            x1 - 9.5,
                            cursor + 15.,
                            2.75,
                            dot_color,
                            0.95,
                        )?;
                        c.draw_text(
                            &self.write,
                            &item.label,
                            c.rect(x0 + 10., cursor + 5., x1 - 19., cursor + 19.),
                            Text {
                                size: 10.5,
                                ..Default::default()
                            },
                            p.muted,
                            0.95,
                        )?;
                        c.draw_text(
                            &self.write,
                            &item.value,
                            c.rect(x0 + 10., cursor + 17., x1 - 10., cursor + 38.),
                            Text {
                                size: 16.5,
                                bold: true,
                                ..Default::default()
                            },
                            p.ink,
                            1.,
                        )?;
                        if !item.sub.is_empty() {
                            c.draw_text(
                                &self.write,
                                &item.sub,
                                c.rect(x0 + 10., cursor + 37., x1 - 10., cursor + row_h - 3.),
                                Text {
                                    size: 9.8,
                                    ..Default::default()
                                },
                                p.muted,
                                0.92,
                            )?;
                        }
                    }
                    cursor += row_h;
                }
            }
            CardBlock::Segments { title, note, items } => {
                if !title.is_empty() || !note.is_empty() {
                    let mid = if note.is_empty() {
                        right
                    } else {
                        left + inner_w * 0.58
                    };
                    if !title.is_empty() {
                        c.draw_text(
                            &self.write,
                            title,
                            c.rect(left, cursor, mid, cursor + RICH_SECTION_TITLE_DIP as f32),
                            Text {
                                size: 10.5,
                                bold: true,
                                ..Default::default()
                            },
                            p.muted,
                            0.90,
                        )?;
                    }
                    if !note.is_empty() {
                        c.draw_text(
                            &self.write,
                            note,
                            c.rect(mid, cursor, right, cursor + RICH_SECTION_TITLE_DIP as f32),
                            Text {
                                size: 10.,
                                align: DWRITE_TEXT_ALIGNMENT_TRAILING,
                                ..Default::default()
                            },
                            p.muted,
                            0.88,
                        )?;
                    }
                    cursor += (RICH_SECTION_TITLE_DIP + 2) as f32;
                }

                let bar_y = cursor + 4.;
                let bar_h = 8.;
                let bar_r = bar_h / 2.;
                c.fill(
                    left,
                    bar_y,
                    right,
                    bar_y + bar_h,
                    bar_r,
                    p.line,
                    if p.dark { 0.16 } else { 0.10 },
                )?;
                let active: Vec<_> = items.iter().filter(|it| it.share > 0).collect();
                let sum: f32 = active.iter().map(|it| f32::from(it.share)).sum();
                if sum > 0. {
                    let gap = 1.5;
                    let total_gap = gap * (active.len().saturating_sub(1) as f32);
                    let usable = (inner_w - total_gap).max(1.);
                    let mut widths: Vec<f32> = active
                        .iter()
                        .map(|it| (usable * f32::from(it.share) / sum).max(5.))
                        .collect();
                    let w_sum: f32 = widths.iter().sum();
                    if w_sum > usable && w_sum > 0. {
                        let scale_w = usable / w_sum;
                        for w in &mut widths {
                            *w *= scale_w;
                        }
                    }
                    c.clipped_round(left, bar_y, right, bar_y + bar_h, bar_r, || {
                        let mut x = left;
                        for (item, seg_w) in active.iter().zip(widths) {
                            let rgb = tone_rgb(item.tone, p, white_surface);
                            c.gradient(
                                x,
                                bar_y,
                                x + seg_w,
                                bar_y + bar_h,
                                2.,
                                (lighten(rgb, 0.18), 0.98),
                                (rgb, 0.98),
                            )?;
                            x += seg_w + gap;
                        }
                        Ok(())
                    })?;
                }
                cursor += RICH_SEGMENT_BAR_AREA_DIP as f32;

                let legend_rows = items.len().div_ceil(3);
                for row in 0..legend_rows {
                    let start = row * 3;
                    let end = (start + 3).min(items.len());
                    let slice = &items[start..end];
                    let cols = slice.len() as f32;
                    let col_w = inner_w / cols;
                    for (col, item) in slice.iter().enumerate() {
                        let x0 = left + col as f32 * col_w;
                        let x1 = x0 + col_w - 4.;
                        let dot_color = tone_rgb(item.tone, p, white_surface);
                        c.fill(x0, cursor + 5., x0 + 6., cursor + 11., 3., dot_color, 0.98)?;
                        let label = if item.value.is_empty() {
                            format!("{} {}%", item.label, item.share)
                        } else {
                            format!("{} {}", item.label, item.value)
                        };
                        c.draw_text(
                            &self.write,
                            &label,
                            c.rect(
                                x0 + 9.,
                                cursor,
                                x1,
                                cursor + RICH_SEGMENT_LEGEND_ROW_DIP as f32,
                            ),
                            Text {
                                size: 10.2,
                                ..Default::default()
                            },
                            p.ink,
                            0.88,
                        )?;
                    }
                    cursor += RICH_SEGMENT_LEGEND_ROW_DIP as f32;
                }
            }
            CardBlock::Meters { title, items } => {
                if !title.is_empty() {
                    c.draw_text(
                        &self.write,
                        title,
                        c.rect(left, cursor, right, cursor + RICH_SECTION_TITLE_DIP as f32),
                        Text {
                            size: 10.5,
                            bold: true,
                            ..Default::default()
                        },
                        p.muted,
                        0.90,
                    )?;
                    cursor += (RICH_SECTION_TITLE_DIP + 4) as f32;
                }
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        cursor += RICH_METER_ROW_GAP_DIP as f32;
                    }
                    let rgb = tone_rgb(item.tone, p, white_surface);
                    c.fill(left, cursor + 5.5, left + 6., cursor + 11.5, 3., rgb, 0.98)?;
                    let val_w = 60.;
                    let detail_w = if item.detail.is_empty() { 0. } else { 96. };
                    let label_right = (right - val_w - detail_w - 6.).max(left + 72.);
                    c.draw_text(
                        &self.write,
                        &item.label,
                        c.rect(left + 10., cursor, label_right, cursor + 17.),
                        Text {
                            size: 11.5,
                            bold: true,
                            align: DWRITE_TEXT_ALIGNMENT_LEADING,
                        },
                        p.ink,
                        0.98,
                    )?;
                    if !item.detail.is_empty() {
                        c.draw_text(
                            &self.write,
                            &item.detail,
                            c.rect(label_right + 2., cursor, right - val_w - 4., cursor + 17.),
                            Text {
                                size: 10.,
                                align: DWRITE_TEXT_ALIGNMENT_TRAILING,
                                ..Default::default()
                            },
                            p.muted,
                            0.90,
                        )?;
                    }
                    c.draw_text(
                        &self.write,
                        &item.value,
                        c.rect(right - val_w, cursor, right, cursor + 17.),
                        Text {
                            size: 11.5,
                            bold: true,
                            align: DWRITE_TEXT_ALIGNMENT_TRAILING,
                        },
                        rgb,
                        1.,
                    )?;

                    let bar_y = cursor + 20.5;
                    let bar_h = 6.5;
                    let bar_r = bar_h / 2.;
                    c.fill(
                        left,
                        bar_y,
                        right,
                        bar_y + bar_h,
                        bar_r,
                        p.line,
                        if p.dark { 0.16 } else { 0.10 },
                    )?;
                    if item.percent > 0 {
                        let fill_w = (inner_w * f32::from(item.percent) / 100.)
                            .max(bar_h)
                            .min(inner_w);
                        c.gradient(
                            left,
                            bar_y,
                            left + fill_w,
                            bar_y + bar_h,
                            bar_r,
                            (lighten(rgb, 0.20), 0.98),
                            (rgb, 0.98),
                        )?;
                    }
                    cursor += RICH_METER_ROW_DIP as f32;
                }
            }
        }
        Ok(cursor)
    }

    fn draw_card_navigation(
        &self,
        c: &Canvas,
        layout: &CardLayout,
        nav: Rect,
        w: f32,
        sample: Option<&Backdrop>,
        white_surface: bool,
    ) -> Result<()> {
        let x = nav.x as f32 / c.scale;
        let y = nav.y as f32 / c.scale;
        let h = nav.height as f32 / c.scale;
        let area = Area::new(x, y, w, y + h);
        let nav_p = if white_surface {
            self.paint_white_shell(c, area, h / 2.)?
        } else {
            self.paint_card_shell(
                c,
                area,
                h / 2.,
                sample,
                self.appearance.get(Key::Backdrop),
                CardState::Ready,
            )?
        };
        let mid = y + h / 2.;
        c.symbol(
            Symbol::ChevronLeft,
            x + 16.,
            mid,
            13.,
            nav_p.ink,
            if layout.page > 0 { 1. } else { 0.35 },
        )?;
        c.symbol(
            Symbol::ChevronRight,
            w - 16.,
            mid,
            13.,
            nav_p.ink,
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
                bold: true,
                align: DWRITE_TEXT_ALIGNMENT_CENTER,
            },
            nav_p.ink,
            1.,
        )
    }

    fn card_detail(
        &self,
        c: &Canvas,
        p: &Palette,
        text: &str,
        rect: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
    ) -> Result<()> {
        let format = text_format(
            &self.write,
            p.typography,
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
                &c.brush_alpha(p.muted, 1.)?,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod white_surface_tests;
