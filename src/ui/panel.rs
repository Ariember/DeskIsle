//! The settings panel: a page per [`settings::Section`] built from the settings schema,
//! its layout in DIPs, pointer hits and keyboard navigation. Drawing lives in the
//! renderer and the window in the platform layer.

use crate::geometry::Rect;
use crate::settings::{
    self, Adjust, Control, DOCK_DISPLAY_MODES, Key, MonitorKey, SCHEMA, SECTIONS, Section, Settings,
};
use crate::visibility::DockDisplayMode;

pub const WIDTH: f32 = 720.;
pub const HEIGHT: f32 = 560.;
const MIN_WIDTH: f32 = 600.;
const MIN_HEIGHT: f32 = 260.;
pub const CAPTION: f32 = 52.;
/// The window buttons sit at the caption's top left, as in macOS: close, then the
/// minimize and zoom lights the panel draws disabled. This is the close light's center.
const LIGHT_X: f32 = 20.;
/// Distance between the centers of neighboring window lights.
pub const LIGHT_GAP: f32 = 20.;
/// Half the close light's hit square, a little larger than the 12 DIP light.
const LIGHT_HIT: f32 = 8.;
const NAV_LEFT: f32 = 10.;
const NAV_RIGHT: f32 = 192.;
/// The sidebar's right edge, where the content area begins.
pub const SIDEBAR: f32 = NAV_RIGHT + 8.;
const NAV_ITEM: f32 = 32.;
const NAV_GAP: f32 = 2.;
const CONTENT_LEFT: f32 = 220.;
const CONTENT_RIGHT: f32 = 24.;
const FOOTER: f32 = 56.;
/// Standard switch and segmented-control rows.
const CARD: f32 = 60.;
const MONITOR_CARD: f32 = 76.;
const MONITOR_INSET: f32 = 72.;
const CONTROL_INSET: f32 = 16.;
const SWITCH_WIDTH: f32 = 40.;
const SWITCH_HEIGHT: f32 = 22.;
const GROUP_GAP: f32 = 14.;
const SLIDER_CARD: f32 = 86.;
const SLIDER_PAD: f32 = 7.;
const SEGMENT: f32 = 56.;
const CONTROL_HEIGHT: f32 = 28.;
const MODE_ROW_HEIGHT: f32 = 24.;
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

    pub(super) fn contains(&self, x: f32, y: f32) -> bool {
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
    Open(&'static str),
    Quit,
    OpenModes,
    DismissModes,
    Change(Key, Adjust),
}

pub const DISPLAY_SETTINGS: Action = Action::Open("ms-settings:display-advanced");

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectLink {
    pub title: &'static str,
    pub action: Action,
    pub row: Area,
    pub button: Area,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AboutPage {
    pub heading: Area,
    pub links: [ProjectLink; 2],
}

impl AboutPage {
    fn new(view: Area) -> Self {
        // Leave both link rows visible even at the minimum panel height.
        let row_height = 44.;
        let heading_bottom = (view.top + 92.).min(view.bottom - 2. * row_height);
        let heading = Area {
            bottom: heading_bottom,
            ..view
        };
        let links = [
            ("GitHub 项目主页", env!("CARGO_PKG_REPOSITORY")),
            (
                "版本发布",
                concat!(env!("CARGO_PKG_REPOSITORY"), "/releases"),
            ),
        ];
        let links = std::array::from_fn(|i| {
            let (title, uri) = links[i];
            let top = heading_bottom + i as f32 * row_height;
            let row = Area::new(view.left, top, view.right, top + row_height);
            let button = Area::new(
                row.right - 88.,
                row.middle() - CONTROL_HEIGHT / 2.,
                row.right - CONTROL_INSET,
                row.middle() + CONTROL_HEIGHT / 2.,
            );
            ProjectLink {
                title,
                action: Action::Open(uri),
                row,
                button,
            }
        });
        Self { heading, links }
    }

    fn action(&self, index: usize) -> Option<Action> {
        if index == self.links.len() {
            Some(Action::Quit)
        } else {
            self.links.get(index).map(|link| link.action)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Dropdown {
        area: Area,
        selected: usize,
    },
    Switch {
        area: Area,
        on: bool,
    },
    Slider {
        area: Area,
        min: i32,
        max: i32,
        step: i32,
        value: i32,
        unit: &'static str,
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
    /// Index in the current page's keyboard focus order.
    pub focus: Option<usize>,
    pub hover: Option<Action>,
    /// The value shown while dragging, committed by the coordinator on release.
    pub slider_preview: Option<(Key, i32)>,
    /// Keyboard highlight while the Dock mode selector is open.
    pub mode_highlight: Option<usize>,
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
        let page = |offset: usize| Action::Page((self.page + offset) % SECTIONS.len());
        match nav {
            Nav::NextPage => return Some(page(1)),
            Nav::PreviousPage => return Some(page(SECTIONS.len() - 1)),
            _ => {}
        }
        if let Some(current) = self.mode_highlight {
            self.hover = None;
            let last = DOCK_DISPLAY_MODES.len() - 1;
            self.mode_highlight = Some(match nav {
                Nav::Next | Nav::Increase => (current + 1).min(last),
                Nav::Previous | Nav::Decrease => current.saturating_sub(1),
                Nav::Activate => {
                    return Some(Action::Change(Key::DockDisplayMode, Adjust::Pick(current)));
                }
                Nav::Close => return Some(Action::DismissModes),
                _ => return None,
            });
            return None;
        }
        let count = layout.focus_count();
        let focused = self.focus.filter(|&i| i < count);
        match nav {
            Nav::Close => Some(Action::Close),
            Nav::Next | Nav::Previous if count > 0 => {
                let next = match (focused, nav == Nav::Next) {
                    (None, true) => 0,
                    (None, false) => count - 1,
                    (Some(i), true) => (i + 1) % count,
                    (Some(i), false) => (i + count - 1) % count,
                };
                self.focus = Some(next);
                // Bring the focused card fully into view.
                let card = layout.rows.get(next)?.card;
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
                if let Some(about) = &layout.about {
                    return if nav == Nav::Activate {
                        about.action(focused?)
                    } else {
                        None
                    };
                }
                let Some(row) = layout.rows.get(focused?) else {
                    return (nav == Nav::Activate).then_some(DISPLAY_SETTINGS);
                };
                if matches!(row.part, Part::Dropdown { .. }) && nav == Nav::Activate {
                    return Some(Action::OpenModes);
                }
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

/// Keep the navigation and controls on screen even when both scaling factors are high.
pub fn panel_dpi(requested: u32, work: Rect) -> u32 {
    let fit = (i64::from(work.width) * 96 / MIN_WIDTH as i64)
        .min(i64::from(work.height) * 96 / MIN_HEIGHT as i64);
    requested.min(fit.max(1) as u32)
}

/// The panel's geometry in DIPs for the current page.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    /// The close light's hit square; the other window lights follow at `LIGHT_GAP`.
    pub close: Area,
    pub nav: Vec<(Area, Section)>,
    pub page: usize,
    /// The page title in the fixed caption.
    pub title: Area,
    /// Content scrolls inside this area and is clipped to it.
    pub view: Area,
    pub rows: Vec<Row>,
    pub groups: Vec<Area>,
    pub backdrop: i32,
    pub footer_button: Area,
    pub about: Option<AboutPage>,
    pub display_settings: Option<Area>,
    pub max_scroll: f32,
    pub scroll_thumb: Option<Area>,
    pub focus: Option<usize>,
    pub hover: Option<Action>,
    pub modes: Option<ModeList>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModeList {
    pub area: Area,
    pub selected: usize,
    pub highlight: usize,
}

impl ModeList {
    pub fn rows(&self) -> impl Iterator<Item = (Area, &'static str)> {
        DOCK_DISPLAY_MODES.iter().enumerate().map(|(index, mode)| {
            let top = self.area.top + MODE_ROW_HEIGHT * index as f32;
            (
                Area {
                    top,
                    bottom: top + MODE_ROW_HEIGHT,
                    ..self.area
                },
                mode.label,
            )
        })
    }
}

impl PanelLayout {
    fn focus_count(&self) -> usize {
        self.about.as_ref().map_or_else(
            || self.rows.len() + usize::from(self.display_settings.is_some()),
            |about| about.links.len() + 1,
        )
    }

    /// Fits the window to its monitor's work area and clamps the current page's scroll.
    pub fn new(state: &mut PanelState, settings: &Settings, dpi: u32, work: Rect) -> Self {
        let scale = dpi as f32 / 96.;
        let width = (work.width as f32 / scale).clamp(MIN_WIDTH, WIDTH).floor();
        let height = (work.height as f32 / scale)
            .clamp(MIN_HEIGHT, HEIGHT)
            .floor();
        let mid = CAPTION / 2.;
        let close = Area::new(
            LIGHT_X - LIGHT_HIT,
            mid - LIGHT_HIT,
            LIGHT_X + LIGHT_HIT,
            mid + LIGHT_HIT,
        );
        let nav = SECTIONS
            .iter()
            .enumerate()
            .map(|(i, &section)| {
                let top = CAPTION + 4. + i as f32 * (NAV_ITEM + NAV_GAP);
                (Area::new(NAV_LEFT, top, NAV_RIGHT, top + NAV_ITEM), section)
            })
            .collect();
        state.page = state.page.min(SECTIONS.len() - 1);
        let section = SECTIONS[state.page];
        let view = Area::new(
            CONTENT_LEFT,
            CAPTION + 8.,
            width - CONTENT_RIGHT,
            height - FOOTER,
        );
        let mut rows = Vec::new();
        let mut groups: Vec<Area> = Vec::new();
        let mut top = view.top;
        for spec in SCHEMA
            .iter()
            .filter(|s| s.section == section && detail_parent(s.key).is_none())
        {
            let detail = SCHEMA
                .iter()
                .find(|s| detail_parent(s.key) == Some(spec.key));
            let starts_group = rows.is_empty()
                || matches!(
                    spec.key,
                    Key::InterfaceScale
                        | Key::Theme
                        | Key::ConfirmPowerActions
                        | Key::HideTaskbar
                        | Key::TemperatureUnit
                        | Key::FontFamily
                        | Key::HoverZoom
                );
            if starts_group && !rows.is_empty() {
                top += GROUP_GAP;
            }
            let height = match spec.key {
                Key::Theme => 122.,
                Key::TopBarBackground => 144.,
                key if stacked_choice(key) => SLIDER_CARD,
                _ if matches!(spec.control, Control::Slider { .. }) => SLIDER_CARD,
                _ if detail.is_some() => MONITOR_CARD,
                _ => CARD,
            };
            let card = Area::new(view.left, top, view.right, top + height);
            if starts_group {
                groups.push(card);
            } else if let Some(group) = groups.last_mut() {
                group.bottom = card.bottom;
            }
            rows.push((spec, detail, card));
            top = card.bottom;
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
        let general = section == Section::General;
        for group in &mut groups {
            group.top -= state.scroll;
            group.bottom -= state.scroll;
        }
        let rows: Vec<Row> = rows
            .into_iter()
            .map(|(spec, detail, mut card)| {
                card.top -= state.scroll;
                card.bottom -= state.scroll;
                let value = state
                    .slider_preview
                    .filter(|(key, _)| *key == spec.key)
                    .map_or_else(|| settings.get(spec.key), |(_, value)| value);
                Row {
                    key: spec.key,
                    label: spec.label,
                    help: if spec.key == Key::DockDisplayMode {
                        match settings.dock_display_mode() {
                            DockDisplayMode::Default => spec.help,
                            DockDisplayMode::Overlay => "允许其他窗口遮挡 Dock",
                            DockDisplayMode::AutoHide => "移开收起，停在主屏底边任意位置唤出",
                            DockDisplayMode::SmartHide => "与前台窗口重叠时收起",
                            DockDisplayMode::Always => "切换应用后保持置顶",
                            DockDisplayMode::Hotkey => "Ctrl+Alt+D 显示或隐藏 Dock",
                            DockDisplayMode::AlwaysIncludingFullscreen => "全屏时也保持置顶",
                            DockDisplayMode::Desktop => "仅在桌面处于前台时显示",
                        }
                    } else {
                        spec.help
                    },
                    card,
                    part: part(spec, value, card),
                    detail: detail.map(|s| detail_control(s, settings.get(s.key), card)),
                }
            })
            .collect();
        let modes = state.mode_highlight.and_then(|highlight| {
            let (button, selected) = rows.iter().find_map(|row| match row.part {
                Part::Dropdown { area, selected } => Some((area, selected)),
                _ => None,
            })?;
            let list_height = MODE_ROW_HEIGHT * DOCK_DISPLAY_MODES.len() as f32;
            // Align the selected entry with the button and keep every row clickable,
            // including on the shortest panel. The title remains draggable.
            let top = (button.middle() - MODE_ROW_HEIGHT * (selected as f32 + 0.5))
                .clamp(CAPTION + 2., height - list_height - 2.);
            Some(ModeList {
                area: Area::new(button.left, top, button.right, top + list_height),
                selected,
                highlight,
            })
        });
        let reset_top = height - FOOTER + (FOOTER - CONTROL_HEIGHT) / 2.;
        let mut layout = Self {
            scale,
            width,
            height,
            close,
            nav,
            page: state.page,
            title: Area::new(view.left, 0., view.right, CAPTION),
            view,
            rows,
            groups,
            backdrop: settings.get(Key::Backdrop),
            display_settings: general.then(|| {
                Area::new(
                    view.left,
                    reset_top,
                    view.left + 132.,
                    reset_top + CONTROL_HEIGHT,
                )
            }),
            about: (section == Section::About).then(|| AboutPage::new(view)),
            footer_button: Area::new(
                if section == Section::About {
                    view.left
                } else {
                    view.right - RESET_WIDTH
                },
                reset_top,
                view.right,
                reset_top + CONTROL_HEIGHT,
            ),
            max_scroll,
            scroll_thumb,
            focus: state.focus,
            hover: state.hover,
            modes,
        };
        state.focus = state.focus.filter(|&i| i < layout.focus_count());
        layout.focus = state.focus;
        layout
    }

    /// The window size in physical pixels.
    pub fn pixels(&self) -> (i32, i32) {
        (
            (self.width * self.scale).round() as i32,
            (self.height * self.scale).round() as i32,
        )
    }

    /// The draggable caption: its height and the left and right edges of the close
    /// light, which stays clickable, in pixels.
    pub fn caption_pixels(&self) -> (i32, i32, i32) {
        (
            (CAPTION * self.scale).round() as i32,
            (self.close.left * self.scale).round() as i32,
            (self.close.right * self.scale).round() as i32,
        )
    }

    /// The sidebar's width in client pixels, where the window shows its backdrop.
    pub fn sidebar_pixels(&self) -> i32 {
        (SIDEBAR * self.scale).round() as i32
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

    /// Begins a slider gesture only on its visible hit target.
    pub fn slider_at(&self, x: i32, y: i32) -> Option<Key> {
        match self.hit(x, y) {
            Some(Action::Change(key, Adjust::Set(_))) => Some(key),
            _ => None,
        }
    }

    /// Continues a captured gesture, including outside the track or window.
    pub fn slider_action(&self, key: Key, x: i32) -> Option<Action> {
        let row = self.rows.iter().find(|row| row.key == key)?;
        self.slider_action_dip(row, x as f32 / self.scale)
    }

    fn slider_action_dip(&self, row: &Row, x: f32) -> Option<Action> {
        let Part::Slider {
            area,
            min,
            max,
            step,
            ..
        } = row.part
        else {
            return None;
        };
        let (left, right) = slider_track(area);
        let fraction = ((x - left) / (right - left)).clamp(0., 1.);
        let steps = (max - min) / step;
        Some(Action::Change(
            row.key,
            Adjust::Set(min + (fraction * steps as f32).round() as i32 * step),
        ))
    }

    /// `x` and `y` are client pixels of the panel window.
    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        if let Some(modes) = &self.modes {
            return Some(
                modes
                    .rows()
                    .position(|(a, _)| a.contains(x, y))
                    .map_or(Action::DismissModes, |i| {
                        Action::Change(Key::DockDisplayMode, Adjust::Pick(i))
                    }),
            );
        }
        if self.close.contains(x, y) {
            return Some(Action::Close);
        }
        if let Some(page) = self.nav.iter().position(|(area, _)| area.contains(x, y)) {
            return Some(Action::Page(page));
        }
        if self
            .display_settings
            .is_some_and(|area| area.contains(x, y))
        {
            return Some(DISPLAY_SETTINGS);
        }
        if self.footer_button.contains(x, y) {
            return Some(if self.about.is_some() {
                Action::Quit
            } else {
                Action::Reset
            });
        }
        if let Some(about) = &self.about {
            return about
                .links
                .iter()
                .find(|link| link.button.contains(x, y))
                .map(|link| link.action);
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
            Part::Dropdown { area, .. } => return area.contains(x, y).then_some(Action::OpenModes),
            // In composite cards the space around the style buttons is neutral.
            Part::Switch { area, .. }
                if row.detail.is_some() && !area.contains(x, y) && y >= row.card.middle() =>
            {
                return None;
            }
            // The whole card flips a standalone switch, as in Windows Settings.
            Part::Switch { .. } => Adjust::Toggle,
            Part::Slider { area, .. } => {
                if area.contains(x, y) {
                    return self.slider_action_dip(row, x);
                }
                return None;
            }
            Part::Segments { options, .. } => {
                Adjust::Pick(options.iter().position(|(area, _)| area.contains(x, y))?)
            }
        };
        Some(Action::Change(row.key, adjust))
    }
}

/// The thumb stays entirely inside the clickable area at both limits.
pub fn slider_track(area: Area) -> (f32, f32) {
    (area.left + SLIDER_PAD, area.right - SLIDER_PAD)
}

pub fn stacked_choice(key: Key) -> bool {
    matches!(
        key,
        Key::Theme
            | Key::TopBarBackground
            | Key::Accent
            | Key::FontFamily
            | Key::TaskbarShortcut
            | Key::Language
    )
}

/// These controls share a card with the monitor they configure.
fn detail_parent(key: Key) -> Option<Key> {
    use MonitorKey::*;
    Some(Key::Monitor(match key {
        Key::Monitor(CpuStyle) => Cpu,
        Key::Monitor(GpuTemperature) => Temperature,
        Key::Monitor(MemoryStyle) => Memory,
        Key::Monitor(DiskStyle) => Disk,
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
        Control::Slider { .. } => unreachable!("monitor details are toggles or styles"),
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
        Control::Slider {
            min,
            max,
            step,
            unit,
        } => Part::Slider {
            area: Area::new(
                card.left + CONTROL_INSET,
                card.bottom - 33.,
                right,
                card.bottom - 9.,
            ),
            min,
            max,
            step,
            value,
            unit,
        },
        Control::Choice(_) if spec.key == Key::DockDisplayMode => Part::Dropdown {
            area: Area::new(right - 166., top, right, bottom),
            selected: value as usize,
        },
        Control::Choice(choices) => {
            let (left, top, segment, bottom) = if stacked_choice(spec.key) {
                let left = card.left + CONTROL_INSET;
                let height = match spec.key {
                    Key::Theme => 64.,
                    Key::TopBarBackground => 86.,
                    _ => CONTROL_HEIGHT,
                };
                (
                    left,
                    card.bottom - height - 9.,
                    (right - left) / choices.len() as f32,
                    card.bottom - 9.,
                )
            } else {
                (right - choices.len() as f32 * SEGMENT, top, SEGMENT, bottom)
            };
            // Font and shortcut names have very different lengths. Give the long names
            // room even on the narrowest panel, leaving the other controls evenly spaced.
            let weight = |label: &str| -> f32 {
                if matches!(spec.key, Key::FontFamily | Key::TaskbarShortcut) {
                    4. + label
                        .chars()
                        .map(|c| if c.is_ascii() { 1. } else { 2. })
                        .sum::<f32>()
                } else {
                    1.
                }
            };
            let total = choices
                .iter()
                .map(|choice| weight(choice.label))
                .sum::<f32>();
            let mut x = left;
            Part::Segments {
                options: choices
                    .iter()
                    .map(|choice| {
                        let width = segment * choices.len() as f32 * weight(choice.label) / total;
                        let area = Area::new(x, top, x + width, bottom);
                        x = area.right;
                        (area, choice.label)
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
        Part::Switch { area, .. } | Part::Dropdown { area, .. } => area.left,
        Part::Slider { .. } => row.card.right - 78.,
        Part::Segments { .. } if stacked_choice(row.key) => row.card.right,
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
            for (page, section) in SECTIONS.iter().enumerate() {
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
                // The window must not treat the close light as part of the drag caption.
                let (caption, left, right) = layout.caption_pixels();
                assert!(y < caption && (left..right).contains(&x));
                assert!(layout.nav[0].0.top >= CAPTION);
                // The backdrop region covers the whole sidebar and none of the content.
                assert!(layout.nav[page].0.right < SIDEBAR && layout.view.left > SIDEBAR);
                let (x, y) = center(layout.footer_button, s);
                let footer_action = if *section == Section::About {
                    Action::Quit
                } else {
                    Action::Reset
                };
                assert_eq!(layout.hit(x, y), Some(footer_action));
                for index in 0..layout.rows.len() {
                    let current = PanelLayout::new(&mut state, &settings, dpi, work(4000));
                    state.key(Nav::Next, &current);
                    let layout = PanelLayout::new(&mut state, &settings, dpi, work(4000));
                    let row = &layout.rows[index];
                    let change = |adjust| Some(Action::Change(row.key, adjust));
                    match &row.part {
                        Part::Dropdown { area, .. } => {
                            let (x, y) = center(*area, s);
                            assert_eq!(layout.hit(x, y), Some(Action::OpenModes));
                            assert_eq!(state.key(Nav::Activate, &layout), Some(Action::OpenModes));
                        }
                        Part::Switch { area, .. } => {
                            let (x, y) = center(*area, s);
                            assert_eq!(layout.hit(x, y), change(Adjust::Toggle));
                        }
                        Part::Slider { area, min, max, .. } => {
                            let (_, y) = center(*area, s);
                            let left = ((area.left + 1.) * s).round() as i32;
                            let right = ((area.right - 1.) * s).round() as i32;
                            assert_eq!(layout.slider_at(left, y), Some(row.key));
                            assert_eq!(layout.hit(left, y), change(Adjust::Set(*min)));
                            assert_eq!(layout.hit(right, y), change(Adjust::Set(*max)));
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
                if let Some(button) = layout.display_settings {
                    let (x, y) = center(button, s);
                    assert_eq!(layout.hit(x, y), Some(DISPLAY_SETTINGS));
                    state.key(Nav::Next, &layout);
                    let layout = PanelLayout::new(&mut state, &settings, dpi, work(4000));
                    assert_eq!(state.key(Nav::Activate, &layout), Some(DISPLAY_SETTINGS));
                }
                // The window corner and the caption do nothing.
                assert_eq!(layout.hit(1, 1), None);
            }
        }
    }

    #[test]
    fn about_links_and_quit_work_with_pointer_and_keyboard_on_short_panels() {
        let settings = Settings::default();
        for dpi in [96, 120, 144, 192] {
            let work = work((MIN_HEIGHT * dpi as f32 / 96.) as i32);
            let mut state = PanelState::default();
            let layout = PanelLayout::new(&mut state, &settings, dpi, work);
            let (page, (tab, _)) = layout
                .nav
                .iter()
                .enumerate()
                .find(|(_, (_, section))| *section == Section::About)
                .unwrap();
            let (x, y) = center(*tab, layout.scale);
            assert!(tab.bottom <= layout.height);
            assert_eq!(layout.hit(x, y), Some(Action::Page(page)));
            state.open_page(page);
            let layout = PanelLayout::new(&mut state, &settings, dpi, work);
            let about = layout.about.as_ref().unwrap();
            for (area, expected) in [
                (
                    about.links[0].button,
                    Action::Open("https://github.com/Ariember/DeskIsle"),
                ),
                (
                    about.links[1].button,
                    Action::Open("https://github.com/Ariember/DeskIsle/releases"),
                ),
                (layout.footer_button, Action::Quit),
            ] {
                assert!(area.top >= layout.view.top && area.bottom <= layout.height);
                let (x, y) = center(area, layout.scale);
                assert_eq!(layout.hit(x, y), Some(expected));
                state.key(Nav::Next, &layout);
                let focused = PanelLayout::new(&mut state, &settings, dpi, work);
                assert_eq!(state.key(Nav::Activate, &focused), Some(expected));
                assert_eq!(state.key(Nav::Increase, &focused), None);
            }
            assert_eq!(state.key(Nav::Close, &layout), Some(Action::Close));
        }
    }

    #[test]
    fn dock_mode_list_fits_short_panels_and_commits_only_on_selection() {
        let mut settings = Settings::default();
        let mut state = dock_page();
        let work = Rect {
            width: 1200,
            ..work(520)
        };
        for selected in 0..DOCK_DISPLAY_MODES.len() {
            settings.adjust(Key::DockDisplayMode, Adjust::Pick(selected));
            state.mode_highlight = Some(selected);
            let layout = PanelLayout::new(&mut state, &settings, 192, work);
            let modes = layout.modes.as_ref().unwrap();
            assert_eq!(modes.selected, selected);
            assert!(modes.area.top >= CAPTION && modes.area.bottom <= layout.height);
            for (index, (area, _)) in modes.rows().enumerate() {
                let (x, y) = center(area, layout.scale);
                assert_eq!(
                    layout.hit(x, y),
                    Some(Action::Change(Key::DockDisplayMode, Adjust::Pick(index)))
                );
            }
            assert_eq!(layout.hit(1, 1), Some(Action::DismissModes));
            assert_eq!(state.key(Nav::Previous, &layout), None);
            assert_eq!(
                state.key(Nav::Activate, &layout),
                Some(Action::Change(
                    Key::DockDisplayMode,
                    Adjust::Pick(selected.saturating_sub(1))
                ))
            );
            assert_eq!(state.key(Nav::Close, &layout), Some(Action::DismissModes));
        }
    }

    #[test]
    fn captured_slider_snaps_to_steps_and_previews_without_changing_settings() {
        let settings = Settings::default();
        let mut state = dock_page();
        state.slider_preview = Some((Key::IconSize, 46));
        let layout = PanelLayout::new(&mut state, &settings, 144, work(4000));
        let row = layout
            .rows
            .iter()
            .find(|row| row.key == Key::IconSize)
            .unwrap();
        let Part::Slider { area, value, .. } = row.part else {
            panic!("expected icon size slider")
        };
        assert_eq!(value, 46);
        assert_eq!(settings.icon_size(), 34);
        let (left, right) = slider_track(area);
        let x = ((left + 0.342 * (right - left)) * layout.scale).round() as i32;
        let set = |n| Some(Action::Change(Key::IconSize, Adjust::Set(n)));
        assert_eq!(layout.slider_action(Key::IconSize, x), set(34));
        assert_eq!(layout.slider_action(Key::IconSize, -200), set(24));
        assert_eq!(layout.slider_action(Key::IconSize, i32::MAX), set(56));
        assert_eq!(layout.slider_at(x, 0), None);
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
        assert!(first.top < layout.view.top);
        let hidden = (first.top + layout.view.top) / 2.;
        assert_eq!(layout.hit(800, (hidden * 2.) as i32), None);
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
                    assert_eq!(detail.key, Key::Monitor(MonitorKey::GpuTemperature));
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
