//! The settings panel: a page per [`settings::Section`] built from the settings schema,
//! its layout in DIPs, pointer hits and keyboard navigation. Drawing lives in the
//! renderer and the window in the platform layer.

use crate::geometry::Rect;
use crate::settings::{
    self, Adjust, Control, Key, PreviewKey, SCHEMA, SECTIONS, Section, Settings,
};

pub const WIDTH: f32 = 720.;
pub const HEIGHT: f32 = 560.;
const MIN_WIDTH: f32 = 600.;
const MIN_HEIGHT: f32 = 260.;
pub const CAPTION: f32 = 48.;
const CLOSE_WIDTH: f32 = 46.;
const NAV_LEFT: f32 = 12.;
const NAV_RIGHT: f32 = 196.;
const NAV_ITEM: f32 = 34.;
const NAV_GAP: f32 = 4.;
const CONTENT_LEFT: f32 = 212.;
const CONTENT_RIGHT: f32 = 20.;
const FOOTER: f32 = 60.;
const CARD: f32 = 68.;
const MONITOR_CARD: f32 = 78.;
const MONITOR_INSET: f32 = 72.;
const CARD_GAP: f32 = 8.;
const CONTROL_INSET: f32 = 16.;
const SWITCH_WIDTH: f32 = 40.;
const SWITCH_HEIGHT: f32 = 20.;
const STEP_BUTTON: f32 = 32.;
const STEP_VALUE: f32 = 64.;
const SEGMENT: f32 = 56.;
const CONTROL_HEIGHT: f32 = 32.;
const RESET_WIDTH: f32 = 112.;
/// Content moved per wheel notch, in DIPs.
const SCROLL_STEP: f32 = 48.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Area {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        (self.left..self.right).contains(&x) && (self.top..self.bottom).contains(&y)
    }

    pub fn middle(&self) -> f32 {
        (self.top + self.bottom) / 2.
    }
}

/// What a click or key in the panel asks the coordinator to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Close,
    Page(usize),
    /// Restores the defaults of every setting on the current page.
    Reset,
    Change(Key, Adjust),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Switch {
        area: Area,
        on: bool,
    },
    Stepper {
        minus: Area,
        value: Area,
        plus: Area,
        text: String,
        can_down: bool,
        can_up: bool,
    },
    Segments {
        options: Vec<(Area, &'static str)>,
        selected: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: Key,
    pub label: &'static str,
    pub help: &'static str,
    /// The card behind the row, already moved by the scroll offset.
    pub card: Area,
    pub part: Part,
    pub detail: Option<Detail>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub key: Key,
    pub label: &'static str,
    pub part: Part,
    /// Alongside a checkbox. The label and box activate the same setting.
    pub label_area: Option<Area>,
}

/// Keyboard intents, translated from keys by the platform layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Next,
    Previous,
    Decrease,
    Increase,
    Activate,
    NextPage,
    PreviousPage,
    Close,
}

/// What the user is looking at; kept across layouts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelState {
    pub page: usize,
    /// DIPs the page content moved up.
    pub scroll: f32,
    /// A row of the current page, shown with a focus ring after keyboard use.
    pub focus: Option<usize>,
    pub hover: Option<Action>,
}

impl PanelState {
    pub fn open_page(&mut self, page: usize) {
        if page < SECTIONS.len() && page != self.page {
            *self = Self {
                page,
                ..Self::default()
            };
        }
    }

    /// Moves the content by wheel `delta` (120 per notch). Returns whether it moved.
    pub fn scroll_by(&mut self, delta: i16, layout: &PanelLayout) -> bool {
        let scroll =
            (self.scroll - f32::from(delta) / 120. * SCROLL_STEP).clamp(0., layout.max_scroll);
        let moved = scroll != self.scroll;
        self.scroll = scroll;
        moved
    }

    /// Handles a key. Focus and scroll change here; the returned action is for the caller.
    pub fn key(&mut self, nav: Nav, layout: &PanelLayout) -> Option<Action> {
        let count = layout.rows.len();
        let focused = self.focus.filter(|&i| i < count);
        let page = |offset: usize| Action::Page((self.page + offset) % SECTIONS.len());
        match nav {
            Nav::Close => Some(Action::Close),
            Nav::NextPage => Some(page(1)),
            Nav::PreviousPage => Some(page(SECTIONS.len() - 1)),
            Nav::Next | Nav::Previous if count > 0 => {
                let next = match (focused, nav == Nav::Next) {
                    (None, true) => 0,
                    (None, false) => count - 1,
                    (Some(i), true) => (i + 1) % count,
                    (Some(i), false) => (i + count - 1) % count,
                };
                self.focus = Some(next);
                // Bring the focused card fully into view.
                let card = layout.rows[next].card;
                let view = layout.view;
                if card.top < view.top {
                    self.scroll -= view.top - card.top;
                } else if card.bottom > view.bottom {
                    self.scroll += card.bottom - view.bottom;
                }
                self.scroll = self.scroll.clamp(0., layout.max_scroll);
                None
            }
            Nav::Decrease | Nav::Increase | Nav::Activate => {
                let row = &layout.rows[focused?];
                let adjust = match nav {
                    Nav::Decrease => Adjust::Step(-1),
                    Nav::Increase => Adjust::Step(1),
                    _ => Adjust::Toggle,
                };
                let key = match (&row.detail, nav) {
                    (Some(detail), Nav::Decrease | Nav::Increase) => detail.key,
                    _ => row.key,
                };
                Some(Action::Change(key, adjust))
            }
            _ => None,
        }
    }
}

/// The panel's geometry in DIPs for the current page.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    pub close: Area,
    pub nav: Vec<(Area, Section)>,
    pub page: usize,
    /// The page title in the fixed caption.
    pub title: Area,
    /// Content scrolls inside this area and is clipped to it.
    pub view: Area,
    pub rows: Vec<Row>,
    pub reset: Area,
    pub max_scroll: f32,
    pub scroll_thumb: Option<Area>,
    pub focus: Option<usize>,
    pub hover: Option<Action>,
}

impl PanelLayout {
    /// Fits the window to its monitor's work area and clamps the current page's scroll.
    pub fn new(state: &mut PanelState, settings: &Settings, dpi: u32, work: Rect) -> Self {
        let scale = dpi as f32 / 96.;
        let width = (work.width as f32 / scale).clamp(MIN_WIDTH, WIDTH).floor();
        let height = (work.height as f32 / scale)
            .clamp(MIN_HEIGHT, HEIGHT)
            .floor();
        let close = Area::new(width - CLOSE_WIDTH, 0., width, CAPTION - 8.);
        let nav = SECTIONS
            .iter()
            .enumerate()
            .map(|(i, &section)| {
                let top = CAPTION + 8. + i as f32 * (NAV_ITEM + NAV_GAP);
                (Area::new(NAV_LEFT, top, NAV_RIGHT, top + NAV_ITEM), section)
            })
            .collect();
        state.page = state.page.min(SECTIONS.len() - 1);
        let section = SECTIONS[state.page];
        let view = Area::new(
            CONTENT_LEFT,
            CAPTION + 12.,
            width - CONTENT_RIGHT,
            height - FOOTER,
        );
        let mut rows = Vec::new();
        let mut top = view.top;
        for spec in SCHEMA
            .iter()
            .filter(|s| s.section == section && detail_parent(s.key).is_none())
        {
            let detail = SCHEMA
                .iter()
                .find(|s| detail_parent(s.key) == Some(spec.key));
            let height = if detail.is_some() { MONITOR_CARD } else { CARD };
            let card = Area::new(view.left, top, view.right, top + height);
            rows.push((spec, detail, card));
            top = card.bottom + CARD_GAP;
        }
        let content = top - view.top;
        let max_scroll = (content - (view.bottom - view.top)).max(0.);
        state.scroll = state.scroll.clamp(0., max_scroll);
        let scroll_thumb = (max_scroll > 0.).then(|| {
            let track = view.bottom - view.top;
            let thumb = (track * track / content).max(18.);
            let y = view.top + (track - thumb) * state.scroll / max_scroll;
            Area::new(view.right + 6., y, view.right + 10., y + thumb)
        });
        state.focus = state.focus.filter(|&i| i < rows.len());
        let rows = rows
            .into_iter()
            .map(|(spec, detail, mut card)| {
                card.top -= state.scroll;
                card.bottom -= state.scroll;
                Row {
                    key: spec.key,
                    label: spec.label,
                    help: spec.help,
                    card,
                    part: part(spec, settings.get(spec.key), card),
                    detail: detail.map(|s| detail_control(s, settings.get(s.key), card)),
                }
            })
            .collect();
        let reset_top = height - FOOTER + (FOOTER - CONTROL_HEIGHT) / 2.;
        Self {
            scale,
            width,
            height,
            close,
            nav,
            page: state.page,
            title: Area::new(view.left, 0., close.left, CAPTION),
            view,
            rows,
            reset: Area::new(
                view.right - RESET_WIDTH,
                reset_top,
                view.right,
                reset_top + CONTROL_HEIGHT,
            ),
            max_scroll,
            scroll_thumb,
            focus: state.focus,
            hover: state.hover,
        }
    }

    /// The window size in physical pixels.
    pub fn pixels(&self) -> (i32, i32) {
        (
            (self.width * self.scale).round() as i32,
            (self.height * self.scale).round() as i32,
        )
    }

    /// The draggable caption: its height and where the close button begins, in pixels.
    pub fn caption_pixels(&self) -> (i32, i32) {
        (
            (CAPTION * self.scale).round() as i32,
            (self.close.left * self.scale).round() as i32,
        )
    }

    /// Centers the window in `work`, in screen pixels.
    pub fn place(&self, work: Rect) -> Rect {
        let (width, height) = self.pixels();
        Rect {
            x: work.x + (work.width - width) / 2,
            y: work.y + ((work.height - height) / 2).max(0),
            width,
            height,
        }
    }

    /// `x` and `y` are client pixels of the panel window.
    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        if self.close.contains(x, y) {
            return Some(Action::Close);
        }
        if let Some(page) = self.nav.iter().position(|(area, _)| area.contains(x, y)) {
            return Some(Action::Page(page));
        }
        if self.reset.contains(x, y) {
            return Some(Action::Reset);
        }
        if !self.view.contains(x, y) {
            return None;
        }
        let row = self.rows.iter().find(|row| row.card.contains(x, y))?;
        if let Some(detail) = &row.detail {
            match &detail.part {
                Part::Switch { area, .. }
                    if area.contains(x, y)
                        || detail.label_area.is_some_and(|a| a.contains(x, y)) =>
                {
                    return Some(Action::Change(detail.key, Adjust::Toggle));
                }
                Part::Segments { options, .. } => {
                    if let Some(i) = options.iter().position(|(area, _)| area.contains(x, y)) {
                        return Some(Action::Change(detail.key, Adjust::Pick(i)));
                    }
                }
                _ => {}
            }
        }
        let adjust = match &row.part {
            // In composite cards the space around the style buttons is neutral.
            Part::Switch { area, .. }
                if row.detail.is_some() && !area.contains(x, y) && y >= row.card.middle() =>
            {
                return None;
            }
            // The whole card flips a standalone switch, as in Windows Settings.
            Part::Switch { .. } => Adjust::Toggle,
            Part::Stepper {
                minus,
                plus,
                can_down,
                can_up,
                ..
            } => {
                if *can_down && minus.contains(x, y) {
                    Adjust::Step(-1)
                } else if *can_up && plus.contains(x, y) {
                    Adjust::Step(1)
                } else {
                    return None;
                }
            }
            Part::Segments { options, .. } => {
                Adjust::Pick(options.iter().position(|(area, _)| area.contains(x, y))?)
            }
        };
        Some(Action::Change(row.key, adjust))
    }
}

/// These controls share a card with the monitor they configure.
fn detail_parent(key: Key) -> Option<Key> {
    use PreviewKey::*;
    Some(Key::Preview(match key {
        Key::Preview(CpuStyle) => Cpu,
        Key::Preview(GpuTemperature) => Temperature,
        Key::Preview(MemoryStyle) => Memory,
        Key::Preview(DiskStyle) => Disk,
        _ => return None,
    }))
}

fn detail_control(spec: &settings::Spec, value: i32, card: Area) -> Detail {
    let left = card.left + MONITOR_INSET;
    let bottom = card.bottom - 10.;
    let area = Area::new(left, bottom - 24., left + 24., bottom);
    let part = match spec.control {
        Control::Toggle => Part::Switch {
            area,
            on: value != 0,
        },
        Control::Choice(choices) => Part::Segments {
            options: choices
                .iter()
                .enumerate()
                .map(|(i, choice)| {
                    let x = left + i as f32 * 32.;
                    (Area::new(x, area.top, x + 28., bottom), choice.label)
                })
                .collect(),
            selected: value as usize,
        },
        Control::Stepper { .. } => unreachable!("monitor details are toggles or styles"),
    };
    let label_area = matches!(part, Part::Switch { .. })
        .then(|| Area::new(area.right, area.top, area.right + 44., bottom));
    Detail {
        key: spec.key,
        label: spec.label,
        part,
        label_area,
    }
}

fn part(spec: &settings::Spec, value: i32, card: Area) -> Part {
    let right = card.right - CONTROL_INSET;
    let middle = card.middle();
    let (top, bottom) = (middle - CONTROL_HEIGHT / 2., middle + CONTROL_HEIGHT / 2.);
    match spec.control {
        Control::Toggle => Part::Switch {
            area: Area::new(
                right - SWITCH_WIDTH,
                middle - SWITCH_HEIGHT / 2.,
                right,
                middle + SWITCH_HEIGHT / 2.,
            ),
            on: value != 0,
        },
        Control::Stepper { min, max, unit, .. } => {
            let plus = Area::new(right - STEP_BUTTON, top, right, bottom);
            let value_area = Area::new(plus.left - STEP_VALUE, top, plus.left, bottom);
            let minus = Area::new(value_area.left - STEP_BUTTON, top, value_area.left, bottom);
            Part::Stepper {
                minus,
                value: value_area,
                plus,
                text: format!("{value} {unit}"),
                can_down: value > min,
                can_up: value < max,
            }
        }
        Control::Choice(choices) => {
            let left = right - choices.len() as f32 * SEGMENT;
            Part::Segments {
                options: choices
                    .iter()
                    .enumerate()
                    .map(|(i, choice)| {
                        let l = left + i as f32 * SEGMENT;
                        (Area::new(l, top, l + SEGMENT, bottom), choice.label)
                    })
                    .collect(),
                selected: value as usize,
            }
        }
    }
}

/// Where the row's label and help text go: from the card's left edge to its control.
pub fn text_area(row: &Row) -> Area {
    let control_left = match &row.part {
        Part::Switch { area, .. } => area.left,
        Part::Stepper { minus, .. } => minus.left,
        Part::Segments { options, .. } => options.first().map_or(row.card.right, |o| o.0.left),
    };
    Area::new(
        row.card.left
            + if row.detail.is_some() {
                MONITOR_INSET
            } else {
                CONTROL_INSET
            },
        row.card.top,
        control_left - CONTROL_INSET,
        row.card.bottom,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(height: i32) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 2400,
            height,
        }
    }

    fn center(area: Area, scale: f32) -> (i32, i32) {
        (
            ((area.left + area.right) / 2. * scale) as i32,
            (area.middle() * scale) as i32,
        )
    }

    /// The state with the Dock page open, which has every kind of control.
    fn dock_page() -> PanelState {
        PanelState {
            page: SECTIONS.iter().position(|&s| s == Section::Dock).unwrap(),
            ..PanelState::default()
        }
    }

    #[test]
    fn every_control_hits_its_own_setting_at_each_scale() {
        let settings = Settings::default();
        for dpi in [96, 120, 144, 192] {
            for page in 0..SECTIONS.len() {
                let mut state = PanelState {
                    page,
                    ..PanelState::default()
                };
                let layout = PanelLayout::new(&mut state, &settings, dpi, work(4000));
                let s = layout.scale;
                let (x, y) = center(layout.nav[page].0, s);
                assert_eq!(layout.hit(x, y), Some(Action::Page(page)));
                let (x, y) = center(layout.close, s);
                assert_eq!(layout.hit(x, y), Some(Action::Close));
                let (x, y) = center(layout.reset, s);
                assert_eq!(layout.hit(x, y), Some(Action::Reset));
                for row in &layout.rows {
                    let change = |adjust| Some(Action::Change(row.key, adjust));
                    match &row.part {
                        Part::Switch { area, .. } => {
                            let (x, y) = center(*area, s);
                            assert_eq!(layout.hit(x, y), change(Adjust::Toggle));
                        }
                        Part::Stepper {
                            minus, plus, value, ..
                        } => {
                            let (x, y) = center(*minus, s);
                            assert_eq!(layout.hit(x, y), change(Adjust::Step(-1)));
                            let (x, y) = center(*plus, s);
                            assert_eq!(layout.hit(x, y), change(Adjust::Step(1)));
                            let (x, y) = center(*value, s);
                            assert_eq!(layout.hit(x, y), None);
                        }
                        Part::Segments { options, .. } => {
                            for (i, (area, _)) in options.iter().enumerate() {
                                let (x, y) = center(*area, s);
                                assert_eq!(layout.hit(x, y), change(Adjust::Pick(i)));
                            }
                        }
                    }
                    assert!(text_area(row).right > text_area(row).left + 120.);
                }
                // The window corner and the caption do nothing.
                assert_eq!(layout.hit(1, 1), None);
            }
        }
    }

    #[test]
    fn limits_disable_stepper_buttons() {
        let mut settings = Settings::default();
        settings.set(Key::IconSize, 56);
        let layout = PanelLayout::new(&mut dock_page(), &settings, 96, work(4000));
        let row = layout.rows.iter().find(|r| r.key == Key::IconSize).unwrap();
        let Part::Stepper {
            plus, minus, text, ..
        } = &row.part
        else {
            panic!("icon size is a stepper");
        };
        assert_eq!(text, "56 px");
        let (x, y) = center(*plus, 1.);
        assert_eq!(layout.hit(x, y), None);
        let (x, y) = center(*minus, 1.);
        assert!(layout.hit(x, y).is_some());
    }

    #[test]
    fn short_work_area_scrolls_the_focused_row_into_view() {
        let settings = Settings::default();
        let mut state = dock_page();
        // At 2x, 520 pixels leave the minimum height, too short for every card.
        let work = Rect {
            width: 1280,
            ..work(520)
        };
        let layout = PanelLayout::new(&mut state, &settings, 192, work);
        assert_eq!(layout.height, MIN_HEIGHT);
        assert_eq!(layout.pixels().0, work.width);
        let caption = layout.title;
        assert_eq!(layout.scroll_thumb.unwrap().top, layout.view.top);
        assert!(layout.max_scroll > 0.);
        let last = layout.rows.len() - 1;
        assert_eq!(state.key(Nav::Previous, &layout), None);
        assert_eq!(state.focus, Some(last));
        let layout = PanelLayout::new(&mut state, &settings, 192, work);
        assert!(layout.rows[last].card.bottom <= layout.view.bottom + 0.01);
        assert_eq!(
            state.key(Nav::Increase, &layout),
            Some(Action::Change(layout.rows[last].key, Adjust::Step(1)))
        );
        // Wheel scrolling stops at both ends.
        assert!(state.scroll_by(120 * 20, &layout));
        assert_eq!(state.scroll, 0.);
        assert!(!state.scroll_by(120, &layout));
        assert!(state.scroll_by(-120 * 20, &layout));
        assert_eq!(state.scroll, layout.max_scroll);

        // Rows hidden under the caption or footer are not clickable.
        let layout = PanelLayout::new(&mut state, &settings, 192, work);
        assert_eq!(caption, layout.title);
        assert!((layout.scroll_thumb.unwrap().bottom - layout.view.bottom).abs() < 0.01);
        let first = layout.rows[0].card;
        assert!(first.bottom < layout.view.top);
        assert_eq!(layout.hit(800, (first.middle() * 2.) as i32), None);
    }

    #[test]
    fn pages_wrap_and_reset_focus() {
        let settings = Settings::default();
        let mut state = PanelState::default();
        let layout = PanelLayout::new(&mut state, &settings, 96, work(4000));
        state.key(Nav::Next, &layout);
        assert_eq!(state.focus, Some(0));
        let last = SECTIONS.len() - 1;
        assert_eq!(
            state.key(Nav::PreviousPage, &layout),
            Some(Action::Page(last))
        );
        state.open_page(last);
        assert_eq!(state.focus, None);
        let layout = PanelLayout::new(&mut state, &settings, 96, work(4000));
        assert_eq!(state.key(Nav::NextPage, &layout), Some(Action::Page(0)));
        assert_eq!(state.key(Nav::Activate, &layout), None);
    }

    #[test]
    fn monitor_styles_and_gpu_checkbox_adjust_their_own_values() {
        let mut state = PanelState::default();
        state.open_page(
            SECTIONS
                .iter()
                .position(|&s| s == Section::Monitor)
                .unwrap(),
        );
        let layout = PanelLayout::new(&mut state, &Settings::default(), 144, work(2000));
        for (index, row) in layout.rows.iter().enumerate() {
            let Some(detail) = &row.detail else { continue };
            let action_at = |area: Area| {
                let (x, y) = center(area, layout.scale);
                layout.hit(x, y)
            };
            match &detail.part {
                Part::Segments { options, .. } => {
                    for (i, &(area, _)) in options.iter().enumerate() {
                        assert_eq!(
                            action_at(area),
                            Some(Action::Change(detail.key, Adjust::Pick(i)))
                        );
                    }
                    let (_, y) = center(options[0].0, layout.scale);
                    let gap = ((options[0].0.right + options[1].0.left) / 2. * layout.scale) as i32;
                    assert_eq!(layout.hit(gap, y), None);
                }
                Part::Switch { area, .. } => {
                    assert_eq!(detail.key, Key::Preview(PreviewKey::GpuTemperature));
                    let toggle = Some(Action::Change(detail.key, Adjust::Toggle));
                    assert_eq!(action_at(*area), toggle);
                    assert_eq!(action_at(detail.label_area.unwrap()), toggle);
                }
                _ => panic!("unexpected monitor control"),
            }
            state.focus = Some(index);
            assert_eq!(
                state.key(Nav::Activate, &layout),
                Some(Action::Change(row.key, Adjust::Toggle))
            );
            assert_eq!(
                state.key(Nav::Increase, &layout),
                Some(Action::Change(detail.key, Adjust::Step(1)))
            );
        }
    }
}
