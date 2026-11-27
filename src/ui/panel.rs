//! The settings panel: a page per [`settings::Section`] built from the settings schema,
//! its layout in DIPs, pointer hits and keyboard navigation. Drawing lives in the
//! renderer and the window in the platform layer.

use crate::geometry::Rect;
use crate::settings::{
    self, Adjust, Control, DOCK_DISPLAY_MODES, Key, MonitorKey, SCHEMA, SECTIONS, Section, Settings,
};
use crate::visibility::DockDisplayMode;
use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

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
const PLUGIN_HERO_CARD: f32 = 68.;
const INSTALLED_ROW: f32 = 68.;
const MARKET_ROW: f32 = 76.;
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
    OpenPluginsDirectory,
    PluginTab(PluginTab),
    BrowseMarket,
    InstallPlugin(usize),
    RemovePlugin(usize),
    CheckUpdates,
    Quit,
    OpenModes,
    DismissModes,
    Change(Key, Adjust),
}

pub const DISPLAY_SETTINGS: Action = Action::Open("ms-settings:display-advanced");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginTab {
    #[default]
    All,
    Market,
    Installed,
}

impl PluginTab {
    pub const ALL: [Self; 3] = [Self::All, Self::Market, Self::Installed];

    pub fn index(self) -> usize {
        match self {
            Self::All => 0,
            Self::Market => 1,
            Self::Installed => 2,
        }
    }

    pub fn step(self, delta: isize) -> Self {
        let count = Self::ALL.len() as isize;
        Self::ALL[(self.index() as isize + delta).rem_euclid(count) as usize]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarketState {
    #[default]
    Install,
    Update,
    Reinstall,
    Installed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketItem {
    pub id: String,
    pub name: String,
    pub version: String,
    pub installed_version: Option<String>,
    pub description: String,
    pub state: MarketState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstalledHealth {
    #[default]
    Ready,
    Setup,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledItem {
    pub id: String,
    pub title: String,
    pub version: Option<String>,
    pub market_update: Option<(usize, String)>,
    pub summary: String,
    pub cards: usize,
    pub health: InstalledHealth,
    pub removable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketView {
    pub installed: Arc<[InstalledItem]>,
    pub items: Arc<[MarketItem]>,
    pub source: Option<String>,
    pub error: Option<String>,
    pub loaded: bool,
    pub busy: Option<Action>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFocus {
    Tabs,
    Refresh,
    EmptyBrowseMarket,
    InstalledUpdate(usize),
    InstalledRemove(usize),
    MarketItem(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PluginStop {
    pub kind: PluginFocus,
    pub scroll_area: Area,
    pub ring_area: Area,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstalledRow {
    pub index: usize,
    pub area: Area,
    pub update_button: Option<(Area, usize)>,
    pub remove_button: Option<Area>,
    pub remove_action: Option<Action>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstalledSection {
    pub header: Area,
    pub meta: String,
    pub meta_accent: bool,
    pub group: Area,
    pub empty_button: Option<Area>,
    pub rows: Vec<InstalledRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarketSection {
    pub header: Area,
    pub source_label: String,
    pub error_banner: Option<(Area, Area)>,
    pub group: Area,
    pub state_button: Option<(Area, &'static str)>,
    view: MarketView,
}

impl MarketSection {
    pub fn visible(&self, viewport: Area) -> Range<usize> {
        if self.view.items.is_empty() {
            return 0..0;
        }
        let first = ((viewport.top - self.group.top) / MARKET_ROW)
            .floor()
            .max(0.) as usize;
        let end = ((viewport.bottom - self.group.top) / MARKET_ROW)
            .ceil()
            .max(0.) as usize;
        first.min(self.view.items.len())..end.min(self.view.items.len())
    }

    pub fn row(&self, index: usize) -> Area {
        let top = self.group.top + MARKET_ROW * index as f32;
        Area {
            top,
            bottom: top + MARKET_ROW,
            ..self.group
        }
    }

    pub fn button(&self, index: usize) -> Area {
        let row = self.row(index);
        Area::new(
            row.right - 90.,
            row.middle() - CONTROL_HEIGHT / 2.,
            row.right - CONTROL_INSET,
            row.middle() + CONTROL_HEIGHT / 2.,
        )
    }

    pub fn button_label(&self, index: usize) -> &'static str {
        let item = &self.view.items[index];
        if self.view.busy == Some(Action::InstallPlugin(index)) {
            if item.state == MarketState::Update {
                "更新中…"
            } else {
                "安装中…"
            }
        } else {
            match item.state {
                MarketState::Install => "安装",
                MarketState::Update => "更新",
                MarketState::Reinstall => "重装",
                MarketState::Installed => "已安装",
            }
        }
    }

    pub fn action(&self, index: usize) -> Option<Action> {
        if self.view.busy.is_some() || index >= self.view.items.len() {
            return None;
        }
        (self.view.items[index].state != MarketState::Installed)
            .then_some(Action::InstallPlugin(index))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginsPage {
    pub tab: PluginTab,
    pub tabs_track: Area,
    pub tabs: [(Area, PluginTab); 3],
    pub refresh_button: Area,
    pub refresh_label: &'static str,
    pub refresh_action: Option<Action>,
    pub installed: Option<InstalledSection>,
    pub market: Option<MarketSection>,
    pub stops: Vec<PluginStop>,
    pub view: MarketView,
}

impl PluginsPage {
    fn build(viewport: Area, start_top: f32, tab: PluginTab, view: &MarketView) -> (Self, f32) {
        let bar_top = start_top + 10.;
        let track_width = 228_f32.min((viewport.right - viewport.left - 110.).max(168.));
        let tabs_track = Area::new(
            viewport.left,
            bar_top,
            viewport.left + track_width,
            bar_top + CONTROL_HEIGHT,
        );
        let seg_w = track_width / PluginTab::ALL.len() as f32;
        let tabs = std::array::from_fn(|i| {
            let left = tabs_track.left + i as f32 * seg_w;
            (
                Area::new(left, bar_top, left + seg_w, bar_top + CONTROL_HEIGHT),
                PluginTab::ALL[i],
            )
        });
        let refresh_button = Area::new(
            viewport.right - 98.,
            bar_top,
            viewport.right,
            bar_top + CONTROL_HEIGHT,
        );
        let refresh_label = if view.busy == Some(Action::BrowseMarket) {
            "读取中…"
        } else if view.error.is_some() {
            "重试"
        } else if view.loaded {
            "刷新市场"
        } else {
            "浏览市场"
        };
        let refresh_action = view.busy.is_none().then_some(Action::BrowseMarket);
        let mut cur = bar_top + CONTROL_HEIGHT;
        let mut stops = vec![
            PluginStop {
                kind: PluginFocus::Tabs,
                scroll_area: Area::new(viewport.left, bar_top, viewport.right, cur),
                ring_area: tabs_track,
                enabled: true,
            },
            PluginStop {
                kind: PluginFocus::Refresh,
                scroll_area: Area::new(viewport.left, bar_top, viewport.right, cur),
                ring_area: refresh_button,
                enabled: refresh_action.is_some(),
            },
        ];

        let show_installed =
            tab == PluginTab::Installed || (tab == PluginTab::All && !view.installed.is_empty());
        let installed = show_installed.then(|| {
            let header = Area::new(
                viewport.left + 2.,
                cur + 10.,
                viewport.right - 2.,
                cur + 28.,
            );
            let group_top = header.bottom + 4.;
            let updates = view
                .installed
                .iter()
                .filter(|item| item.market_update.is_some())
                .count();
            let total_cards: usize = view.installed.iter().map(|item| item.cards).sum();
            let (meta, meta_accent) = if updates > 0 {
                (format!("{updates} 款可更新"), true)
            } else if view.installed.is_empty() {
                ("暂无本地插件".into(), false)
            } else {
                (
                    format!("{} 款插件 · {total_cards} 张卡片", view.installed.len()),
                    false,
                )
            };
            if view.installed.is_empty() {
                let group = Area::new(viewport.left, group_top, viewport.right, group_top + 86.);
                let mid_x = (group.left + group.right) / 2.;
                let btn = Area::new(
                    mid_x - 58.,
                    group.bottom - 35.,
                    mid_x + 58.,
                    group.bottom - 7.,
                );
                stops.push(PluginStop {
                    kind: PluginFocus::EmptyBrowseMarket,
                    scroll_area: group,
                    ring_area: btn,
                    enabled: true,
                });
                cur = group.bottom;
                InstalledSection {
                    header,
                    meta,
                    meta_accent,
                    group,
                    empty_button: Some(btn),
                    rows: Vec::new(),
                }
            } else {
                let group = Area::new(
                    viewport.left,
                    group_top,
                    viewport.right,
                    group_top + INSTALLED_ROW * view.installed.len() as f32,
                );
                let mut rows = Vec::with_capacity(view.installed.len());
                for (i, item) in view.installed.iter().enumerate() {
                    let row_top = group_top + i as f32 * INSTALLED_ROW;
                    let area = Area::new(
                        viewport.left,
                        row_top,
                        viewport.right,
                        row_top + INSTALLED_ROW,
                    );
                    let right = area.right - CONTROL_INSET;
                    let mid = area.middle();
                    let remove_button = item.removable.then(|| {
                        let width = if view.busy == Some(Action::RemovePlugin(i)) {
                            70.
                        } else {
                            58.
                        };
                        Area::new(right - width, mid - 14., right, mid + 14.)
                    });
                    let remove_action =
                        (item.removable && view.busy.is_none()).then_some(Action::RemovePlugin(i));
                    let update_button = item.market_update.as_ref().map(|&(market_idx, _)| {
                        let upd_right = remove_button.map_or(right, |btn| btn.left - 8.);
                        let width = if view.busy == Some(Action::InstallPlugin(market_idx)) {
                            70.
                        } else {
                            58.
                        };
                        (
                            Area::new(upd_right - width, mid - 14., upd_right, mid + 14.),
                            market_idx,
                        )
                    });
                    if let Some((btn, market_idx)) = update_button {
                        stops.push(PluginStop {
                            kind: PluginFocus::InstalledUpdate(market_idx),
                            scroll_area: area,
                            ring_area: btn,
                            enabled: view.busy.is_none(),
                        });
                    }
                    if let Some(btn) = remove_button {
                        stops.push(PluginStop {
                            kind: PluginFocus::InstalledRemove(i),
                            scroll_area: area,
                            ring_area: btn,
                            enabled: remove_action.is_some(),
                        });
                    }
                    rows.push(InstalledRow {
                        index: i,
                        area,
                        update_button,
                        remove_button,
                        remove_action,
                    });
                }
                cur = group.bottom;
                InstalledSection {
                    header,
                    meta,
                    meta_accent,
                    group,
                    empty_button: None,
                    rows,
                }
            }
        });

        let show_market = tab == PluginTab::Market || tab == PluginTab::All;
        let market = show_market.then(|| {
            let header = Area::new(
                viewport.left + 2.,
                cur + 12.,
                viewport.right - 2.,
                cur + 30.,
            );
            let mut body_top = header.bottom + 4.;
            let source_label = if view.busy == Some(Action::BrowseMarket) {
                "正在同步市场索引…".into()
            } else if let Some(source) = &view.source {
                format!("{source} · SHA-256 校验")
            } else {
                "GitHub 仓库索引".into()
            };
            let error_banner = (view.error.is_some() && !view.items.is_empty()).then(|| {
                let banner = Area::new(viewport.left, body_top, viewport.right, body_top + 42.);
                let retry = Area::new(
                    banner.right - 72.,
                    banner.middle() - 13.,
                    banner.right - 12.,
                    banner.middle() + 13.,
                );
                body_top = banner.bottom + 8.;
                (banner, retry)
            });
            let (group, state_button) = if view.items.is_empty() {
                let group = Area::new(viewport.left, body_top, viewport.right, body_top + 84.);
                let state_button = (view.busy != Some(Action::BrowseMarket)
                    && (!view.loaded || view.error.is_some()))
                .then(|| {
                    let btn = Area::new(
                        group.right - 92.,
                        group.middle() - 14.,
                        group.right - 16.,
                        group.middle() + 14.,
                    );
                    let label = if view.error.is_some() {
                        "重试"
                    } else {
                        "浏览"
                    };
                    (btn, label)
                });
                (group, state_button)
            } else {
                let group = Area::new(
                    viewport.left,
                    body_top,
                    viewport.right,
                    body_top + MARKET_ROW * view.items.len() as f32,
                );
                (group, None)
            };
            let section = MarketSection {
                header,
                source_label,
                error_banner,
                group,
                state_button,
                view: view.clone(),
            };
            for i in 0..view.items.len() {
                stops.push(PluginStop {
                    kind: PluginFocus::MarketItem(i),
                    scroll_area: section.row(i),
                    ring_area: section.button(i),
                    enabled: section.action(i).is_some(),
                });
            }
            cur = group.bottom;
            section
        });

        (
            Self {
                tab,
                tabs_track,
                tabs,
                refresh_button,
                refresh_label,
                refresh_action,
                installed,
                market,
                stops,
                view: view.clone(),
            },
            cur,
        )
    }

    fn shift(&mut self, scroll: f32) {
        let move_y = |a: &mut Area| {
            a.top -= scroll;
            a.bottom -= scroll;
        };
        move_y(&mut self.tabs_track);
        for (area, _) in &mut self.tabs {
            move_y(area);
        }
        move_y(&mut self.refresh_button);
        if let Some(installed) = &mut self.installed {
            move_y(&mut installed.header);
            move_y(&mut installed.group);
            if let Some(btn) = &mut installed.empty_button {
                move_y(btn);
            }
            for row in &mut installed.rows {
                move_y(&mut row.area);
                if let Some((btn, _)) = &mut row.update_button {
                    move_y(btn);
                }
                if let Some(btn) = &mut row.remove_button {
                    move_y(btn);
                }
            }
        }
        if let Some(market) = &mut self.market {
            move_y(&mut market.header);
            if let Some((banner, retry)) = &mut market.error_banner {
                move_y(banner);
                move_y(retry);
            }
            move_y(&mut market.group);
            if let Some((btn, _)) = &mut market.state_button {
                move_y(btn);
            }
        }
        for stop in &mut self.stops {
            move_y(&mut stop.scroll_area);
            move_y(&mut stop.ring_area);
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Action> {
        if let Some(&(_, tab)) = self.tabs.iter().find(|(a, _)| a.contains(x, y)) {
            return Some(Action::PluginTab(tab));
        }
        if self.refresh_button.contains(x, y) {
            return self.refresh_action;
        }
        if let Some(installed) = &self.installed {
            if let Some(btn) = installed.empty_button
                && btn.contains(x, y)
            {
                return Some(Action::PluginTab(PluginTab::Market));
            }
            if installed.group.contains(x, y)
                && let Some(row) = installed.rows.iter().find(|r| r.area.contains(x, y))
            {
                if let Some((btn, market_idx)) = row.update_button
                    && btn.contains(x, y)
                {
                    return self
                        .view
                        .busy
                        .is_none()
                        .then_some(Action::InstallPlugin(market_idx));
                }
                if let Some(btn) = row.remove_button
                    && btn.contains(x, y)
                {
                    return row.remove_action;
                }
                return None;
            }
        }
        if let Some(market) = &self.market {
            if let Some((_, retry)) = market.error_banner
                && retry.contains(x, y)
            {
                return self.refresh_action;
            }
            if let Some((btn, _)) = market.state_button
                && btn.contains(x, y)
            {
                return self.refresh_action;
            }
            if !self.view.items.is_empty() && market.group.contains(x, y) {
                let index = ((y - market.group.top) / MARKET_ROW) as usize;
                return market
                    .button(index)
                    .contains(x, y)
                    .then(|| market.action(index))
                    .flatten();
            }
        }
        None
    }

    fn key(&self, stop_index: usize, nav: Nav) -> Option<Action> {
        let stop = self.stops.get(stop_index).filter(|s| s.enabled)?;
        match stop.kind {
            PluginFocus::Tabs => match nav {
                Nav::Decrease => Some(Action::PluginTab(self.tab.step(-1))),
                Nav::Increase | Nav::Activate => Some(Action::PluginTab(self.tab.step(1))),
                _ => None,
            },
            PluginFocus::Refresh => (nav == Nav::Activate)
                .then_some(self.refresh_action)
                .flatten(),
            PluginFocus::EmptyBrowseMarket => {
                (nav == Nav::Activate).then_some(Action::PluginTab(PluginTab::Market))
            }
            PluginFocus::InstalledUpdate(market_idx) => (nav == Nav::Activate
                && self.view.busy.is_none())
            .then_some(Action::InstallPlugin(market_idx)),
            PluginFocus::InstalledRemove(index) => (nav == Nav::Activate
                && self.view.busy.is_none())
            .then_some(Action::RemovePlugin(index)),
            PluginFocus::MarketItem(index) => (nav == Nav::Activate)
                .then(|| self.market.as_ref()?.action(index))
                .flatten(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectLink {
    pub title: &'static str,
    pub action: Action,
    pub row: Area,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AboutPage {
    pub compact: bool,
    pub icon: Area,
    pub name: Area,
    pub version: Area,
    pub check_button: Area,
    pub check_label: &'static str,
    pub links: [ProjectLink; 2],
}

impl AboutPage {
    fn new(view: Area) -> Self {
        let center = (view.left + view.right) / 2.;
        let half_width = ((view.right - view.left) / 2.).min(182.);
        let (left, right) = (center - half_width, center + half_width);
        let height = view.bottom - view.top;
        // Stack the identity above the links when possible. At minimum height
        // the identity and update button share a line so all actions still fit.
        let compact = height < 310.;
        let row_height = if compact { 36. } else { 46. };
        let links_offset = if compact { 72. } else { 218. };
        let top = view.top + (height - links_offset - 2. * row_height) / 2.;
        let (icon, name, version, check_button) = if compact {
            (
                Area::new(left, top + 6., left + 48., top + 54.),
                Area::new(left + 62., top, right - 108., top + 34.),
                Area::new(left + 62., top + 34., right - 108., top + 59.),
                Area::new(right - 96., top + 14., right, top + 46.),
            )
        } else {
            (
                Area::new(center - 36., top, center + 36., top + 72.),
                Area::new(left, top + 82., right, top + 124.),
                Area::new(left, top + 124., right, top + 150.),
                Area::new(center - 52., top + 160., center + 52., top + 192.),
            )
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
            let top = top + links_offset + i as f32 * row_height;
            ProjectLink {
                title,
                action: Action::Open(uri),
                row: Area::new(left, top, right, top + row_height),
            }
        });
        Self {
            compact,
            icon,
            name,
            version,
            check_button,
            check_label: "检查更新",
            links,
        }
    }

    /// Buttons in the same order in which Tab reaches them.
    pub fn buttons(&self) -> impl Iterator<Item = (Area, Action)> {
        std::iter::once((self.check_button, Action::CheckUpdates))
            .chain(self.links.iter().map(|link| (link.row, link.action)))
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
    pub enabled: bool,
    pub label: &'static str,
    pub help: Cow<'static, str>,
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
    pub plugin_tab: PluginTab,
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

    pub fn set_plugin_tab(&mut self, tab: PluginTab) {
        if self.plugin_tab != tab {
            self.plugin_tab = tab;
            self.scroll = 0.;
            if self.focus.is_some_and(|i| i > 1) {
                self.focus = None;
            }
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
        let focusable = |i: usize| layout.focusable(i);
        let focused = self.focus.filter(|&i| focusable(i));
        match nav {
            Nav::Close => Some(Action::Close),
            Nav::Next | Nav::Previous if count > 0 => {
                let next = (1..=count)
                    .map(|step| match nav {
                        Nav::Next => (focused.unwrap_or(count - 1) + step) % count,
                        _ => (focused.unwrap_or(0) + count - step) % count,
                    })
                    .find(|&i| focusable(i))?;
                self.focus = Some(next);
                // Bring the focused card fully into view.
                if let Some(card) = layout.focus_area(next) {
                    let view = layout.view;
                    if card.top < view.top {
                        self.scroll -= view.top - card.top;
                    } else if card.bottom > view.bottom {
                        self.scroll += card.bottom - view.bottom;
                    }
                    self.scroll = self.scroll.clamp(0., layout.max_scroll);
                }
                None
            }
            Nav::Decrease | Nav::Increase | Nav::Activate => {
                if let Some(about) = &layout.about {
                    return if nav == Nav::Activate {
                        about
                            .buttons()
                            .map(|(_, action)| action)
                            .chain([Action::Quit])
                            .nth(focused?)
                    } else {
                        None
                    };
                }
                let Some(row) = layout.rows.get(focused?) else {
                    let index = focused? - layout.rows.len();
                    if let Some(plugins) = &layout.plugins
                        && index < plugins.stops.len()
                    {
                        return plugins.key(index, nav);
                    }
                    return (nav == Nav::Activate).then_some(
                        layout
                            .footer_link
                            .map_or(Action::Reset, |(_, _, action)| action),
                    );
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
    pub plugins: Option<PluginsPage>,
    pub groups: Vec<Area>,
    pub footer_button: Area,
    pub about: Option<AboutPage>,
    pub footer_link: Option<(Area, &'static str, Action)>,
    pub max_scroll: f32,
    pub scrolled_top: bool,
    pub scrolled_bottom: bool,
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
            || {
                self.footer_focus()
                    + usize::from(
                        self.footer_link.is_some() || SECTIONS[self.page] == Section::Appearance,
                    )
            },
            |about| about.buttons().count() + 1,
        )
    }

    pub fn footer_focus(&self) -> usize {
        self.rows.len() + self.plugins.as_ref().map_or(0, |p| p.stops.len())
    }

    pub fn focus_ring(&self) -> Option<(PluginFocus, Area)> {
        let plugins = self.plugins.as_ref()?;
        let index = self.focus?.checked_sub(self.rows.len())?;
        let stop = plugins.stops.get(index).filter(|s| s.enabled)?;
        Some((stop.kind, stop.ring_area))
    }

    fn focus_area(&self, index: usize) -> Option<Area> {
        if let Some(row) = self.rows.get(index) {
            return Some(row.card);
        }
        let plugins = self.plugins.as_ref()?;
        let i = index.checked_sub(self.rows.len())?;
        plugins.stops.get(i).map(|s| s.scroll_area)
    }

    fn focusable(&self, index: usize) -> bool {
        if let Some(row) = self.rows.get(index) {
            return row.enabled;
        }
        let i = index - self.rows.len();
        if let Some(plugins) = &self.plugins
            && let Some(stop) = plugins.stops.get(i)
        {
            return stop.enabled;
        }
        index < self.focus_count()
    }

    /// Fits the window to its monitor's work area and clamps the current page's scroll.
    pub fn new(state: &mut PanelState, settings: &Settings, dpi: u32, work: Rect) -> Self {
        Self::with_market(state, settings, dpi, work, &MarketView::default())
    }

    pub fn with_market(
        state: &mut PanelState,
        settings: &Settings,
        dpi: u32,
        work: Rect,
        market_view: &MarketView,
    ) -> Self {
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
        let nav_top = CAPTION + 4.;
        let nav_stride = (NAV_ITEM + NAV_GAP).min((height - nav_top - 4.) / SECTIONS.len() as f32);
        let nav = SECTIONS
            .iter()
            .enumerate()
            .map(|(i, &section)| {
                let top = nav_top + i as f32 * nav_stride;
                (
                    Area::new(NAV_LEFT, top, NAV_RIGHT, top + nav_stride - NAV_GAP),
                    section,
                )
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
                || matches!(spec.key, Key::Appearance(key) if key.parent().is_none())
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
                Key::DesktopCards => PLUGIN_HERO_CARD,
                Key::Appearance(_) if matches!(spec.control, Control::Slider { .. }) => 70.,
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
        let plugins = (section == Section::Plugins).then(|| {
            let (page, end_top) = PluginsPage::build(view, top, state.plugin_tab, market_view);
            top = end_top;
            page
        });
        let content = top - view.top;
        let max_scroll = (content - (view.bottom - view.top)).max(0.);
        state.scroll = state.scroll.clamp(0., max_scroll);
        let scrolled_top = state.scroll > 0.5;
        let scrolled_bottom = state.scroll + 0.5 < max_scroll;
        let plugins = plugins.map(|mut page| {
            page.shift(state.scroll);
            page
        });
        let scroll_thumb = (max_scroll > 0.).then(|| {
            let track = view.bottom - view.top;
            let thumb = (track * track / content).max(18.);
            let y = view.top + (track - thumb) * state.scroll / max_scroll;
            Area::new(view.right + 6., y, view.right + 10., y + thumb)
        });
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
                    enabled: match spec.key {
                        Key::Appearance(key) => key.editable(settings),
                        _ => true,
                    },
                    label: spec.label,
                    help: if spec.key == Key::DockDisplayMode {
                        Cow::Borrowed(match settings.dock_display_mode() {
                            DockDisplayMode::Default => spec.help,
                            DockDisplayMode::Overlay => "允许其他窗口遮挡 Dock",
                            DockDisplayMode::AutoHide => "移开收起，停在主屏底边任意位置唤出",
                            DockDisplayMode::SmartHide => "与前台窗口重叠时收起",
                            DockDisplayMode::Always => "切换应用后保持置顶",
                            DockDisplayMode::Hotkey => "Ctrl+Alt+D 显示或隐藏 Dock",
                            DockDisplayMode::AlwaysIncludingFullscreen => "全屏时也保持置顶",
                            DockDisplayMode::Desktop => "仅在桌面处于前台时显示",
                        })
                    } else if spec.key == Key::DesktopCards {
                        desktop_cards_help(value != 0, market_view)
                    } else {
                        Cow::Borrowed(spec.help)
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
            plugins,
            groups,
            footer_link: match section {
                Section::General => Some(("屏幕刷新率…", DISPLAY_SETTINGS)),
                Section::Plugins => Some(("打开插件目录…", Action::OpenPluginsDirectory)),
                _ => None,
            }
            .map(|(label, action)| {
                let area = Area::new(
                    view.left,
                    reset_top,
                    view.left + 132.,
                    reset_top + CONTROL_HEIGHT,
                );
                (area, label, action)
            }),
            about: (section == Section::About).then(|| AboutPage::new(view)),
            footer_button: Area::new(
                view.right - RESET_WIDTH,
                reset_top,
                view.right,
                reset_top + CONTROL_HEIGHT,
            ),
            max_scroll,
            scrolled_top,
            scrolled_bottom,
            scroll_thumb,
            focus: state.focus,
            hover: state.hover,
            modes,
        };
        state.focus = state.focus.filter(|&i| layout.focusable(i));
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
        if !row.enabled {
            return None;
        }
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
        if let Some((area, _, action)) = self.footer_link
            && area.contains(x, y)
        {
            return Some(action);
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
                .buttons()
                .find(|(area, _)| area.contains(x, y))
                .map(|(_, action)| action);
        }
        if !self.view.contains(x, y) {
            return None;
        }
        if let Some(plugins) = &self.plugins
            && let Some(action) = plugins.hit(x, y)
        {
            return Some(action);
        }
        let row = self.rows.iter().find(|row| row.card.contains(x, y))?;
        if !row.enabled {
            return None;
        }
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

fn desktop_cards_help(enabled: bool, view: &MarketView) -> Cow<'static, str> {
    let count = view.installed.len();
    let cards: usize = view.installed.iter().map(|item| item.cards).sum();
    let errors = view
        .installed
        .iter()
        .filter(|item| item.health == InstalledHealth::Error)
        .count();
    let setup = view
        .installed
        .iter()
        .filter(|item| item.health == InstalledHealth::Setup)
        .count();
    if !enabled {
        if count == 0 {
            Cow::Borrowed("已暂停 · 开启后在主屏右侧显示插件卡片")
        } else {
            Cow::Owned(format!(
                "已暂停 · 已保留 {count} 款本地插件（共 {cards} 张卡片）"
            ))
        }
    } else if errors > 0 {
        Cow::Owned(format!("已安装 {count} 款插件 · {errors} 款存在异常"))
    } else if setup > 0 {
        Cow::Owned(format!("共 {cards} 张卡片 · {setup} 款插件等待配置"))
    } else if count > 0 && cards > 0 {
        Cow::Owned(format!("正在展示 {count} 款插件发布的 {cards} 张卡片"))
    } else if count > 0 {
        Cow::Owned(format!("已安装 {count} 款插件 · 当前未发布可见卡片"))
    } else {
        Cow::Borrowed("在主屏桌面显示各插件发布的卡片")
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
            } else if row.key == Key::DesktopCards {
                62.
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
        let mut settings = Settings::default();
        for spec in SCHEMA {
            if let Key::Appearance(key) = spec.key
                && let Some(parent) = key.parent()
            {
                settings.set(Key::Appearance(parent), 1);
            }
        }
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
                let expected_link = match section {
                    Section::General => Some(DISPLAY_SETTINGS),
                    Section::Plugins => Some(Action::OpenPluginsDirectory),
                    _ => None,
                };
                assert_eq!(
                    layout.footer_link.map(|(_, _, action)| action),
                    expected_link
                );
                if let Some((button, _, action)) = layout.footer_link {
                    let (x, y) = center(button, s);
                    assert_eq!(layout.hit(x, y), Some(action));
                    if let Some(plugins) = &layout.plugins {
                        let (x, y) = center(plugins.refresh_button, s);
                        assert_eq!(layout.hit(x, y), Some(Action::BrowseMarket));
                        state.key(Nav::Next, &layout);
                        assert_eq!(
                            state.key(Nav::Increase, &layout),
                            Some(Action::PluginTab(PluginTab::Market))
                        );
                        state.key(Nav::Next, &layout);
                        assert_eq!(
                            state.key(Nav::Activate, &layout),
                            Some(Action::BrowseMarket)
                        );
                    }
                    state.key(Nav::Next, &layout);
                    let layout = PanelLayout::new(&mut state, &settings, dpi, work(4000));
                    assert_eq!(state.key(Nav::Activate, &layout), Some(action));
                }
                // The window corner and the caption do nothing.
                assert_eq!(layout.hit(1, 1), None);
            }
        }
    }

    #[test]
    fn market_install_buttons_scroll_into_view_and_skip_installed_items() {
        let mut state = PanelState {
            page: SECTIONS
                .iter()
                .position(|s| *s == Section::Plugins)
                .unwrap(),
            ..Default::default()
        };
        let mut market = MarketView {
            installed: vec![InstalledItem {
                id: "local-card".into(),
                title: "本地卡片".into(),
                version: Some("1.0.0".into()),
                market_update: Some((0, "1.2.0".into())),
                summary: "1 张卡片 · 今日待办".into(),
                cards: 1,
                health: InstalledHealth::Ready,
                removable: true,
            }]
            .into(),
            loaded: true,
            items: (0..18)
                .map(|i| MarketItem {
                    id: format!("plugin-{i}"),
                    name: format!("插件 {i}"),
                    version: "1.2.0".into(),
                    installed_version: (i == 0 || i == 7).then(|| "1.0.0".into()),
                    description: "示例卡片描述".into(),
                    state: if i == 0 {
                        MarketState::Update
                    } else if i == 7 {
                        MarketState::Installed
                    } else {
                        MarketState::Install
                    },
                })
                .collect(),
            ..Default::default()
        };
        let settings = Settings::default();
        let mut layout = PanelLayout::with_market(&mut state, &settings, 144, work(510), &market);
        for expected in [
            Action::Change(Key::DesktopCards, Adjust::Toggle),
            Action::PluginTab(PluginTab::Market),
            Action::BrowseMarket,
            Action::InstallPlugin(0),
            Action::RemovePlugin(0),
        ]
        .into_iter()
        .chain((0..18).filter(|i| *i != 7).map(Action::InstallPlugin))
        {
            state.key(Nav::Next, &layout);
            layout = PanelLayout::with_market(&mut state, &settings, 144, work(510), &market);
            assert_eq!(state.key(Nav::Activate, &layout), Some(expected));
            if let Action::InstallPlugin(index) = expected
                && state.focus.unwrap() > 4
            {
                let button = layout
                    .plugins
                    .as_ref()
                    .unwrap()
                    .market
                    .as_ref()
                    .unwrap()
                    .button(index);
                let (x, y) = center(button, layout.scale);
                assert_eq!(layout.hit(x, y), Some(expected));
            }
        }
        market.busy = Some(Action::InstallPlugin(17));
        layout = PanelLayout::with_market(&mut state, &settings, 144, work(510), &market);
        let section = layout.plugins.as_ref().unwrap().market.as_ref().unwrap();
        assert_eq!(section.button_label(17), "安装中…");
        let (x, y) = center(section.button(17), layout.scale);
        assert_eq!(layout.hit(x, y), None);
        assert_eq!(state.key(Nav::Activate, &layout), None);

        // Switching to the Installed tab preserves tab-bar keyboard focus and isolates installed items.
        state.focus = Some(1);
        state.set_plugin_tab(PluginTab::Installed);
        assert_eq!(state.focus, Some(1));
        market.busy = None;
        layout = PanelLayout::with_market(&mut state, &settings, 144, work(510), &market);
        let pages = layout.plugins.as_ref().unwrap();
        assert!(pages.installed.is_some() && pages.market.is_none());
        let remove = pages.installed.as_ref().unwrap().rows[0]
            .remove_button
            .unwrap();
        let (x, y) = center(remove, layout.scale);
        assert_eq!(layout.hit(x, y), Some(Action::RemovePlugin(0)));

        // An empty Installed tab offers a direct jump to the Market tab, and inline errors retry.
        let empty_error = MarketView {
            error: Some("网络连接超时".into()),
            ..Default::default()
        };
        layout = PanelLayout::with_market(&mut state, &settings, 144, work(510), &empty_error);
        let cta = layout
            .plugins
            .as_ref()
            .unwrap()
            .installed
            .as_ref()
            .unwrap()
            .empty_button
            .unwrap();
        let (x, y) = center(cta, layout.scale);
        assert_eq!(layout.hit(x, y), Some(Action::PluginTab(PluginTab::Market)));
        state.set_plugin_tab(PluginTab::Market);
        layout = PanelLayout::with_market(&mut state, &settings, 144, work(510), &empty_error);
        let (retry, label) = layout
            .plugins
            .as_ref()
            .unwrap()
            .market
            .as_ref()
            .unwrap()
            .state_button
            .unwrap();
        assert_eq!(label, "重试");
        let (x, y) = center(retry, layout.scale);
        assert_eq!(layout.hit(x, y), Some(Action::BrowseMarket));
    }

    #[test]
    fn appearance_controls_follow_their_switch_for_pointer_and_keyboard() {
        use crate::settings::AppearanceKey::{DockCustom, DockOpacity, Reflection};
        let mut settings = Settings::default();
        let mut state = PanelState {
            page: SECTIONS
                .iter()
                .position(|&s| s == Section::Appearance)
                .unwrap(),
            ..Default::default()
        };
        let layout = PanelLayout::new(&mut state, &settings, 96, work(800));
        let opacity = layout
            .rows
            .iter()
            .find(|r| r.key == Key::Appearance(DockOpacity))
            .unwrap();
        let Part::Slider { area, .. } = opacity.part else {
            panic!("expected opacity slider")
        };
        let (x, y) = center(area, layout.scale);
        assert_eq!(layout.hit(x, y), None);
        state.key(Nav::Next, &layout);
        state.key(Nav::Next, &layout);
        assert_eq!(
            layout.rows[state.focus.unwrap()].key,
            Key::Appearance(Reflection)
        );
        state.key(Nav::Previous, &layout);
        assert_eq!(
            state.key(Nav::Activate, &layout),
            Some(Action::Change(Key::Appearance(DockCustom), Adjust::Toggle))
        );
        settings.adjust(Key::Appearance(DockCustom), Adjust::Toggle);
        let layout = PanelLayout::new(&mut state, &settings, 96, work(800));
        assert_eq!(layout.slider_at(x, y), Some(Key::Appearance(DockOpacity)));
        state.key(Nav::Next, &layout);
        assert_eq!(
            state.key(Nav::Increase, &layout),
            Some(Action::Change(
                Key::Appearance(DockOpacity),
                Adjust::Step(1)
            ))
        );
        state.focus = None;
        state.key(Nav::Previous, &layout);
        assert_eq!(state.key(Nav::Activate, &layout), Some(Action::Reset));
    }

    #[test]
    fn about_actions_work_with_pointer_and_keyboard_at_both_sizes() {
        let settings = Settings::default();
        for dpi in [96, 120, 144, 192] {
            for (width, height) in [(MIN_WIDTH, MIN_HEIGHT), (WIDTH, HEIGHT)] {
                let scale = dpi as f32 / 96.;
                let work = Rect {
                    width: (width * scale) as i32,
                    ..work((height * scale) as i32)
                };
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
                for link in &about.links {
                    let y = (link.row.middle() * layout.scale).round() as i32;
                    for x in [link.row.left + 1., link.row.right - 1.] {
                        assert_eq!(
                            layout.hit((x * layout.scale).round() as i32, y),
                            Some(link.action)
                        );
                    }
                }
                let footer_y = (layout.footer_button.middle() * layout.scale).round() as i32;
                assert_eq!(
                    layout.hit((layout.view.left * layout.scale) as i32, footer_y),
                    None
                );
                for (area, expected) in [
                    (about.check_button, Action::CheckUpdates),
                    (
                        about.links[0].row,
                        Action::Open("https://github.com/Ariember/DeskIsle"),
                    ),
                    (
                        about.links[1].row,
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
