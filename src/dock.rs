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
    pub top: Rect,
    pub bottom: Rect,
    pub slots: Vec<Slot>,
    pub first: usize,
}

impl Layout {
    /// `work` is a nonempty work area and `dpi` its effective DPI.
    pub fn new(work: Rect, dpi: u32, count: usize, first: usize) -> Self {
        let px = |dip: i32| ((i64::from(dip) * i64::from(dpi) + 48) / 96) as i32;
        let inset_x = px(8).min((work.width - 1) / 2);
        let available = work.width - 2 * inset_x;
        let padding = px(6).min((available - 1) / 2);
        let slot_width = px(52).min(available - 2 * padding);
        let capacity = ((available - 2 * padding) / slot_width) as usize;
        let visible = count.min(capacity);
        let first = first.min(count - visible);

        let bar = |width, height, at_top| {
            let height = work.height.min(height);
            let inset = px(8).min(work.height - height);
            Rect {
                x: work.x + (work.width - width) / 2,
                y: work.y
                    + if at_top {
                        inset
                    } else {
                        work.height - height - inset
                    },
                width,
                height,
            }
        };
        let top = bar(available.min(px(360)), px(36), true);
        let bottom = bar(
            (visible as i32 * slot_width + 2 * padding).max(1),
            px(64),
            false,
        );
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
            top,
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
    fn offset_monitor_keeps_bars_centered_and_hit_edges_half_open() {
        let work = Rect {
            x: -1920,
            y: -150,
            width: 1920,
            height: 1032,
        };
        for dpi in [96, 120, 144, 192] {
            let layout = Layout::new(work, dpi, 4, 0);
            for rect in [layout.top, layout.bottom] {
                assert!((2 * (rect.x - work.x) + rect.width - work.width).abs() <= 1);
            }
            assert_eq!(layout.top.y - work.y, 8 * dpi as i32 / 96);
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
}
