use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedApp {
    pub app_id: String,
    pub label: String,
    pub icon_path: PathBuf,
    pub launch: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: isize,
    pub app_id: String,
    pub title: String,
    pub label: String,
    pub icon_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppGroup {
    pub app_id: String,
    pub label: String,
    pub icon_path: PathBuf,
    launch: Option<PathBuf>,
    pub windows: Vec<WindowInfo>,
    pub active: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action<'a> {
    Launch(&'a Path),
    Activate(&'a WindowInfo),
    Choose(&'a [WindowInfo]),
}

impl AppGroup {
    /// Pinned applications stay on the Dock without windows.
    pub fn pinned(&self) -> bool {
        self.launch.is_some()
    }

    /// The executable that starts another instance, when its path is known.
    pub fn executable(&self) -> Option<&Path> {
        self.launch
            .as_deref()
            .or_else(|| Some(self.icon_path.as_path()).filter(|p| !p.as_os_str().is_empty()))
    }

    pub fn action(&self) -> Action<'_> {
        match (self.launch.as_deref(), self.windows.as_slice()) {
            (Some(path), []) => Action::Launch(path),
            (_, [only]) => Action::Activate(only),
            (_, choices) => Action::Choose(choices),
        }
    }
}

pub fn group_apps(pins: &[PinnedApp], windows: &[WindowInfo], foreground: isize) -> Vec<AppGroup> {
    // Stable application order keeps the target under the pointer when the Z order changes.
    let mut running: BTreeMap<&str, Vec<WindowInfo>> = BTreeMap::new();
    for window in windows {
        running
            .entry(&window.app_id)
            .or_default()
            .push(window.clone());
    }
    let mut groups = Vec::new();
    for pin in pins {
        let windows = running.remove(pin.app_id.as_str()).unwrap_or_default();
        groups.push(AppGroup {
            app_id: pin.app_id.clone(),
            label: pin.label.clone(),
            icon_path: pin.icon_path.clone(),
            launch: Some(pin.launch.clone()),
            active: windows.iter().any(|w| w.id == foreground),
            windows,
        });
    }
    for (app_id, windows) in running {
        let representative = &windows[0];
        groups.push(AppGroup {
            app_id: app_id.to_owned(),
            label: representative.label.clone(),
            icon_path: representative.icon_path.clone(),
            launch: None,
            active: windows.iter().any(|w| w.id == foreground),
            windows,
        });
    }
    groups
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    fn contains(self, x: i32, y: i32) -> bool {
        (self.x..self.x + self.width).contains(&x) && (self.y..self.y + self.height).contains(&y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub index: usize,
    /// Client pixels relative to the bottom bar.
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Screen pixels in the target monitor's work area.
    pub bottom: Rect,
    pub slots: Vec<Slot>,
    pub first: usize,
}

fn scaled(dpi: u32) -> impl Fn(i32) -> i32 {
    move |dip: i32| ((i64::from(dip) * i64::from(dpi) + 48) / 96) as i32
}

impl Layout {
    /// `work` is a nonempty work area and `dpi` its effective DPI.
    pub fn new(work: Rect, dpi: u32, count: usize, first: usize) -> Self {
        let px = scaled(dpi);
        let inset_x = px(8).min((work.width - 1) / 2);
        let available = work.width - 2 * inset_x;
        let padding = px(6).min((available - 1) / 2);
        let slot_width = px(52).min(available - 2 * padding);
        let capacity = ((available - 2 * padding) / slot_width) as usize;
        let visible = count.min(capacity);
        let first = first.min(count - visible);

        let width = (visible as i32 * slot_width + 2 * padding).max(1);
        let height = work.height.min(px(64));
        let inset = px(8).min(work.height - height);
        let bottom = Rect {
            x: work.x + (work.width - width) / 2,
            y: work.y + work.height - height - inset,
            width,
            height,
        };
        let inset_y = px(4).min((bottom.height - 1) / 2);
        let slots = (0..visible)
            .map(|offset| Slot {
                index: first + offset,
                rect: Rect {
                    x: padding + offset as i32 * slot_width,
                    y: inset_y,
                    width: slot_width,
                    height: bottom.height - 2 * inset_y,
                },
            })
            .collect();
        Self {
            bottom,
            slots,
            first,
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.slots
            .iter()
            .find(|s| s.rect.contains(x, y))
            .map(|s| s.index)
    }
}

/// Height of the full-width status bar in DIPs.
pub const STATUS_HEIGHT: i32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusHit {
    Tray(usize),
    Overflow,
    /// The active application's name.
    Title,
    Clock,
    /// The mark and empty space.
    Bar,
}

/// Client-pixel layout of the full-width status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLayout {
    pub width: i32,
    pub height: i32,
    pub title: Rect,
    /// `index` refers to the shown tray icons in display order.
    pub tray: Vec<Slot>,
    /// Lists every tray icon, so it appears whenever any icon exists.
    pub overflow: Option<Rect>,
    pub clock: Rect,
}

impl StatusLayout {
    /// `shown` counts the tray icons the user keeps on the bar. Icons that do not fit
    /// stay reachable through the overflow button.
    pub fn new(width: i32, height: i32, dpi: u32, shown: usize, total: usize) -> Self {
        let px = scaled(dpi);
        let width = width.max(1);
        let height = height.max(1);
        let cell = |x, w| Rect {
            x,
            y: 0,
            width: w,
            height,
        };
        let clock_width = px(48).min(width);
        let clock = cell((width - px(10) - clock_width).max(0), clock_width);
        let slot = px(28).max(1);
        let overflow = (total > 0).then(|| cell((clock.x - px(4) - slot).max(0), slot));
        let tray_end = overflow.map_or(clock.x, |r| r.x);
        // Keep room for the mark and the active application's name.
        let title_min = px(220);
        let capacity = ((tray_end - title_min).max(0) / slot) as usize;
        let visible = shown.min(capacity);
        let tray_start = tray_end - visible as i32 * slot;
        let tray = (0..visible)
            .map(|index| Slot {
                index,
                rect: cell(tray_start + index as i32 * slot, slot),
            })
            .collect();
        let title_x = px(34).min(tray_start);
        let title = cell(title_x, (tray_start - px(12) - title_x).max(0));
        Self {
            width,
            height,
            title,
            tray,
            overflow,
            clock,
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> StatusHit {
        if let Some(slot) = self.tray.iter().find(|s| s.rect.contains(x, y)) {
            StatusHit::Tray(slot.index)
        } else if self.overflow.is_some_and(|r| r.contains(x, y)) {
            StatusHit::Overflow
        } else if self.clock.contains(x, y) {
            StatusHit::Clock
        } else if self.title.contains(x, y) {
            StatusHit::Title
        } else {
            StatusHit::Bar
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: isize, app: &str) -> WindowInfo {
        WindowInfo {
            id,
            app_id: app.into(),
            title: format!("{app} {id}"),
            label: app.into(),
            icon_path: app.into(),
        }
    }

    #[test]
    fn launch_switch_and_choose_follow_the_observed_windows() {
        let pins = [PinnedApp {
            app_id: "editor".into(),
            label: "Editor".into(),
            icon_path: "editor.exe".into(),
            launch: "editor.exe".into(),
        }];
        let windows = [
            window(3, "viewer"),
            window(1, "editor"),
            window(2, "editor"),
        ];
        let groups = group_apps(&pins, &windows[..1], 3);
        assert_eq!(groups[0].action(), Action::Launch(Path::new("editor.exe")));
        assert!(groups[1].active);
        let groups = group_apps(&pins, &windows[..2], 1);
        assert_eq!(groups[0].action(), Action::Activate(&windows[1]));
        assert!(groups[0].active);
        assert!(!groups[1].active);
        let groups = group_apps(&pins, &windows, 0);
        assert_eq!(groups[0].action(), Action::Choose(&windows[1..]));
        assert!(!groups.iter().any(|g| g.active));
    }

    #[test]
    fn offset_monitor_keeps_dock_centered_and_hit_edges_half_open() {
        let work = Rect {
            x: -1920,
            y: -150,
            width: 1920,
            height: 1032,
        };
        for dpi in [96, 120, 144, 192] {
            let layout = Layout::new(work, dpi, 4, 0);
            let rect = layout.bottom;
            assert!((2 * (rect.x - work.x) + rect.width - work.width).abs() <= 1);
            assert_eq!(
                work.y + work.height - layout.bottom.y - layout.bottom.height,
                8 * dpi as i32 / 96
            );
            let slot = layout.slots[1].rect;
            assert_eq!(layout.hit(slot.x, slot.y), Some(1));
            assert_eq!(layout.hit(slot.x - 1, slot.y), Some(0));
            assert_eq!(layout.hit(slot.x + slot.width, slot.y), Some(2));
            assert_eq!(layout.hit(slot.x, slot.y + slot.height), None);
        }
    }

    #[test]
    fn overflow_keeps_last_application_reachable_and_clamps_after_close() {
        let work = Rect {
            x: 90,
            y: 15,
            width: 240,
            height: 600,
        };
        let layout = Layout::new(work, 144, 16, usize::MAX);
        let last = layout.slots.last().unwrap().rect;
        assert!(layout.bottom.width <= work.width);
        assert!(layout.first > 0);
        assert_eq!(layout.hit(last.x, last.y), Some(15));
        let layout = Layout::new(work, 144, 1, layout.first);
        assert_eq!(layout.first, 0);
        let first = layout.slots[0].rect;
        assert_eq!(layout.hit(first.x, first.y), Some(0));
    }

    #[test]
    fn status_bar_keeps_title_room_and_overflows_extra_tray_icons() {
        for dpi in [96, 144, 192] {
            let px = scaled(dpi);
            let (width, height) = (px(1920), px(STATUS_HEIGHT));
            let layout = StatusLayout::new(width, height, dpi, 3, 5);
            assert_eq!(layout.tray.len(), 3);
            let overflow = layout.overflow.unwrap();
            let last = layout.tray[2].rect;
            assert!(last.x + last.width <= overflow.x);
            assert!(overflow.x + overflow.width <= layout.clock.x);
            assert!(layout.clock.x + layout.clock.width <= width);
            assert!(layout.title.x + layout.title.width < layout.tray[0].rect.x);
            assert_eq!(layout.hit(last.x, 0), StatusHit::Tray(2));
            assert_eq!(layout.hit(last.x + last.width, 0), StatusHit::Overflow);
            assert_eq!(layout.hit(overflow.x - 1, height - 1), StatusHit::Tray(2));
            assert_eq!(layout.hit(px(20), px(10)), StatusHit::Bar);
            assert_eq!(layout.hit(layout.title.x, px(10)), StatusHit::Title);
            assert_eq!(layout.hit(layout.clock.x, px(10)), StatusHit::Clock);

            // A narrow bar moves icons into the overflow list but keeps the title area.
            let narrow = StatusLayout::new(px(400), height, dpi, 40, 40);
            assert!(narrow.tray.len() < 40);
            assert!(narrow.tray[0].rect.x >= px(220));
            assert!(narrow.overflow.is_some());
        }
        let empty = StatusLayout::new(800, 30, 96, 0, 0);
        assert!(empty.tray.is_empty() && empty.overflow.is_none());
        assert_eq!(empty.hit(700, 10), StatusHit::Title);
        assert_eq!(empty.hit(735, 10), StatusHit::Bar);
    }
}
