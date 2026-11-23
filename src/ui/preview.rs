use crate::geometry::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewHit {
    Window(usize),
    Close(usize),
}

impl PreviewHit {
    pub fn index(self) -> usize {
        match self {
            Self::Window(index) | Self::Close(index) => index,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewCard {
    /// Whole card area in client pixels.
    pub bounds: Rect,
    /// Window title text area in client pixels.
    pub title: Rect,
    /// Top-right close button hit area in client pixels.
    pub close: Rect,
    /// Area reserved for the live DWM thumbnail in client pixels, below the header.
    pub thumbnail: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewLayout {
    /// Screen pixels above the Dock.
    pub bounds: Rect,
    /// Client-pixel cards, in the same order as the source windows.
    pub cards: Vec<PreviewCard>,
}

/// Header bar height at the top of each preview card, in DIPs.
pub const HEADER_HEIGHT: i32 = 26;
/// Size of the top-right close button in each preview card, in DIPs.
pub const CLOSE_SIZE: i32 = 22;

impl PreviewLayout {
    pub fn new(center_x: i32, dock_top: i32, work: Rect, dpi: u32, count: usize) -> Option<Self> {
        let count = i32::try_from(count).ok().filter(|&count| count > 0)?;
        let px = |dip: i32| ((i64::from(dip) * i64::from(dpi) + 48) / 96) as i32;
        let dock_gap = px(8);
        let header = px(HEADER_HEIGHT).max(1);
        let card_pad = px(6);
        let close_size = px(CLOSE_SIZE).max(1);
        let title_inset = px(2);
        let title_gap = px(4);
        let bottom = (dock_top - dock_gap).min(work.y + work.height);
        let available_height = bottom - work.y;
        let preferred_width = px(224);

        // Pack 16:9 thumbnails below their title headers at their preferred size where
        // possible. Cards sit flush against the preview window edges without an outer
        // frame; when the group wraps, choose the arrangement with the largest thumbnails.
        let max_columns = count.min(work.width.max(0));
        let (thumb_width, columns, rows) = (1..=max_columns)
            .filter_map(|columns| {
                let rows = 1 + (count - 1) / columns;
                let cell_width = work.width / columns;
                let cell_height = available_height / rows;
                let avail_thumb_w = cell_width - 2 * card_pad;
                let avail_thumb_h = cell_height - header - card_pad;
                if avail_thumb_w <= 0 || avail_thumb_h <= 0 {
                    return None;
                }
                let width = preferred_width
                    .min(avail_thumb_w)
                    .min(avail_thumb_h * 16 / 9);
                (width > 0).then_some((width, columns, rows))
            })
            .max()?;
        let thumb_height = (thumb_width * 9 / 16).max(1);
        let card_width = thumb_width + 2 * card_pad;
        let card_height = header + thumb_height + card_pad;
        let bounds_width = columns * card_width;
        let bounds_height = rows * card_height;
        let bounds = Rect {
            x: (center_x - bounds_width / 2).clamp(work.x, work.x + work.width - bounds_width),
            y: bottom - bounds_height,
            width: bounds_width,
            height: bounds_height,
        };
        let cards = (0..count)
            .map(|index| {
                let x = (index % columns) * card_width;
                let y = (index / columns) * card_height;
                let card_bounds = Rect {
                    x,
                    y,
                    width: card_width,
                    height: card_height,
                };
                let close_w = close_size.min(card_width);
                let close_h = close_size.min(header);
                let close_margin = ((header - close_h) / 2).min(card_pad);
                let close = Rect {
                    x: (x + card_width - close_margin - close_w).max(x),
                    y: y + (header - close_h) / 2,
                    width: close_w,
                    height: close_h,
                };
                let title_x = (x + card_pad + title_inset).min(close.x);
                let title = Rect {
                    x: title_x,
                    y,
                    width: (close.x - title_gap - title_x).max(0),
                    height: header,
                };
                let thumbnail = Rect {
                    x: x + card_pad,
                    y: y + header,
                    width: thumb_width,
                    height: thumb_height,
                };
                PreviewCard {
                    bounds: card_bounds,
                    title,
                    close,
                    thumbnail,
                }
            })
            .collect();
        Some(Self { bounds, cards })
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<PreviewHit> {
        self.cards.iter().enumerate().find_map(|(index, card)| {
            if card.close.contains(x, y) {
                Some(PreviewHit::Close(index))
            } else if card.bounds.contains(x, y) {
                Some(PreviewHit::Window(index))
            } else {
                None
            }
        })
    }

    /// Screen rectangle connecting the preview panel to its Dock icon slot, covering
    /// both the air gap above the Dock and the Dock's top rim above the slot.
    pub fn bridge(&self, slot_screen: Rect) -> Rect {
        let left = self.bounds.x.min(slot_screen.x);
        let right = (self.bounds.x + self.bounds.width).max(slot_screen.x + slot_screen.width);
        let top = self.bounds.y + self.bounds.height;
        let bottom = slot_screen.y.max(top);
        Rect {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        }
    }
}

/// The source dimensions and the destination cell are nonempty.
pub fn fit_thumbnail(cell: Rect, source_width: i32, source_height: i32) -> Rect {
    let fit =
        |space, dimension, other| (i64::from(space) * i64::from(dimension)) / i64::from(other);
    let width =
        fit(cell.height, source_width, source_height).clamp(1, i64::from(cell.width)) as i32;
    let height =
        fit(cell.width, source_height, source_width).clamp(1, i64::from(cell.height)) as i32;
    Rect {
        x: cell.x + (cell.width - width) / 2,
        y: cell.y + (cell.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: -800,
        y: 40,
        width: 800,
        height: 550,
    };

    #[test]
    fn single_preview_centers_over_its_icon_and_stays_inside_the_screen_edges() {
        for dpi in [96, 144, 192] {
            let center = PreviewLayout::new(-400, 525, WORK, dpi, 1).unwrap();
            assert_eq!(center.bounds.x + center.bounds.width / 2, -400);
            assert_eq!(
                center.bounds.y + center.bounds.height,
                525 - 8 * dpi as i32 / 96
            );
            let card = center.cards[0];
            assert_eq!(
                card.bounds,
                Rect {
                    x: 0,
                    y: 0,
                    width: center.bounds.width,
                    height: center.bounds.height,
                }
            );
            assert_eq!(card.thumbnail.width, 224 * dpi as i32 / 96);
            assert!(card.close.y + card.close.height <= card.thumbnail.y);
            assert!(card.title.x + card.title.width <= card.close.x);
            let left = PreviewLayout::new(-790, 525, WORK, dpi, 1).unwrap();
            let right = PreviewLayout::new(-10, 525, WORK, dpi, 1).unwrap();
            assert_eq!(left.bounds.x, WORK.x);
            assert_eq!(right.bounds.x + right.bounds.width, WORK.x + WORK.width);
        }
    }

    #[test]
    fn all_windows_fit_in_distinct_cards_when_a_group_wraps() {
        let layout = PreviewLayout::new(-200, 525, WORK, 144, 12).unwrap();
        assert_eq!(layout.cards.len(), 12);
        assert!(layout.bounds.y >= WORK.y);
        assert!(layout.cards.last().unwrap().bounds.y > layout.cards[0].bounds.y);
        for (i, card) in layout.cards.iter().enumerate() {
            let b = card.bounds;
            assert!(b.x >= 0 && b.x + b.width <= layout.bounds.width);
            assert!(b.y >= 0 && b.y + b.height <= layout.bounds.height);
            assert!(card.close.x >= b.x && card.close.x + card.close.width <= b.x + b.width);
            assert!(card.close.y >= b.y && card.close.y + card.close.height <= card.thumbnail.y);
            assert!(layout.cards[..i].iter().all(|other| {
                other.bounds.y + other.bounds.height <= b.y
                    || other.bounds.x + other.bounds.width <= b.x
            }));
        }
        assert!(PreviewLayout::new(-200, 525, WORK, 144, 0).is_none());
        assert!(PreviewLayout::new(-200, WORK.y, WORK, 144, 1).is_none());
    }

    #[test]
    fn hit_testing_distinguishes_close_button_card_body_and_empty_cells() {
        let layout = PreviewLayout::new(-400, 525, WORK, 144, 2).unwrap();
        // Cards sit flush against the preview window edges with no dead outer margin.
        assert_eq!(layout.hit(0, 0), Some(PreviewHit::Window(0)));
        assert_eq!(
            layout.hit(layout.bounds.width - 1, layout.bounds.height - 1),
            Some(PreviewHit::Window(1))
        );
        assert_eq!(layout.hit(-1, 0), None);
        assert_eq!(layout.hit(layout.bounds.width, 0), None);
        for (i, card) in layout.cards.iter().enumerate() {
            assert_eq!(
                layout.hit(
                    card.close.x + card.close.width / 2,
                    card.close.y + card.close.height / 2,
                ),
                Some(PreviewHit::Close(i))
            );
            assert_eq!(
                layout.hit(
                    card.thumbnail.x + card.thumbnail.width / 2,
                    card.thumbnail.y + card.thumbnail.height / 2,
                ),
                Some(PreviewHit::Window(i))
            );
            assert_eq!(
                layout.hit(
                    card.title.x + card.title.width / 2,
                    card.title.y + card.title.height / 2,
                ),
                Some(PreviewHit::Window(i))
            );
        }
        // A wrapped group with an incomplete last row leaves its trailing cell empty.
        let wrapped = PreviewLayout::new(-400, 525, WORK, 144, 3).unwrap();
        assert_eq!(
            wrapped.hit(wrapped.bounds.width - 1, wrapped.bounds.height - 1),
            None
        );
    }

    #[test]
    fn bridge_spans_between_preview_bottom_and_dock_slot_top() {
        let layout = PreviewLayout::new(-400, 525, WORK, 96, 1).unwrap();
        let slot = Rect {
            x: -426,
            y: 529,
            width: 52,
            height: 56,
        };
        let bridge = layout.bridge(slot);
        assert_eq!(bridge.y, layout.bounds.y + layout.bounds.height);
        assert_eq!(bridge.y + bridge.height, slot.y);
        // Covers both the 8px gap above the Dock and the 4px top rim inside the Dock.
        assert!(bridge.contains(-400, 524));
        assert!(bridge.contains(-400, 526));
        assert!(!bridge.contains(-400, 529));
        assert!(!bridge.contains(-400, layout.bounds.y + layout.bounds.height - 1));
    }

    #[test]
    fn portrait_and_wide_sources_are_centered_without_stretching() {
        let cell = Rect {
            x: 20,
            y: 30,
            width: 200,
            height: 100,
        };
        assert_eq!(
            fit_thumbnail(cell, 400, 800),
            Rect {
                x: 95,
                width: 50,
                ..cell
            }
        );
        assert_eq!(
            fit_thumbnail(cell, 1200, 300),
            Rect {
                y: 55,
                height: 50,
                ..cell
            }
        );
    }
}
