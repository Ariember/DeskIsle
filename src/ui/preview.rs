use crate::geometry::Rect;

pub struct PreviewLayout {
    /// Screen pixels above the Dock.
    pub bounds: Rect,
    /// Client pixels, in the same order as the source windows.
    pub cells: Vec<Rect>,
}

impl PreviewLayout {
    pub fn new(center_x: i32, dock_top: i32, work: Rect, dpi: u32, count: usize) -> Option<Self> {
        let count = i32::try_from(count).ok().filter(|&count| count > 0)?;
        let px = |dip: i32| ((i64::from(dip) * i64::from(dpi) + 48) / 96) as i32;
        let gap = px(8);
        let bottom = (dock_top - gap).min(work.y + work.height);
        let available_height = bottom - work.y;
        let preferred_width = px(224);

        // Pack 16:9 cells at their preferred size where possible. When the group
        // wraps, choose the arrangement with the largest thumbnails.
        let max_columns = count.min((work.width - gap) / (gap + 1));
        let (width, columns, rows) = (1..=max_columns)
            .filter_map(|columns| {
                let rows = 1 + (count - 1) / columns;
                let cell_width = (work.width - gap) / columns - gap;
                let cell_height = (available_height - gap) / rows - gap;
                if cell_height <= 0 {
                    return None;
                }
                let width = preferred_width.min(cell_width).min(cell_height * 16 / 9);
                Some((width, columns, rows))
            })
            .max()?;
        let height = (width * 9 / 16).max(1);
        let bounds_width = gap + columns * (width + gap);
        let bounds_height = gap + rows * (height + gap);
        let bounds = Rect {
            x: (center_x - bounds_width / 2).clamp(work.x, work.x + work.width - bounds_width),
            y: bottom - bounds_height,
            width: bounds_width,
            height: bounds_height,
        };
        let cells = (0..count)
            .map(|index| Rect {
                x: gap + (index % columns) * (width + gap),
                y: gap + (index / columns) * (height + gap),
                width,
                height,
            })
            .collect();
        Some(Self { bounds, cells })
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
            assert_eq!(center.cells[0].width, 224 * dpi as i32 / 96);
            let left = PreviewLayout::new(-790, 525, WORK, dpi, 1).unwrap();
            let right = PreviewLayout::new(-10, 525, WORK, dpi, 1).unwrap();
            assert_eq!(left.bounds.x, WORK.x);
            assert_eq!(right.bounds.x + right.bounds.width, WORK.x + WORK.width);
        }
    }

    #[test]
    fn all_windows_fit_in_distinct_cells_when_a_group_wraps() {
        let layout = PreviewLayout::new(-200, 525, WORK, 144, 12).unwrap();
        assert_eq!(layout.cells.len(), 12);
        assert!(layout.bounds.y >= WORK.y);
        assert!(layout.cells.last().unwrap().y > layout.cells[0].y);
        for (i, cell) in layout.cells.iter().enumerate() {
            assert!(cell.x >= 0 && cell.x + cell.width <= layout.bounds.width);
            assert!(cell.y >= 0 && cell.y + cell.height <= layout.bounds.height);
            assert!(
                layout.cells[..i].iter().all(
                    |other| other.y + other.height <= cell.y || other.x + other.width <= cell.x
                )
            );
        }
        assert!(PreviewLayout::new(-200, 525, WORK, 144, 0).is_none());
        assert!(PreviewLayout::new(-200, WORK.y, WORK, 144, 1).is_none());
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
