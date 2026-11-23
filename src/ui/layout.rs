use crate::dock::Pin;
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
    /// Index in the application groups.
    App(usize),
    /// Index in the saved pin order.
    Separator(usize),
}

impl DockHit {
    fn width(self, app: i32, separator: i32) -> i32 {
        match self {
            Self::Separator(_) => separator,
            _ => app,
        }
    }
}

/// Application groups follow pin order, then include the remaining running apps.
pub fn dock_order(pins: &[Pin], app_count: usize) -> impl Iterator<Item = DockHit> {
    let mut pins = pins.iter().enumerate();
    let mut apps = (0..app_count).map(DockHit::App);
    std::iter::from_fn(move || match pins.next() {
        Some((index, Pin::Separator)) => Some(DockHit::Separator(index)),
        _ => apps.next(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockDrop {
    Launchpad,
    /// Boundary in the full Dock order, including separators and scrolled-out items.
    Insert(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DockSlot {
    pub target: DockHit,
    /// Position after the launchpad, counting both apps and separators.
    pub index: usize,
    /// Client pixels relative to the bottom bar.
    pub rect: Rect,
}

pub struct DockPreview {
    pub slots: Vec<DockSlot>,
    pub scale: f32,
    gap: Option<Rect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Screen pixels in the target monitor's work area.
    pub bottom: Rect,
    /// The last row of work-area pixels, including both corners.
    work_edge: Rect,
    pub slots: Vec<DockSlot>,
    pub order: Vec<DockHit>,
    pub first: usize,
    app_width: i32,
    separator_width: i32,
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
    pub fn new(
        work: Rect,
        dpi: u32,
        order: impl IntoIterator<Item = DockHit>,
        first: usize,
        style: DockStyle,
    ) -> Self {
        let order: Vec<_> = order.into_iter().collect();
        let px = scaled(dpi);
        let inset_x = px(8).min((work.width - 1) / 2);
        let available = work.width - 2 * inset_x;
        let padding = px(6).min((available - 1) / 2);
        let room = available - 2 * padding;
        let app_width = px(style.icon + style.spacing).clamp(1, room);
        let separator_width = px(16).clamp(1, app_width);
        let targets = std::iter::once(DockHit::Launchpad).chain(order.iter().copied());
        // Keep the launchpad in front if there is also room for an app. On smaller
        // displays it scrolls with the row so every target stays reachable.
        let fixed = usize::from(room / app_width >= 2);
        let scrolling = order.len() + 1 - fixed;
        let mut remaining = room - fixed as i32 * app_width;
        let tail = targets
            .clone()
            .rev()
            .take(scrolling)
            .take_while(|target| {
                remaining -= target.width(app_width, separator_width);
                remaining >= 0
            })
            .count();
        let first = first.min(scrolling - tail);
        let height = work.height.min(px(style.icon + 30));
        let inset_y = px(4).min((height - 1) / 2);
        let mut x = padding;
        let slots = targets
            .enumerate()
            .filter(|(i, _)| *i < fixed || *i >= fixed + first)
            .map_while(|(i, target)| {
                let width = target.width(app_width, separator_width);
                if x + width > padding + room {
                    return None;
                }
                let rect = Rect {
                    x,
                    y: inset_y,
                    width,
                    height: height - 2 * inset_y,
                };
                x += width;
                Some(DockSlot {
                    target,
                    index: i.saturating_sub(1),
                    rect,
                })
            })
            .collect();
        let width = x + padding;
        let inset = px(style.gap).min(work.height - height);
        let bottom = Rect {
            x: work.x + (work.width - width) / 2,
            y: work.y + work.height - height - inset,
            width,
            height,
        };
        let icon = px(style.icon)
            .min(app_width)
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
            order,
            first,
            app_width,
            separator_width,
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
        self.slot_at(x, y).map(|s| s.target)
    }

    pub fn slot_at(&self, x: i32, y: i32) -> Option<&DockSlot> {
        self.slots.iter().find(|s| s.rect.contains(x, y))
    }

    /// Fit a preview into the current HWND, preserving the narrower separator slots.
    fn fit_preview(&self, mut slots: Vec<DockSlot>, gap_at: Option<usize>) -> DockPreview {
        let first = self.slots[0].rect;
        let room = i64::from(self.bottom.width - 2 * first.x);
        let width = |hit: DockHit| i64::from(hit.width(self.app_width, self.separator_width));
        let pitch = i64::from(self.app_width);
        let total = slots.iter().map(|slot| width(slot.target)).sum::<i64>()
            + if gap_at.is_some() { pitch } else { 0 };
        let edge = |offset: i64| first.x + (offset * room / total) as i32;
        let mut offset = 0;
        let mut next = |w| {
            let x = edge(offset);
            offset += w;
            Rect {
                x,
                width: edge(offset) - x,
                ..first
            }
        };
        let mut gap = None;
        for (i, slot) in slots.iter_mut().enumerate() {
            if gap_at == Some(i) {
                gap = Some(next(pitch));
            }
            slot.rect = next(width(slot.target));
        }
        if gap_at == Some(slots.len()) {
            gap = Some(next(pitch));
        }
        let pitch = (pitch * room / total) as i32;
        let size = if gap.is_some() {
            pitch - pitch / 10
        } else {
            pitch
        };
        DockPreview {
            slots,
            gap,
            scale: size.clamp(1, self.icon) as f32 / self.icon as f32,
        }
    }

    pub fn insertion_preview(&self, at: usize) -> DockPreview {
        let gap = self
            .slots
            .partition_point(|s| s.target == DockHit::Launchpad || s.index < at);
        self.fit_preview(self.slots.clone(), Some(gap))
    }

    pub fn reorder_preview(&self, from: usize, to: usize) -> DockPreview {
        let mut order = self.order.clone();
        let item = order.remove(from);
        order.insert(to, item);
        let mut slots = self.slots.clone();
        for slot in &mut slots {
            if slot.target != DockHit::Launchpad {
                slot.target = order[slot.index];
            }
        }
        self.fit_preview(slots, None)
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
            let preview = self.insertion_preview(at);
            if preview
                .gap
                .is_some_and(|r| (r.x..r.x + r.width).contains(&x))
            {
                DockDrop::Insert(at)
            } else {
                Self::drop_among(preview.slots.into_iter(), x, y, None)
            }
        } else {
            Self::drop_among(self.slots.iter().copied(), x, y, None)
        };
        Some(match target {
            DockDrop::Insert(at) => DockDrop::Insert(at.min(pins)),
            DockDrop::Launchpad => DockDrop::Launchpad,
        })
    }

    /// `source` is the item's current position in the full Dock order.
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
            if slot.target == DockHit::Launchpad {
                if slot.rect.contains(x, y) {
                    return DockDrop::Launchpad;
                }
                continue;
            }
            if let Some(from) = source
                && slot.rect.contains(x, y)
            {
                return DockDrop::Insert(slot.index + usize::from(from < slot.index));
            }
            if x < slot.rect.x + slot.rect.width / 2 {
                return DockDrop::Insert(slot.index);
            }
            boundary = slot.index + 1;
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
    /// Width in DIPs of what the control shows. The input language's single character
    /// fits an icon's width.
    pub fn width(self) -> i32 {
        match self {
            Self::Battery => BATTERY_ICON,
            _ => BAR_ICON,
        }
    }
}

/// Edge in DIPs of the icons on the status bar: tray images, system icons, the overflow
/// list and each monitor's symbol.
pub const BAR_ICON: i32 = 16;
/// The battery is drawn wider, so its level reads at the bar's height.
pub const BATTERY_ICON: i32 = 22;
/// Space in DIPs between the visible content of neighboring status bar items. A slot
/// pads its content by half of it on each side and slots abut, so icons, readings and
/// the time all stand one gap apart and every point between them hits one.
pub const BAR_GAP: i32 = 12;
/// Space in DIPs between a monitor's symbol and its readout.
pub const METRIC_SPACING: i32 = 4;
/// Text sizes in DIPs, shared with the renderer so the text measured is the text drawn.
/// The clock is semibold.
pub const CLOCK_TEXT: f32 = 13.;
pub const METRIC_TEXT: f32 = 12.;
/// The widest the clock gets: it shows two-digit hours and minutes.
const CLOCK_SAMPLE: &str = "00:00";
/// Distance in DIPs from the trailing edge to the clock's slot, which leaves the time as
/// far from that edge as the mark's ring is from the leading one.
const TRAILING: i32 = 6;

/// Width in DIPs of a monitor's readout beside its symbol. A value reserves its widest
/// reading in the bar's font, three digits, and is drawn right-aligned, so changing
/// readings never move their neighbors.
pub fn readout_width(indicator: &Indicator, measure: &dyn Fn(&str, f32, bool) -> f32) -> f32 {
    match indicator.style {
        Style::Value => measure(&indicator.format_value(100), METRIC_TEXT, false),
        Style::Bars | Style::Ring => 16.,
        Style::Graph => 26.,
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
    /// `measure` gives the width in DIPs of one line of text at a size, semibold when
    /// asked, in the bar's font.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        size: (i32, i32),
        dpi: u32,
        shown: usize,
        total: usize,
        system: &[SystemItem],
        metrics: &[Indicator],
        close_maximized: bool,
        measure: impl Fn(&str, f32, bool) -> f32,
    ) -> Self {
        let px = scaled(dpi);
        // Every item's slot is its content plus one gap.
        let padded = |content: f32| {
            ((content + BAR_GAP as f32) * dpi as f32 / 96.)
                .round()
                .max(1.) as i32
        };
        let width = size.0.max(1);
        let height = size.1.max(1);
        let cell = |x, w| Rect {
            x,
            y: 0,
            width: w,
            height,
        };
        let slot = padded(BAR_ICON as f32);
        // Flush with the corner, so a throw of the pointer there closes the window.
        let close = close_maximized.then(|| cell(width - slot.min(width), slot.min(width)));
        let clock_end = close.map_or(width - px(TRAILING), |r| r.x).max(0);
        let clock_width = padded(measure(CLOCK_SAMPLE, CLOCK_TEXT, true)).min(clock_end);
        let clock = cell(clock_end - clock_width, clock_width);
        let widths = system
            .iter()
            .map(|&item| padded(item.width() as f32))
            .collect::<Vec<_>>();
        let system_start = (clock.x - widths.iter().sum::<i32>()).max(0);
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
            let symbol = (BAR_ICON + METRIC_SPACING) as f32;
            let w = padded(symbol + readout_width(indicator, &measure));
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
        let overflow = (total > 0).then(|| cell((metrics_start - slot).max(0), slot));
        let tray_end = overflow.map_or(metrics_start, |r| r.x);
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

    /// Text widths as the renderer estimates them when DirectWrite cannot lay text out.
    fn estimate(text: &str, size: f32, _bold: bool) -> f32 {
        text.chars().count() as f32 * size * 0.62
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
            let layout = Layout::new(work, dpi, dock_order(&[], 4), 0, DockStyle::default());
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
        let layout = Layout::new(
            work,
            144,
            dock_order(&[], 16),
            usize::MAX,
            DockStyle::default(),
        );
        let last = layout.slots.last().unwrap().rect;
        assert!(layout.bottom.width <= work.width);
        assert!(layout.first > 0);
        assert_eq!(layout.hit(last.x, last.y), Some(DockHit::App(15)));
        let first = layout.slots[0].rect;
        assert_eq!(layout.hit(first.x, first.y), Some(DockHit::Launchpad));
        let layout = Layout::new(
            work,
            144,
            dock_order(&[], 1),
            layout.first,
            DockStyle::default(),
        );
        assert_eq!(layout.first, 0);
        let app = layout.slots[1].rect;
        assert_eq!(layout.hit(app.x, app.y), Some(DockHit::App(0)));
        let empty = Layout::new(work, 144, dock_order(&[], 0), 0, DockStyle::default());
        assert_eq!(empty.slots.len(), 1);
        assert_eq!(empty.hit(first.x, first.y), Some(DockHit::Launchpad));

        let narrow = Rect { width: 60, ..work };
        for (first, target) in [
            (0, DockHit::Launchpad),
            (1, DockHit::App(0)),
            (2, DockHit::App(1)),
        ] {
            let layout = Layout::new(narrow, 144, dock_order(&[], 2), first, DockStyle::default());
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
            let layout = Layout::new(work, dpi, dock_order(&[], 16), 5, DockStyle::default());
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
        let empty = Layout::new(work, 96, dock_order(&[], 0), 0, DockStyle::default());
        assert_eq!(
            empty.drop_target(empty.bottom.width - 1, 0, None),
            Some(DockDrop::Insert(0))
        );
    }

    #[test]
    fn separators_have_narrow_slots_and_keep_apps_on_the_chosen_side() {
        use crate::dock::pinned;
        use DockHit::{App, Launchpad, Separator};
        let pins = [
            Pin::App(pinned("editor.lnk".into())),
            Pin::Separator,
            Pin::App(pinned("viewer.exe".into())),
            Pin::Separator,
        ];
        let work = Rect {
            x: -1200,
            y: 0,
            width: 1200,
            height: 800,
        };
        for dpi in [96, 144, 192] {
            let layout = Layout::new(work, dpi, dock_order(&pins, 3), 0, DockStyle::default());
            let targets = |slots: &[DockSlot]| slots.iter().map(|s| s.target).collect::<Vec<_>>();
            assert_eq!(
                targets(&layout.slots),
                [
                    Launchpad,
                    App(0),
                    Separator(1),
                    App(1),
                    Separator(3),
                    App(2)
                ]
            );
            let line = layout.slots[2].rect;
            assert!(line.width < layout.slots[1].rect.width);
            let mid = line.x + line.width / 2;
            let y = line.y + line.height / 2;
            assert_eq!(layout.hit(line.x - 1, y), Some(App(0)));
            assert_eq!(layout.hit(mid, y), Some(Separator(1)));
            assert_eq!(layout.hit(line.x + line.width, y), Some(App(1)));
            for (x, at) in [(mid - 1, 1), (mid, 2)] {
                let target = Some(DockDrop::Insert(at));
                assert_eq!(layout.file_drop_target(x, y, None, pins.len()), target);
                assert_eq!(layout.file_drop_target(x, y, target, pins.len()), target);
            }
            let preview = layout.reorder_preview(1, 2);
            assert_eq!(
                targets(&preview.slots),
                [
                    Launchpad,
                    App(0),
                    App(1),
                    Separator(1),
                    Separator(3),
                    App(2)
                ]
            );
            assert_eq!(preview.slots[3].rect.width, line.width);
            assert_eq!(preview.slots[2].rect.x, line.x);
            assert_eq!(layout.hit(mid, y), Some(Separator(1)));
        }
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
                dock_order(&[], apps),
                first,
                DockStyle {
                    icon: 56,
                    ..DockStyle::default()
                },
            );
            let preview = layout.insertion_preview(at);
            let icon = layout.icon as f32 * preview.scale;
            let mut left = layout.slots[0].rect.x;
            let mut right = layout.bottom.width - left;
            for (slot, original) in preview.slots.iter().zip(&layout.slots) {
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
        for (width, order, first) in [
            (60, dock_order(&[], 2).collect(), 1),
            (800, dock_order(&[], 0).collect(), 0),
            (800, dock_order(&[], 3).collect(), 0),
            (340, dock_order(&[], 16).collect(), 5),
            (
                220,
                vec![
                    DockHit::App(0),
                    DockHit::Separator(1),
                    DockHit::App(1),
                    DockHit::Separator(3),
                    DockHit::App(2),
                    DockHit::App(3),
                ],
                1,
            ),
        ] {
            let work = Rect {
                x: 0,
                y: 0,
                width,
                height: 600,
            };
            for dpi in [96, 144] {
                let layout = Layout::new(work, dpi, order.clone(), first, DockStyle::default());
                for pins in [0, order.len() / 2, order.len()] {
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
        let layout = Layout::new(work, 144, dock_order(&[], 3), 0, style);
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
        let layout = Layout::new(work, 144, dock_order(&[], 4), 0, DockStyle::default());
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
            let layout =
                StatusLayout::new((width, height), dpi, 3, 5, &system, &[], false, estimate);
            assert_eq!(layout.tray.len(), 3);
            let overflow = layout.overflow.unwrap();
            let last = layout.tray[2].rect;
            assert!(last.x + last.width <= overflow.x);
            let (first, sound) = (layout.system[0].1, layout.system[1].1);
            assert!(overflow.x + overflow.width <= first.x);
            // The overflow list sits on the tray's pitch, evenly between its neighbors.
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
            let close = StatusLayout::new((width, height), dpi, 3, 5, &system, &[], true, estimate);
            assert_eq!(close.hit(width - 1, 0), StatusHit::CloseMaximized);
            assert_eq!(close.hit(close.close.unwrap().x - 1, 0), StatusHit::Clock);
            assert_eq!(layout.hit(width - 1, 0), StatusHit::Bar);
            let narrow = StatusLayout::new(
                (px(520), height),
                dpi,
                40,
                40,
                &system,
                &[],
                false,
                estimate,
            );
            assert!(narrow.tray.len() < 40);
            assert!(narrow.tray[0].rect.x >= px(220));
            assert!(narrow.overflow.is_some());
        }
        let empty = StatusLayout::new((800, 30), 96, 0, 0, &[], &[], false, estimate);
        assert!(empty.tray.is_empty() && empty.overflow.is_none());
        assert_eq!(empty.hit(700, 10), StatusHit::Title);
        assert_eq!(empty.hit(735, 10), StatusHit::Bar);
    }

    #[test]
    fn battery_slot_fits_its_wider_icon_and_targets_highlight_slots() {
        let system = [SystemItem::Battery, SystemItem::Control];
        let layout = StatusLayout::new((1920, 30), 96, 0, 0, &system, &[], false, estimate);
        let (battery, control) = (layout.system[0].1, layout.system[1].1);
        assert_eq!((battery.width, control.width), (34, 28));
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
                    estimate,
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

    #[test]
    fn neighbors_stand_one_gap_apart_whatever_they_show() {
        use crate::metrics::Metric;

        let metrics = [
            Indicator::new(Metric::Memory, Style::Value),
            Indicator::new(Metric::CpuTemperature, Style::Value),
            Indicator::new(Metric::Cpu, Style::Bars),
            Indicator::new(Metric::Disk, Style::Graph),
            Indicator::new(Metric::Cpu, Style::Ring),
        ];
        let system = [
            SystemItem::InputLanguage,
            SystemItem::Network,
            SystemItem::Battery,
            SystemItem::Sound,
            SystemItem::Control,
        ];
        let wide = |text: &str, size, bold| estimate(text, size, bold) * 1.4;
        for measure in [&estimate as &dyn Fn(&str, f32, bool) -> f32, &wide] {
            for dpi in [96, 120, 144, 192] {
                let s = dpi as f32 / 96.;
                let px = scaled(dpi);
                let layout = StatusLayout::new(
                    (px(1920), px(STATUS_HEIGHT)),
                    dpi,
                    4,
                    6,
                    &system,
                    &metrics,
                    true,
                    measure,
                );
                assert_eq!(layout.metrics.len(), metrics.len());
                // Every slot from the first tray icon to the close button, with the width
                // in DIPs of what it shows.
                let icon = BAR_ICON as f32;
                let mut row = layout
                    .tray
                    .iter()
                    .map(|slot| (slot.rect, icon))
                    .collect::<Vec<_>>();
                row.push((layout.overflow.unwrap(), icon));
                row.extend(layout.metrics.iter().zip(&metrics).map(|(&r, indicator)| {
                    let symbol = (BAR_ICON + METRIC_SPACING) as f32;
                    (r, symbol + readout_width(indicator, measure))
                }));
                row.extend(
                    layout
                        .system
                        .iter()
                        .map(|&(item, r)| (r, item.width() as f32)),
                );
                let clock = measure(CLOCK_SAMPLE, CLOCK_TEXT, true);
                row.extend([(layout.clock, clock), (layout.close.unwrap(), icon)]);
                for (rect, content) in &row {
                    let padding = rect.width as f32 - content * s;
                    assert!(
                        (padding - BAR_GAP as f32 * s).abs() <= 0.5,
                        "{dpi}: {rect:?} pads {content} DIPs by {padding} pixels"
                    );
                }
                for pair in row.windows(2) {
                    assert_eq!(pair[0].0.x + pair[0].0.width, pair[1].0.x, "{dpi}");
                }
                assert_eq!(
                    row.last().unwrap().0.x + row.last().unwrap().0.width,
                    px(1920)
                );
            }
        }
    }
}
