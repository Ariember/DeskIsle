use std::path::PathBuf;

use super::panel::Area;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchpadHit {
    Item(usize),
    Page(usize),
    Search,
    Background,
}

pub struct Tile {
    pub index: usize,
    pub name: String,
    pub icon: Area,
    pub label: Area,
    pub hit: Area,
}

/// All areas are client-local DIPs. Only the window and pointer coordinates are pixels.
pub struct LaunchpadLayout {
    pub width: i32,
    pub height: i32,
    pub dpi: u32,
    pub search: Area,
    pub tiles: Vec<Tile>,
    pub dots: Vec<(usize, Area)>,
    pub page: usize,
    pub pages: usize,
}

impl LaunchpadLayout {
    pub fn new(
        width: i32,
        height: i32,
        dpi: u32,
        paths: &[PathBuf],
        query: &str,
        page: usize,
    ) -> Self {
        let scale = dpi as f32 / 96.;
        let (w, h) = (width as f32 / scale, height as f32 / scale);
        let search_width = 216_f32.min(w * 0.8);
        let search_y = (h * 0.026).min(24.);
        let search = Area::new(
            (w - search_width) / 2.,
            search_y,
            (w + search_width) / 2.,
            search_y + 28.,
        );
        let grid_top = search.bottom + (h * 0.035).min(32.);
        let grid_width = (w * 0.84).min(1512.);
        let grid_height = (h - grid_top - 42.).max(1.);
        let columns = ((grid_width / 136.) as usize).clamp(1, 7);
        let rows = ((grid_height / 122.) as usize).max(1);
        let pitch_x = grid_width / columns as f32;
        let pitch_y = (grid_height / rows as f32).min(166.);
        let icon_size = 80_f32.min(pitch_x * 0.6).min((pitch_y - 24.).max(1.));
        let capacity = columns * rows;

        let query = query.trim().to_lowercase();
        let matching: Vec<_> = paths
            .iter()
            .enumerate()
            .filter_map(|(index, path)| {
                let name = path
                    .file_stem()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                (query.is_empty() || name.to_lowercase().contains(&query))
                    .then(|| (index, name.into_owned()))
            })
            .collect();
        let pages = matching.len().div_ceil(capacity).max(1);
        let page = page.min(pages - 1);
        let tiles = matching
            .into_iter()
            .skip(page * capacity)
            .take(capacity)
            .enumerate()
            .map(|(i, (index, name))| {
                let center = (w - grid_width) / 2. + (i % columns) as f32 * pitch_x + pitch_x / 2.;
                let top = grid_top + (i / columns) as f32 * pitch_y;
                let icon = Area::new(
                    center - icon_size / 2.,
                    top,
                    center + icon_size / 2.,
                    top + icon_size,
                );
                let half_label = (pitch_x * 0.48).min(98.);
                let label = Area::new(
                    center - half_label,
                    icon.bottom + 6.,
                    center + half_label,
                    icon.bottom + 27.,
                );
                let hit = Area::new(label.left, icon.top - 4., label.right, label.bottom + 2.);
                Tile {
                    index,
                    name,
                    icon,
                    label,
                    hit,
                }
            })
            .collect();
        let mut dots = Vec::new();
        if pages > 1 {
            let count = pages.min(7);
            let first = page.saturating_sub(count / 2).min(pages - count);
            let left = (w - count as f32 * 20.) / 2.;
            for slot in 0..count {
                let x = left + slot as f32 * 20.;
                dots.push((first + slot, Area::new(x, h - 34., x + 20., h - 10.)));
            }
        }
        Self {
            width,
            height,
            dpi,
            search,
            tiles,
            dots,
            page,
            pages,
        }
    }

    pub fn first_item(&self) -> Option<usize> {
        self.tiles.first().map(|tile| tile.index)
    }

    pub fn hit(&self, x: i32, y: i32) -> LaunchpadHit {
        let scale = self.dpi as f32 / 96.;
        let (x, y) = (x as f32 / scale, y as f32 / scale);
        if self.search.contains(x, y) {
            return LaunchpadHit::Search;
        }
        if let Some((page, _)) = self.dots.iter().find(|(_, area)| area.contains(x, y)) {
            return LaunchpadHit::Page(*page);
        }
        self.tiles
            .iter()
            .find(|tile| tile.hit.contains(x, y))
            .map_or(LaunchpadHit::Background, |tile| {
                LaunchpadHit::Item(tile.index)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_tile_launches_original_entry_at_scaled_coordinates() {
        let paths = ["editor.exe", "Studio Canary.lnk", "studio.exe"].map(PathBuf::from);
        let layout = LaunchpadLayout::new(1440, 900, 144, &paths, "StUdIo", 4);
        assert_eq!(layout.first_item(), Some(1));
        let icon = layout.tiles[1].icon;
        assert_eq!(
            layout.hit(
                ((icon.left + icon.right) * 0.75) as i32,
                (icon.middle() * 1.5) as i32
            ),
            LaunchpadHit::Item(2)
        );
        assert_eq!(layout.hit(0, 0), LaunchpadHit::Background);
        assert_eq!(
            LaunchpadLayout::new(1440, 900, 144, &paths, "missing", 0).first_item(),
            None
        );
    }

    #[test]
    fn page_dots_reach_last_entry_and_page_clamps_after_removal() {
        let paths: Vec<_> = (0..52).map(|i| PathBuf::from(format!("{i}.exe"))).collect();
        let first = LaunchpadLayout::new(800, 600, 96, &paths, "", 0);
        let last = LaunchpadLayout::new(800, 600, 96, &paths, "", usize::MAX);
        let dot = first.dots.last().unwrap().1;
        assert_eq!(
            first.hit(dot.left as i32 + 1, dot.middle() as i32),
            LaunchpadHit::Page(last.page)
        );
        assert_eq!(last.tiles.last().unwrap().index, 51);
        assert_eq!(
            LaunchpadLayout::new(800, 600, 96, &paths[..1], "", last.page).page,
            0
        );
    }
}
