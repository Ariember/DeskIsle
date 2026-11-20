use crate::geometry::Rect;

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
    /// Edge of each icon in pixels.
    pub icon: i32,
}

/// The Dock's adjustable sizes in DIPs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DockStyle {
    pub icon: i32,
    /// Distance from the bottom of the work area.
    pub gap: i32,
}

impl Default for DockStyle {
    fn default() -> Self {
        Self { icon: 34, gap: 8 }
    }
}

fn scaled(dpi: u32) -> impl Fn(i32) -> i32 {
    move |dip: i32| ((i64::from(dip) * i64::from(dpi) + 48) / 96) as i32
}

impl Layout {
    /// `work` is a nonempty work area and `dpi` its effective DPI.
    pub fn new(work: Rect, dpi: u32, count: usize, first: usize, style: DockStyle) -> Self {
        let px = scaled(dpi);
        let inset_x = px(8).min((work.width - 1) / 2);
        let available = work.width - 2 * inset_x;
        let padding = px(6).min((available - 1) / 2);
        let slot_width = px(style.icon + 18).min(available - 2 * padding).max(1);
        let capacity = ((available - 2 * padding) / slot_width) as usize;
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
        let icon = px(style.icon)
            .min(slot_width)
            .min(bottom.height - 2 * inset_y)
            .max(1);
        Self {
            bottom,
            slots,
            first,
            icon,
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

/// A system control on the status bar, between the tray and the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemItem {
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
    Tray(usize),
    Overflow,
    System(SystemItem),
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
    /// System controls in display order.
    pub system: Vec<(SystemItem, Rect)>,
    pub clock: Rect,
}

impl StatusLayout {
    /// `shown` counts the tray icons the user keeps on the bar. Icons that do not fit
    /// stay reachable through the overflow button.
    /// `system` lists the system controls to show, in display order; they always fit
    /// before tray icons do.
    pub fn new(
        width: i32,
        height: i32,
        dpi: u32,
        shown: usize,
        total: usize,
        system: &[SystemItem],
    ) -> Self {
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
        let overflow = (total > 0).then(|| cell((system_start - px(6) - slot).max(0), slot));
        let tray_end = overflow.map_or(system_start, |r| r.x);
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
            system,
            clock,
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> StatusHit {
        if let Some(slot) = self.tray.iter().find(|s| s.rect.contains(x, y)) {
            StatusHit::Tray(slot.index)
        } else if self.overflow.is_some_and(|r| r.contains(x, y)) {
            StatusHit::Overflow
        } else if let Some(&(item, _)) = self.system.iter().find(|(_, r)| r.contains(x, y)) {
            StatusHit::System(item)
        } else if self.clock.contains(x, y) {
            StatusHit::Clock
        } else if self.title.contains(x, y) {
            StatusHit::Title
        } else {
            StatusHit::Bar
        }
    }

    /// The slot a hovered or open target highlights. The title and empty space have none.
    pub fn target(&self, hit: StatusHit) -> Option<Rect> {
        match hit {
            StatusHit::Tray(index) => self.tray.get(index).map(|slot| slot.rect),
            StatusHit::Overflow => self.overflow,
            StatusHit::System(item) => {
                let found = self.system.iter().find(|(i, _)| *i == item);
                found.map(|&(_, rect)| rect)
            }
            StatusHit::Clock => Some(self.clock),
            StatusHit::Title | StatusHit::Bar => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let layout = Layout::new(work, 144, 16, usize::MAX, DockStyle::default());
        let last = layout.slots.last().unwrap().rect;
        assert!(layout.bottom.width <= work.width);
        assert!(layout.first > 0);
        assert_eq!(layout.hit(last.x, last.y), Some(15));
        let layout = Layout::new(work, 144, 1, layout.first, DockStyle::default());
        assert_eq!(layout.first, 0);
        let first = layout.slots[0].rect;
        assert_eq!(layout.hit(first.x, first.y), Some(0));
    }

    #[test]
    fn dock_style_sizes_slots_and_gap() {
        let work = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let style = DockStyle { icon: 48, gap: 0 };
        let layout = Layout::new(work, 144, 3, 0, style);
        assert_eq!(layout.icon, 72);
        assert_eq!(layout.slots[0].rect.width, 99);
        assert_eq!(layout.bottom.height, 117);
        assert_eq!(layout.bottom.y + layout.bottom.height, work.height);
        let slot = layout.slots[2].rect;
        assert_eq!(layout.hit(slot.x + slot.width - 1, slot.y), Some(2));
    }

    #[test]
    fn status_bar_keeps_title_room_and_overflows_extra_tray_icons() {
        for dpi in [96, 144, 192] {
            let px = scaled(dpi);
            let (width, height) = (px(1920), px(STATUS_HEIGHT));
            let system = [SystemItem::Network, SystemItem::Sound, SystemItem::Control];
            let layout = StatusLayout::new(width, height, dpi, 3, 5, &system);
            assert_eq!(layout.tray.len(), 3);
            let overflow = layout.overflow.unwrap();
            let last = layout.tray[2].rect;
            assert!(last.x + last.width <= overflow.x);
            let (first, sound) = (layout.system[0].1, layout.system[1].1);
            assert!(overflow.x + overflow.width <= first.x);
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
            assert_eq!(layout.hit(last.x + last.width, 0), StatusHit::Overflow);
            assert_eq!(layout.hit(overflow.x - 1, height - 1), StatusHit::Tray(2));
            assert_eq!(layout.hit(px(20), px(10)), StatusHit::Bar);
            assert_eq!(layout.hit(layout.title.x, px(10)), StatusHit::Title);
            assert_eq!(layout.hit(layout.clock.x, px(10)), StatusHit::Clock);

            // A narrow bar moves icons into the overflow list but keeps the title area.
            let narrow = StatusLayout::new(px(520), height, dpi, 40, 40, &system);
            assert!(narrow.tray.len() < 40);
            assert!(narrow.tray[0].rect.x >= px(220));
            assert!(narrow.overflow.is_some());
        }
        let empty = StatusLayout::new(800, 30, 96, 0, 0, &[]);
        assert!(empty.tray.is_empty() && empty.overflow.is_none());
        assert_eq!(empty.hit(700, 10), StatusHit::Title);
        assert_eq!(empty.hit(735, 10), StatusHit::Bar);
    }

    #[test]
    fn battery_slot_has_room_for_its_charge_and_targets_highlight_slots() {
        let system = [SystemItem::Battery, SystemItem::Control];
        let layout = StatusLayout::new(1920, 30, 96, 0, 0, &system);
        let (battery, control) = (layout.system[0].1, layout.system[1].1);
        assert_eq!(battery.width, 60);
        assert_eq!(battery.x + battery.width, control.x);
        let target = layout
            .target(StatusHit::System(SystemItem::Control))
            .unwrap();
        assert_eq!(target, control);
        assert_eq!(layout.target(StatusHit::Clock), Some(layout.clock));
        assert_eq!(layout.target(StatusHit::Bar), None);
    }
}
