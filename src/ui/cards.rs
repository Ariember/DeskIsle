use crate::geometry::Rect;
use std::ops::Range;

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
    pub fn new(work: Rect, dpi: u32, count: usize, page: usize) -> Option<Self> {
        let px = |dip: i32| (dip as f32 * dpi as f32 / 96.).round() as i32;
        let margin = px(18);
        let gap = px(10);
        let tile = px(132);
        let width = px(304).min(work.width - 2 * margin);
        let height = work.height - 2 * margin;
        if count == 0 || width <= 0 || tile == 0 || height < tile {
            return None;
        }
        let full_rows = ((height + gap) / (tile + gap)) as usize;
        let nav_height = px(32);
        // A short work area keeps the page buttons inside the first card's header.
        let embedded = height < tile + nav_height + gap;
        let paged = count > full_rows;
        let header = if paged && !embedded {
            nav_height + gap
        } else {
            0
        };
        let capacity = ((height - header + gap) / (tile + gap)).max(1) as usize;
        let pages = count.div_ceil(capacity);
        let page = page.min(pages - 1);
        let start = page * capacity;
        let items = start..start.saturating_add(capacity).min(count);
        let tiles: Vec<_> = (0..items.len())
            .map(|i| Rect {
                x: 0,
                y: header + (i as i32) * (tile + gap),
                width,
                height: tile,
            })
            .collect();
        let navigation = paged.then(|| {
            let width_nav = px(108).min(width);
            Rect {
                x: width - width_nav,
                y: if embedded { px(3) } else { 0 },
                width: width_nav,
                height: px(32),
            }
        });
        let bounds = Rect {
            x: work.x + work.width - margin - width,
            y: work.y + margin,
            width,
            height: tiles.last()?.y + tile,
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

    #[test]
    fn pages_reach_every_plugin_and_clamp_after_removal() {
        let work = Rect {
            x: -1600,
            y: 48,
            width: 1600,
            height: 1200,
        };
        for dpi in [96, 120, 144, 192] {
            let first = CardLayout::new(work, dpi, 21, 0).unwrap();
            let mut shown = Vec::new();
            for page in 0..first.pages {
                let view = CardLayout::new(work, dpi, 21, page).unwrap();
                assert!(
                    view.bounds.x >= work.x
                        && view.bounds.y + view.bounds.height <= work.y + work.height
                );
                assert!(view.navigation.is_some());
                shown.extend(view.items);
            }
            assert_eq!(shown, (0..21).collect::<Vec<_>>());
            let last = CardLayout::new(work, dpi, 1, 10).unwrap();
            assert_eq!(last.items, 0..1);
            assert!(last.navigation.is_none());
        }
    }
}
