use crate::geometry::Rect;
use crate::metrics::{Indicator, Style};
use crate::visibility::DockDisplayMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub index: usize,
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockHit {
    Launchpad,
    App(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockDrop {
    Launchpad,
    /// Insert at this boundary in the full list of apps, including scrolled-out apps.
    Insert(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DockSlot {
    pub target: DockHit,
    /// Client pixels relative to the bottom bar.
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Screen pixels in the target monitor's work area.
    pub bottom: Rect,
    /// The last row of work-area pixels, including both corners.
    work_edge: Rect,
    pub slots: Vec<DockSlot>,
    pub first: usize,
    /// Edge of each icon in pixels.
    pub icon: i32,
}

/// The Dock's adjustable sizes in DIPs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DockStyle {
    pub icon: i32,
    pub spacing: i32,
    /// Distance from the bottom of the work area.
    pub gap: i32,
}

impl Default for DockStyle {
    fn default() -> Self {
        Self {
            icon: 34,
            spacing: 18,
            gap: 8,
        }
    }
}

fn scaled(dpi: u32) -> impl Fn(i32) -> i32 {
    move |dip: i32| ((i64::from(dip) * i64::from(dpi) + 48) / 96) as i32
}

impl Layout {
    /// `work` is a nonempty work area and `dpi` its effective DPI.
    pub fn new(work: Rect, dpi: u32, apps: usize, first: usize, style: DockStyle) -> Self {
        let px = scaled(dpi);
        let inset_x = px(8).min((work.width - 1) / 2);
        let available = work.width - 2 * inset_x;
        let padding = px(6).min((available - 1) / 2);
        let slot_width = px(style.icon + style.spacing)
            .min(available - 2 * padding)
            .max(1);
        let capacity = ((available - 2 * padding) / slot_width) as usize;
        let count = apps + 1;
        let visible = count.min(capacity);
        let first = first.min(count - visible);

        let width = (visible as i32 * slot_width + 2 * padding).max(1);
        let height = work.height.min(px(style.icon + 30));
        let inset = px(style.gap).min(work.height - height);
        let bottom = Rect {
            x: work.x + (work.width - width) / 2,
            y: work.y + work.height - height - inset,
            width,
            height,
        };
        let inset_y = px(4).min((bottom.height - 1) / 2);
        let slots = (0..visible)
            .map(|offset| {
                // Keep the launchpad in front while apps scroll. On a one-slot display
                // it scrolls with the apps so every target stays reachable.
                let index = if offset == 0 && capacity > 1 {
                    0
                } else {
                    first + offset
                };
                let target = match index {
                    0 => DockHit::Launchpad,
                    i => DockHit::App(i - 1),
                };
                DockSlot {
                    target,
                    rect: Rect {
                        x: padding + offset as i32 * slot_width,
                        y: inset_y,
                        width: slot_width,
                        height: bottom.height - 2 * inset_y,
                    },
                }
            })
            .collect();
        let icon = px(style.icon)
            .min(slot_width)
            .min(bottom.height - 2 * inset_y)
            .max(1);
        Self {
            bottom,
            work_edge: Rect {
                y: work.y + work.height - 1,
                height: 1,
                ..work
            },
            slots,
            first,
            icon,
        }
    }

    pub fn reveal_edge(&self, mode: DockDisplayMode) -> Rect {
        if mode == DockDisplayMode::AutoHide {
            self.work_edge
        } else {
            Rect {
                x: self.bottom.x,
                width: self.bottom.width,
                ..self.work_edge
            }
        }
    }

    /// Connect the visible Dock to the edge so the pointer can cross the gap.
    pub fn revealed_window(&self) -> Rect {
        Rect {
            height: self.work_edge.y + 1 - self.bottom.y,
            ..self.bottom
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<DockHit> {
        self.slots
            .iter()
            .find(|s| s.rect.contains(x, y))
            .map(|s| s.target)
    }

    fn insertion_gap(&self, at: usize) -> usize {
        self.slots
            .iter()
            .position(|slot| matches!(slot.target, DockHit::App(index) if index >= at))
            .unwrap_or(self.slots.len())
    }

    /// Divide the current row into equal slots, including one for the incoming app.
    /// Keeping the HWND in place also keeps OLE's drag-enter/leave boundaries stable.
    fn insertion_rect(&self, position: usize) -> Rect {
        let first = self.slots[0].rect;
        let count = self.slots.len() as i64;
        let width = count * i64::from(first.width);
        let edge = |i: usize| first.x + (i as i64 * width / (count + 1)) as i32;
        let x = edge(position);
        Rect {
            x,
            width: edge(position + 1) - x,
            ..first
        }
    }

    fn insertion_slots(&self, gap: usize) -> impl Iterator<Item = DockSlot> + '_ {
        self.slots
            .iter()
            .enumerate()
            .map(move |(i, slot)| DockSlot {
                rect: self.insertion_rect(i + usize::from(i >= gap)),
                target: slot.target,
            })
    }

    /// Visible icon positions and their resting scale while an external app is inserted.
    pub fn insertion_preview(&self, at: usize) -> (Vec<DockSlot>, f32) {
        let slots = self.insertion_slots(self.insertion_gap(at)).collect();
        let pitch = self.insertion_rect(0).width;
        // Leave some space between icons even in a short Dock with large icons.
        let size = (pitch - pitch / 10).clamp(1, self.icon);
        (slots, size as f32 / self.icon as f32)
    }

    /// Hit the incoming file's visible gap and limit insertion to the pinned apps.
    pub fn file_drop_target(
        &self,
        x: i32,
        y: i32,
        current: Option<DockDrop>,
        pins: usize,
    ) -> Option<DockDrop> {
        if !(0..self.bottom.width).contains(&x) || !(0..self.bottom.height).contains(&y) {
            return None;
        }
        let target = if let Some(DockDrop::Insert(at)) = current {
            let gap = self.insertion_gap(at);
            let space = self.insertion_rect(gap);
            if (space.x..space.x + space.width).contains(&x) {
                DockDrop::Insert(at)
            } else {
                Self::drop_among(self.insertion_slots(gap), x, y, None)
            }
        } else {
            Self::drop_among(self.slots.iter().copied(), x, y, None)
        };
        Some(match target {
            DockDrop::Insert(at) => DockDrop::Insert(at.min(pins)),
            DockDrop::Launchpad => DockDrop::Launchpad,
        })
    }

    /// `source` is the current group index of an app being moved within the Dock.
    pub fn drop_target(&self, x: i32, y: i32, source: Option<usize>) -> Option<DockDrop> {
        if !(0..self.bottom.width).contains(&x) || !(0..self.bottom.height).contains(&y) {
            return None;
        }
        Some(Self::drop_among(self.slots.iter().copied(), x, y, source))
    }

    fn drop_among(
        slots: impl Iterator<Item = DockSlot>,
        x: i32,
        y: i32,
        source: Option<usize>,
    ) -> DockDrop {
        let mut boundary = 0;
        for slot in slots {
            if slot.rect.contains(x, y) {
                match (slot.target, source) {
                    (DockHit::Launchpad, _) => return DockDrop::Launchpad,
                    (DockHit::App(index), Some(from)) => {
                        return DockDrop::Insert(index + usize::from(from < index));
                    }
                    _ => {}
                }
            }
            if let DockHit::App(index) = slot.target {
                if x < slot.rect.x + slot.rect.width / 2 {
                    return DockDrop::Insert(index);
                }
                boundary = index + 1;
            }
        }
        DockDrop::Insert(boundary)
    }
}

/// A system control on the status bar, between the tray and the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemItem {
    InputMethod,
    InputLanguage,
    Search,
    Network,
    Display,
    Battery,
    Sound,
    /// Quick toggles and sliders for several features at once.
    Control,
}

impl SystemItem {
    /// Width of the control's slot on the status bar in DIPs. The battery also shows
    /// its charge beside the glyph.
    pub fn slot_width(self) -> i32 {
        match self {
            Self::Battery => 60,
            _ => 30,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusHit {
    CloseMaximized,
    Tray(usize),
    Overflow,
    System(SystemItem),
    /// The active application's name.
    Title,
    Clock,
    /// The DeskIsle mark at the leading edge.
    Mark,
    /// Empty space.
    Bar,
}

/// Client-pixel layout of the full-width status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLayout {
    pub width: i32,
    pub height: i32,
    /// The DeskIsle mark, before the title.
    pub mark: Rect,
    pub title: Rect,
    /// `index` refers to the shown tray icons in display order.
    pub tray: Vec<Slot>,
    /// Lists every tray icon, so it appears whenever any icon exists.
    pub overflow: Option<Rect>,
    /// Visible monitoring indicators, in the same order as the settings.
    pub metrics: Vec<Rect>,
    /// System controls in display order.
    pub system: Vec<(SystemItem, Rect)>,
    pub clock: Rect,
    pub close: Option<Rect>,
}

impl StatusLayout {
    /// `shown` counts the tray icons the user keeps on the bar. Icons that do not fit
    /// stay reachable through the overflow button.
    /// `system` lists the system controls to show, in display order; they always fit
    /// before tray icons do.
    pub fn new(
        size: (i32, i32),
        dpi: u32,
        shown: usize,
        total: usize,
        system: &[SystemItem],
        metrics: &[Indicator],
        close_maximized: bool,
    ) -> Self {
        let px = scaled(dpi);
        let width = size.0.max(1);
        let height = size.1.max(1);
        let cell = |x, w| Rect {
            x,
            y: 0,
            width: w,
            height,
        };
        let close_width = px(28).min(width);
        let close = close_maximized.then(|| cell(width - close_width, close_width));
        let clock_end = close.map_or(width - px(10), |r| r.x).max(0);
        let clock_width = px(48).min(clock_end);
        let clock = cell(clock_end - clock_width, clock_width);
        let slot = px(28).max(1);
        let widths = system
            .iter()
            .map(|&item| px(item.slot_width()).max(1))
            .collect::<Vec<_>>();
        let system_start = (clock.x - px(4) - widths.iter().sum::<i32>()).max(0);
        let mut next = system_start;
        let system = system
            .iter()
            .zip(widths)
            .map(|(&item, w)| {
                let rect = cell(next, w);
                next += w;
                (item, rect)
            })
            .collect::<Vec<_>>();
        // Reserve the mark, some title space and the overflow button before adding
        // indicators. Keep complete cells so a clipped value never reads incorrectly.
        let metrics_room = system_start - px(100) - if total > 0 { slot } else { 0 };
        let mut metrics_width = 0;
        let mut metric_slots = Vec::new();
        for indicator in metrics {
            let w = px(match indicator.style {
                // A three-digit temperature or percentage at the largest font setting
                // needs up to 43 DIPs of text, plus the 25 DIPs around it.
                Style::Value => 70,
                Style::Graph => 56,
                Style::Bars | Style::Ring => 44,
            });
            if metrics_width + w > metrics_room {
                break;
            }
            metric_slots.push(cell(metrics_width, w));
            metrics_width += w;
        }
        let metrics_start = system_start - metrics_width;
        for r in &mut metric_slots {
            r.x += metrics_start;
        }
        // With no monitors, center the overflow chevron on the tray's pitch relative
        // to the first system icon. Otherwise the chevron precedes the monitor strip.
        let first_center = if metric_slots.is_empty() {
            system
                .first()
                .map_or(system_start + slot / 2, |(_, r)| r.x + r.width / 2)
        } else {
            metrics_start + slot / 2
        };
        let half = px(13).max(1);
        let overflow = (total > 0).then(|| {
            let center = (first_center - slot).max(half);
            cell(center - half, 2 * half)
        });
        let tray_end = overflow.map_or(metrics_start, |r| r.x + r.width / 2 - slot / 2);
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
        // The mark's ring is drawn 18 DIPs from the leading edge.
        let mark_x = px(4).min(title_x);
        let mark = cell(mark_x, (px(32).min(title_x) - mark_x).max(0));
        let title = cell(title_x, (tray_start - px(12) - title_x).max(0));
        Self {
            width,
            height,
            mark,
            title,
            tray,
            overflow,
            metrics: metric_slots,
            system,
            clock,
            close,
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> StatusHit {
        if self.close.is_some_and(|r| r.contains(x, y)) {
            StatusHit::CloseMaximized
        } else if let Some(slot) = self.tray.iter().find(|s| s.rect.contains(x, y)) {
            StatusHit::Tray(slot.index)
        } else if self.overflow.is_some_and(|r| r.contains(x, y)) {
            StatusHit::Overflow
        } else if let Some(&(item, _)) = self.system.iter().find(|(_, r)| r.contains(x, y)) {
            StatusHit::System(item)
        } else if self.clock.contains(x, y) {
            StatusHit::Clock
        } else if self.mark.contains(x, y) {
            StatusHit::Mark
        } else if self.title.contains(x, y) {
            StatusHit::Title
        } else {
            StatusHit::Bar
        }
    }

    /// The slot a hovered or open target highlights. The title and empty space have none.
    pub fn target(&self, hit: StatusHit) -> Option<Rect> {
        match hit {
            StatusHit::CloseMaximized => self.close,
            StatusHit::Tray(index) => self.tray.get(index).map(|slot| slot.rect),
            StatusHit::Overflow => self.overflow,
            StatusHit::System(item) => {
                let found = self.system.iter().find(|(i, _)| *i == item);
                found.map(|&(_, rect)| rect)
            }
            StatusHit::Clock => Some(self.clock),
            StatusHit::Mark => Some(self.mark),
            StatusHit::Title | StatusHit::Bar => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS_HEIGHT: i32 = 30;

    #[test]
    fn offset_monitor_keeps_dock_centered_and_hit_edges_half_open() {
        let work = Rect {
            x: -1920,
            y: -150,
            width: 1920,
            height: 1032,
        };
        for dpi in [96, 120, 144, 192] {
            let layout = Layout::new(work, dpi, 4, 0, DockStyle::default());
            let rect = layout.bottom;
            assert!((2 * (rect.x - work.x) + rect.width - work.width).abs() <= 1);
            assert_eq!(
                work.y + work.height - layout.bottom.y - layout.bottom.height,
                8 * dpi as i32 / 96
            );
            let slot = layout.slots[1].rect;
            assert_eq!(layout.hit(slot.x, slot.y), Some(DockHit::App(0)));
            assert_eq!(layout.hit(slot.x - 1, slot.y), Some(DockHit::Launchpad));
            assert_eq!(
                layout.hit(slot.x + slot.width, slot.y),
                Some(DockHit::App(1))
            );
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
        let layout = Layout::new(work, 144, 16, usize::MAX, DockStyle::default());
        let last = layout.slots.last().unwrap().rect;
        assert!(layout.bottom.width <= work.width);
        assert!(layout.first > 0);
        assert_eq!(layout.hit(last.x, last.y), Some(DockHit::App(15)));
        let first = layout.slots[0].rect;
        assert_eq!(layout.hit(first.x, first.y), Some(DockHit::Launchpad));
        let layout = Layout::new(work, 144, 1, layout.first, DockStyle::default());
        assert_eq!(layout.first, 0);
        let app = layout.slots[1].rect;
        assert_eq!(layout.hit(app.x, app.y), Some(DockHit::App(0)));
        let empty = Layout::new(work, 144, 0, 0, DockStyle::default());
        assert_eq!(empty.slots.len(), 1);
        assert_eq!(empty.hit(first.x, first.y), Some(DockHit::Launchpad));

        let narrow = Rect { width: 60, ..work };
        for (first, target) in [
            (0, DockHit::Launchpad),
            (1, DockHit::App(0)),
            (2, DockHit::App(1)),
        ] {
            let layout = Layout::new(narrow, 144, 2, first, DockStyle::default());
            let slot = layout.slots[0].rect;
            assert_eq!(layout.hit(slot.x, slot.y), Some(target));
        }
    }

    #[test]
    fn drops_distinguish_launchpad_insertion_and_both_reorder_directions() {
        let work = Rect {
            x: -800,
            y: 20,
            width: 340,
            height: 600,
        };
        for dpi in [96, 144] {
            let layout = Layout::new(work, dpi, 16, 5, DockStyle::default());
            let launcher = layout.slots[0].rect;
            let app = layout.slots[1].rect;
            let center = app.x + app.width / 2;
            assert_eq!(
                layout.drop_target(launcher.x, launcher.y, None),
                Some(DockDrop::Launchpad)
            );
            assert_eq!(
                layout.drop_target(center - 1, app.y, None),
                Some(DockDrop::Insert(5))
            );
            assert_eq!(
                layout.drop_target(center, app.y, None),
                Some(DockDrop::Insert(6))
            );
            assert_eq!(
                layout.drop_target(center, app.y, Some(4)),
                Some(DockDrop::Insert(6))
            );
            assert_eq!(
                layout.drop_target(center, app.y, Some(6)),
                Some(DockDrop::Insert(5))
            );
            assert_eq!(
                layout.drop_target(layout.bottom.width, app.y, Some(4)),
                None
            );
            assert_eq!(layout.drop_target(center, -1, Some(6)), None);
        }
        let empty = Layout::new(work, 96, 0, 0, DockStyle::default());
        assert_eq!(
            empty.drop_target(empty.bottom.width - 1, 0, None),
            Some(DockDrop::Insert(0))
        );
    }

    #[test]
    fn incoming_app_opens_a_visible_gap_at_the_pinned_boundary() {
        for (width, apps, first, at) in [(800, 0, 0, 0), (800, 2, 0, 1), (340, 16, 5, 3)] {
            let work = Rect {
                x: 0,
                y: 0,
                width,
                height: 600,
            };
            let layout = Layout::new(
                work,
                144,
                apps,
                first,
                DockStyle {
                    icon: 56,
                    ..DockStyle::default()
                },
            );
            let (slots, scale) = layout.insertion_preview(at);
            let icon = layout.icon as f32 * scale;
            let mut left = layout.slots[0].rect.x;
            let mut right = layout.bottom.width - left;
            for (slot, original) in slots.iter().zip(&layout.slots) {
                assert_eq!(slot.target, original.target);
                assert!(icon <= slot.rect.width as f32);
                if matches!(slot.target, DockHit::App(i) if i >= at) {
                    assert!(
                        slot.rect.x + slot.rect.width / 2
                            > original.rect.x + original.rect.width / 2
                    );
                    right = right.min(slot.rect.x);
                } else {
                    assert!(
                        slot.rect.x + slot.rect.width / 2
                            < original.rect.x + original.rect.width / 2
                    );
                    left = slot.rect.x + slot.rect.width;
                }
            }
            assert!((right - left) as f32 >= icon);
            assert_eq!(
                layout.file_drop_target(
                    (left + right) / 2,
                    layout.bottom.height / 2,
                    Some(DockDrop::Insert(at)),
                    at,
                ),
                Some(DockDrop::Insert(at))
            );
        }
    }

    #[test]
    fn stationary_drop_pointer_keeps_its_target_when_icons_make_room() {
        for (width, apps, first) in [(60, 2, 1), (800, 0, 0), (800, 3, 0), (340, 16, 5)] {
            let work = Rect {
                x: 0,
                y: 0,
                width,
                height: 600,
            };
            for dpi in [96, 144] {
                let layout = Layout::new(work, dpi, apps, first, DockStyle::default());
                for pins in [0, apps / 2, apps] {
                    for previous in
                        std::iter::once(None).chain((0..=pins).map(|at| Some(DockDrop::Insert(at))))
                    {
                        for y in [0, layout.bottom.height / 2] {
                            for x in 0..layout.bottom.width {
                                let next = layout.file_drop_target(x, y, previous, pins);
                                if let Some(DockDrop::Insert(at)) = next {
                                    assert!(at <= pins);
                                }
                                assert_eq!(
                                    layout.file_drop_target(x, y, next, pins),
                                    next,
                                    "pointer at {x},{y}, prior gap {previous:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn dock_style_sizes_slots_and_gap() {
        let work = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let style = DockStyle {
            icon: 48,
            spacing: 8,
            gap: 0,
        };
        let layout = Layout::new(work, 144, 3, 0, style);
        assert_eq!(layout.icon, 72);
        assert_eq!(layout.slots[0].rect.width, 84);
        assert_eq!(layout.bottom.height, 117);
        assert_eq!(layout.bottom.y + layout.bottom.height, work.height);
        let slot = layout.slots[2].rect;
        assert_eq!(
            layout.hit(slot.x + slot.width - 1, slot.y),
            Some(DockHit::App(1))
        );
    }

    #[test]
    fn revealed_window_connects_edge_strip_to_icons_across_the_gap() {
        let work = Rect {
            x: -1024,
            y: 40,
            width: 1024,
            height: 728,
        };
        let layout = Layout::new(work, 144, 4, 0, DockStyle::default());
        let edge = layout.reveal_edge(DockDisplayMode::AutoHide);
        let smart = layout.reveal_edge(DockDisplayMode::SmartHide);
        let revealed = layout.revealed_window();
        assert_eq!(edge.y, 767);
        assert_eq!(edge.height, 1);
        assert_eq!((edge.x, edge.width), (work.x, work.width));
        for x in [work.x, work.x + work.width - 1] {
            assert!(edge.contains(x, edge.y));
            assert!(!edge.contains(x, edge.y - 1));
            assert!(!revealed.contains(x, edge.y));
            assert!(!smart.contains(x, edge.y));
        }
        assert_eq!(
            (smart.x, smart.width),
            (layout.bottom.x, layout.bottom.width)
        );
        assert_eq!(revealed.y, layout.bottom.y);
        let x = edge.x + edge.width / 2;
        assert!(revealed.contains(x, edge.y));
        assert!(!layout.bottom.contains(x, edge.y));
        assert!(layout.hit(x - revealed.x, edge.y - revealed.y).is_none());
    }

    #[test]
    fn status_bar_keeps_title_room_and_overflows_extra_tray_icons() {
        for dpi in [96, 144, 192] {
            let px = scaled(dpi);
            let (width, height) = (px(1920), px(STATUS_HEIGHT));
            let system = [SystemItem::Network, SystemItem::Sound, SystemItem::Control];
            let layout = StatusLayout::new((width, height), dpi, 3, 5, &system, &[], false);
            assert_eq!(layout.tray.len(), 3);
            let overflow = layout.overflow.unwrap();
            let last = layout.tray[2].rect;
            assert!(last.x + last.width <= overflow.x);
            let (first, sound) = (layout.system[0].1, layout.system[1].1);
            assert!(overflow.x + overflow.width <= first.x);
            // The chevron sits on the tray's pitch, evenly between its neighbors.
            let center = |r: Rect| r.x + r.width / 2;
            let before = center(overflow) - center(last);
            let after = center(first) - center(overflow);
            assert!((before - after).abs() <= 1, "{dpi}: {before} vs {after}");
            assert!((before - last.width).abs() <= 1);
            let (_, control) = layout.system[2];
            assert!(control.x + control.width <= layout.clock.x);
            assert_eq!(layout.hit(sound.x, 0), StatusHit::System(SystemItem::Sound));
            assert_eq!(
                layout.hit(sound.x - 1, 0),
                StatusHit::System(SystemItem::Network)
            );
            assert!(layout.clock.x + layout.clock.width <= width);
            assert!(layout.title.x + layout.title.width < layout.tray[0].rect.x);
            assert_eq!(layout.hit(last.x, 0), StatusHit::Tray(2));
            assert_eq!(layout.hit(last.x + last.width - 1, 0), StatusHit::Tray(2));
            assert_eq!(layout.hit(overflow.x, 0), StatusHit::Overflow);
            let edge = overflow.x + overflow.width - 1;
            assert_eq!(layout.hit(edge, height - 1), StatusHit::Overflow);
            assert_eq!(layout.hit(px(18), px(10)), StatusHit::Mark);
            assert_eq!(layout.hit(px(2), px(10)), StatusHit::Bar);
            assert_eq!(layout.hit(px(33), px(10)), StatusHit::Bar);
            assert_eq!(layout.hit(layout.title.x, px(10)), StatusHit::Title);
            assert_eq!(layout.hit(layout.clock.x, px(10)), StatusHit::Clock);

            // A narrow bar moves icons into the overflow list but keeps the title area.
            let close = StatusLayout::new((width, height), dpi, 3, 5, &system, &[], true);
            assert_eq!(close.hit(width - 1, 0), StatusHit::CloseMaximized);
            assert_eq!(close.hit(close.close.unwrap().x - 1, 0), StatusHit::Clock);
            assert_eq!(layout.hit(width - 1, 0), StatusHit::Bar);
            let narrow = StatusLayout::new((px(520), height), dpi, 40, 40, &system, &[], false);
            assert!(narrow.tray.len() < 40);
            assert!(narrow.tray[0].rect.x >= px(220));
            assert!(narrow.overflow.is_some());
        }
        let empty = StatusLayout::new((800, 30), 96, 0, 0, &[], &[], false);
        assert!(empty.tray.is_empty() && empty.overflow.is_none());
        assert_eq!(empty.hit(700, 10), StatusHit::Title);
        assert_eq!(empty.hit(735, 10), StatusHit::Bar);
    }

    #[test]
    fn battery_slot_has_room_for_its_charge_and_targets_highlight_slots() {
        let system = [SystemItem::Battery, SystemItem::Control];
        let layout = StatusLayout::new((1920, 30), 96, 0, 0, &system, &[], false);
        let (battery, control) = (layout.system[0].1, layout.system[1].1);
        assert_eq!(battery.width, 60);
        assert_eq!(battery.x + battery.width, control.x);
        let target = layout
            .target(StatusHit::System(SystemItem::Control))
            .unwrap();
        assert_eq!(target, control);
        assert_eq!(layout.target(StatusHit::Clock), Some(layout.clock));
        assert_eq!(layout.target(StatusHit::Mark), Some(layout.mark));
        assert_eq!(layout.target(StatusHit::Bar), None);
    }

    #[test]
    fn metrics_stay_between_overflow_and_system_controls_at_each_scale() {
        use crate::metrics::Metric;

        let metrics = [Style::Value, Style::Bars, Style::Graph, Style::Ring]
            .map(|style| Indicator::new(Metric::Cpu, style));
        for dpi in [96, 120, 144, 192] {
            let px = scaled(dpi);
            let layout = |width, count| {
                StatusLayout::new(
                    (px(width), px(STATUS_HEIGHT)),
                    dpi,
                    count,
                    count,
                    &[SystemItem::Network, SystemItem::Sound],
                    &metrics,
                    false,
                )
            };
            let full = layout(1000, 5);
            assert_eq!(full.metrics.len(), metrics.len());
            let overflow = full.overflow.unwrap();
            assert!(overflow.x + overflow.width <= full.metrics[0].x);
            for pair in full.metrics.windows(2) {
                assert_eq!(pair[0].x + pair[0].width, pair[1].x);
            }
            let last = full.metrics.last().unwrap();
            assert_eq!(last.x + last.width, full.system[0].1.x);
            assert_eq!(full.hit(full.metrics[0].x, 0), StatusHit::Bar);
            assert_eq!(full.hit(overflow.x, 0), StatusHit::Overflow);
            assert_eq!(
                full.hit(last.x + last.width, 0),
                StatusHit::System(SystemItem::Network)
            );

            let narrow = layout(345, 5);
            assert_eq!(narrow.metrics.len(), 1);
            assert!(narrow.tray.is_empty());
            assert!(narrow.overflow.is_some());
            let empty_tray = layout(1000, 0);
            assert!(empty_tray.overflow.is_none());
            assert!(empty_tray.title.x + empty_tray.title.width <= empty_tray.metrics[0].x);
        }
    }
}
