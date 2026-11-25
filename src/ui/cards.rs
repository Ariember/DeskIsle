use crate::cards::{Card, CardBlock};
use crate::geometry::Rect;
use std::ops::Range;

pub(crate) const CARD_WIDTH_DIP: i32 = 320;
pub(crate) const CARD_RADIUS_DIP: f32 = 18.;
pub(crate) const SIMPLE_CARD_HEIGHT_DIP: i32 = 132;

pub(crate) const RICH_HEADER_END_DIP: i32 = 44;
pub(crate) const RICH_HERO_HEIGHT_DIP: i32 = 34;
pub(crate) const RICH_DETAIL_HEIGHT_DIP: i32 = 17;
pub(crate) const RICH_DIVIDER_GAP_DIP: i32 = 11;
pub(crate) const RICH_BLOCK_GAP_DIP: i32 = 10;
pub(crate) const RICH_SECTION_TITLE_DIP: i32 = 16;
pub(crate) const RICH_METRIC_ROW_TALL_DIP: i32 = 54;
pub(crate) const RICH_METRIC_ROW_COMPACT_DIP: i32 = 42;
pub(crate) const RICH_METRIC_ROW_GAP_DIP: i32 = 8;
pub(crate) const RICH_SEGMENT_BAR_AREA_DIP: i32 = 17;
pub(crate) const RICH_SEGMENT_LEGEND_ROW_DIP: i32 = 16;
pub(crate) const RICH_METER_ROW_DIP: i32 = 29;
pub(crate) const RICH_METER_ROW_GAP_DIP: i32 = 7;
pub(crate) const RICH_FOOTER_HEIGHT_DIP: i32 = 24;
pub(crate) const RICH_BOTTOM_PAD_DIP: i32 = 14;

pub(crate) fn block_height_dip(block: &CardBlock) -> i32 {
    match block {
        CardBlock::Metrics { title, items } => {
            let mut h = if title.is_empty() {
                0
            } else {
                RICH_SECTION_TITLE_DIP + 4
            };
            let rows = items.len().div_ceil(2);
            for row in 0..rows {
                if row > 0 {
                    h += RICH_METRIC_ROW_GAP_DIP;
                }
                let start = row * 2;
                let end = (start + 2).min(items.len());
                let has_sub = items[start..end].iter().any(|item| !item.sub.is_empty());
                h += if has_sub {
                    RICH_METRIC_ROW_TALL_DIP
                } else {
                    RICH_METRIC_ROW_COMPACT_DIP
                };
            }
            h
        }
        CardBlock::Segments { title, note, items } => {
            let header = if title.is_empty() && note.is_empty() {
                0
            } else {
                RICH_SECTION_TITLE_DIP + 2
            };
            let legend_rows = items.len().div_ceil(3) as i32;
            header + RICH_SEGMENT_BAR_AREA_DIP + legend_rows * RICH_SEGMENT_LEGEND_ROW_DIP
        }
        CardBlock::Meters { title, items } => {
            let header = if title.is_empty() {
                0
            } else {
                RICH_SECTION_TITLE_DIP + 4
            };
            let count = items.len() as i32;
            header + count * RICH_METER_ROW_DIP + (count - 1).max(0) * RICH_METER_ROW_GAP_DIP
        }
    }
}

pub(crate) fn card_height_dip(card: &Card) -> i32 {
    if !card.is_rich() {
        return SIMPLE_CARD_HEIGHT_DIP;
    }
    let mut h = RICH_HEADER_END_DIP + RICH_HERO_HEIGHT_DIP;
    if !card.detail.is_empty() {
        h += RICH_DETAIL_HEIGHT_DIP;
    }
    if !card.blocks.is_empty() {
        h += RICH_DIVIDER_GAP_DIP;
        for (idx, block) in card.blocks.iter().enumerate() {
            if idx > 0 {
                h += RICH_BLOCK_GAP_DIP;
            }
            h += block_height_dip(block);
        }
    }
    if !card.footer.is_empty() {
        h += RICH_FOOTER_HEIGHT_DIP;
    }
    h + RICH_BOTTOM_PAD_DIP
}

/// Positions in client pixels, apart from the window bounds in screen pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CardLayout {
    pub bounds: Rect,
    pub tiles: Vec<Rect>,
    pub items: Range<usize>,
    pub navigation: Option<Rect>,
    pub page: usize,
    pub pages: usize,
}

impl CardLayout {
    pub fn new(work: Rect, dpi: u32, cards: &[Card], page: usize) -> Option<Self> {
        let px = |dip: i32| (dip as f32 * dpi as f32 / 96.).round() as i32;
        let margin = px(18);
        let gap = px(10);
        let width = px(CARD_WIDTH_DIP).min(work.width - 2 * margin);
        let height = work.height - 2 * margin;
        if cards.is_empty() || width <= 0 || height < px(SIMPLE_CARD_HEIGHT_DIP) {
            return None;
        }
        let heights: Vec<i32> = cards
            .iter()
            .map(|card| px(card_height_dip(card)).min(height))
            .collect();
        let total_height = heights.iter().sum::<i32>() + gap * (heights.len() as i32 - 1);
        let paged = total_height > height;
        let nav_height = px(32);
        let embedded = paged && height < heights[0] + nav_height + gap;
        let header = if paged && !embedded {
            nav_height + gap
        } else {
            0
        };
        let available = (height - header).max(1);

        let mut pages_ranges = Vec::new();
        let mut cursor = 0;
        while cursor < heights.len() {
            let start = cursor;
            let mut used = heights[cursor].min(available);
            cursor += 1;
            while cursor < heights.len() {
                let next = used + gap + heights[cursor].min(available);
                if next > available {
                    break;
                }
                used = next;
                cursor += 1;
            }
            pages_ranges.push(start..cursor);
        }

        let pages = pages_ranges.len();
        let page = page.min(pages - 1);
        let items = pages_ranges[page].clone();
        let mut y = header;
        let mut tiles = Vec::with_capacity(items.len());
        for &tile_height in &heights[items.clone()] {
            let h = tile_height.min(available);
            tiles.push(Rect {
                x: 0,
                y,
                width,
                height: h,
            });
            y += h + gap;
        }
        let navigation = paged.then(|| {
            let width_nav = px(108).min(width);
            Rect {
                x: width - width_nav,
                y: if embedded { px(3) } else { 0 },
                width: width_nav,
                height: nav_height,
            }
        });
        let last = tiles.last()?;
        let bounds = Rect {
            x: work.x + work.width - margin - width,
            y: work.y + margin,
            width,
            height: last.y + last.height,
        };
        Some(Self {
            bounds,
            tiles,
            items,
            navigation,
            page,
            pages,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{CardState, CardTone, MeterItem, MetricItem, SegmentItem};

    #[test]
    fn pages_reach_every_plugin_and_clamp_after_removal() {
        let work = Rect {
            x: -1600,
            y: 48,
            width: 1600,
            height: 1200,
        };
        let cards: Vec<_> = (0..21)
            .map(|i| Card::simple(format!("Card {i}"), "42", "", CardState::Ready))
            .collect();
        for dpi in [96, 120, 144, 192] {
            let first = CardLayout::new(work, dpi, &cards, 0).unwrap();
            let mut shown = Vec::new();
            for page in 0..first.pages {
                let view = CardLayout::new(work, dpi, &cards, page).unwrap();
                assert!(
                    view.bounds.x >= work.x
                        && view.bounds.y + view.bounds.height <= work.y + work.height
                );
                assert!(view.navigation.is_some());
                shown.extend(view.items);
            }
            assert_eq!(shown, (0..21).collect::<Vec<_>>());
            let last = CardLayout::new(work, dpi, &cards[..1], 10).unwrap();
            assert_eq!(last.items, 0..1);
            assert!(last.navigation.is_none());
        }
    }

    #[test]
    fn rich_card_grows_to_fit_blocks_and_pages_cleanly() {
        let rich = Card {
            title: "Magpie 数据面板".into(),
            value: "$158.1".into(),
            detail: "4251 次请求 · 34 会话".into(),
            state: CardState::Ready,
            badge: "近7日 · 10:55".into(),
            blocks: vec![
                CardBlock::Metrics {
                    title: "TOKEN".into(),
                    items: vec![
                        MetricItem {
                            label: "总 Token".into(),
                            value: "267M".into(),
                            sub: "入 846K · 出 1.4M".into(),
                            tone: CardTone::Accent,
                        },
                        MetricItem {
                            label: "缓存命中".into(),
                            value: "93%".into(),
                            sub: "读 248M · 写 17M".into(),
                            tone: CardTone::Good,
                        },
                    ],
                },
                CardBlock::Segments {
                    title: "模型占比".into(),
                    note: "3 个模型".into(),
                    items: vec![SegmentItem {
                        label: "opus-5-5".into(),
                        share: 92,
                        value: "92%".into(),
                        tone: CardTone::Accent,
                    }],
                },
                CardBlock::Meters {
                    title: "订阅配额".into(),
                    items: vec![MeterItem {
                        label: "Codex".into(),
                        value: "剩 6%".into(),
                        detail: "14分重置".into(),
                        percent: 6,
                        tone: CardTone::Danger,
                    }],
                },
            ],
            footer: String::new(),
        };
        assert!(card_height_dip(&rich) > SIMPLE_CARD_HEIGHT_DIP);
        let work = Rect {
            x: 0,
            y: 30,
            width: 1920,
            height: 1050,
        };
        let layout = CardLayout::new(work, 96, &[rich], 0).unwrap();
        assert!(layout.navigation.is_none());
        assert_eq!(layout.tiles.len(), 1);
        assert_eq!(layout.bounds.height, layout.tiles[0].height);
    }
}
