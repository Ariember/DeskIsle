//! DeskIsle's own context menus: entries, their layout in DIPs, placement next to the
//! clicked target, and pointer and keyboard selection. Drawing and the menu window live
//! in the renderer and platform layers.

use std::path::PathBuf;

use crate::dock::{AppGroup, Rect};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Icon {
    None,
    /// A Segoe Fluent Icons code point.
    Glyph(&'static str),
    /// The Shell icon of a file.
    File(PathBuf),
    /// The current image of a tray icon, by index into the tray host's list.
    Tray(usize),
    /// Marks a window of an application; the foreground window is highlighted.
    Window {
        active: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Item<C> {
    pub label: String,
    /// Right-aligned secondary text.
    pub detail: String,
    pub icon: Icon,
    /// `None` shows the item without letting the user choose it.
    pub command: Option<C>,
    pub checked: Option<bool>,
    pub danger: bool,
}

#[derive(Clone, Debug)]
pub enum Entry<C> {
    Header {
        title: String,
        subtitle: String,
        icon: Icon,
    },
    Item(Item<C>),
    Separator,
}

#[derive(Clone, Debug)]
pub struct Menu<C> {
    pub entries: Vec<Entry<C>>,
}

impl<C> Menu<C> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn header(&mut self, title: impl Into<String>, subtitle: impl Into<String>, icon: Icon) {
        self.entries.push(Entry::Header {
            title: title.into(),
            subtitle: subtitle.into(),
            icon,
        });
    }

    pub fn item(
        &mut self,
        label: impl Into<String>,
        icon: Icon,
        command: Option<C>,
    ) -> &mut Item<C> {
        self.entries.push(Entry::Item(Item {
            label: label.into(),
            detail: String::new(),
            icon,
            command,
            checked: None,
            danger: false,
        }));
        match self.entries.last_mut() {
            Some(Entry::Item(item)) => item,
            _ => unreachable!("an item was just pushed"),
        }
    }

    /// Separates groups. Leading and repeated separators are dropped, and a trailing
    /// one is removed by [`MenuLayout::new`].
    pub fn separator(&mut self) {
        if matches!(self.entries.last(), Some(Entry::Item(_))) {
            self.entries.push(Entry::Separator);
        }
    }

    /// The command of an enabled item.
    pub fn command(&self, index: usize) -> Option<&C> {
        match self.entries.get(index)? {
            Entry::Item(item) => item.command.as_ref(),
            _ => None,
        }
    }

    fn enabled(&self, index: usize) -> bool {
        self.command(index).is_some()
    }

    /// The next enabled item after `from` (or before it when `forward` is false),
    /// wrapping around. Starts at either end when nothing is selected.
    pub fn step(&self, from: Option<usize>, forward: bool) -> Option<usize> {
        let count = self.entries.len();
        if count == 0 {
            return None;
        }
        let start = from.unwrap_or(if forward { count - 1 } else { 0 });
        (1..=count)
            .map(|offset| {
                if forward {
                    (start + offset) % count
                } else {
                    (start + count - offset % count) % count
                }
            })
            .find(|&index| self.enabled(index))
    }
}

/// Transparent margin around the panel that holds its shadow, in DIPs.
pub const SHADOW: f32 = 16.;
pub const PADDING: f32 = 5.;
pub const ITEM_HEIGHT: f32 = 32.;
pub const HEADER_HEIGHT: f32 = 52.;
const SEPARATOR_HEIGHT: f32 = 9.;
/// Horizontal room before the label for icons and window marks.
pub const ICON_COLUMN: f32 = 34.;
pub const CHECK_COLUMN: f32 = 28.;
pub const TEXT_INSET: f32 = 12.;
pub const LABEL_SIZE: f32 = 13.;
pub const DETAIL_SIZE: f32 = 12.;
pub const TITLE_SIZE: f32 = 14.;
const MIN_WIDTH: f32 = 216.;
const MAX_WIDTH: f32 = 380.;
/// Distance between the menu and the target it was opened for, in DIPs.
const GAP: f32 = 6.;

/// Where a menu opens: centered on `x`, clear of the edge at `y`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub x: i32,
    pub y: i32,
    /// Opens above `y` when there is room, otherwise below it.
    pub up: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The pointer is over the shadow margin or outside the window.
    Outside,
    /// A header, separator, disabled item or the panel padding.
    Inert,
    Item(usize),
}

/// A menu's geometry in DIPs, with the panel inset by [`SHADOW`] inside the window.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuLayout {
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    /// Top and bottom of each entry, aligned with `Menu::entries`.
    pub rows: Vec<(f32, f32)>,
    pub checks: bool,
}

impl MenuLayout {
    /// `measure(text, size, bold)` returns the width of a single line in DIPs.
    pub fn new<C>(menu: &mut Menu<C>, dpi: u32, measure: impl Fn(&str, f32, bool) -> f32) -> Self {
        if matches!(menu.entries.last(), Some(Entry::Separator)) {
            menu.entries.pop();
        }
        let checks = menu.entries.iter().any(|e| {
            matches!(
                e,
                Entry::Item(Item {
                    checked: Some(_),
                    ..
                })
            )
        });
        let mut content: f32 = 0.;
        let mut rows = Vec::with_capacity(menu.entries.len());
        let mut y = SHADOW + PADDING;
        for entry in &menu.entries {
            let height = match entry {
                Entry::Header {
                    title, subtitle, ..
                } => {
                    let text =
                        measure(title, TITLE_SIZE, true).max(measure(subtitle, DETAIL_SIZE, false));
                    content = content.max(TEXT_INSET + 40. + text + TEXT_INSET);
                    HEADER_HEIGHT
                }
                Entry::Item(item) => {
                    let mut width =
                        TEXT_INSET + ICON_COLUMN + measure(&item.label, LABEL_SIZE, false);
                    if !item.detail.is_empty() {
                        width += 24. + measure(&item.detail, DETAIL_SIZE, false);
                    }
                    content = content.max(width + TEXT_INSET);
                    ITEM_HEIGHT
                }
                Entry::Separator => SEPARATOR_HEIGHT,
            };
            rows.push((y, y + height));
            y += height;
        }
        if checks {
            content += CHECK_COLUMN;
        }
        let panel = (content + 2. * PADDING).ceil().clamp(MIN_WIDTH, MAX_WIDTH);
        Self {
            scale: dpi as f32 / 96.,
            width: panel + 2. * SHADOW,
            height: y + PADDING + SHADOW,
            rows,
            checks,
        }
    }

    /// The window size in physical pixels.
    pub fn pixels(&self) -> (i32, i32) {
        (
            (self.width * self.scale).ceil() as i32,
            (self.height * self.scale).ceil() as i32,
        )
    }

    /// The panel's left, top, right and bottom edges.
    pub fn panel(&self) -> (f32, f32, f32, f32) {
        (SHADOW, SHADOW, self.width - SHADOW, self.height - SHADOW)
    }

    /// The highlight behind an entry's row.
    pub fn row(&self, index: usize) -> (f32, f32, f32, f32) {
        let (top, bottom) = self.rows[index];
        (SHADOW + PADDING, top, self.width - SHADOW - PADDING, bottom)
    }

    /// `x` and `y` are client pixels of the menu window.
    pub fn hit<C>(&self, menu: &Menu<C>, x: i32, y: i32) -> Hit {
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        let (left, top, right, bottom) = self.panel();
        if !(left..right).contains(&x) || !(top..bottom).contains(&y) {
            return Hit::Outside;
        }
        match self.rows.iter().position(|&(t, b)| (t..b).contains(&y)) {
            Some(index) if menu.enabled(index) => Hit::Item(index),
            _ => Hit::Inert,
        }
    }

    /// Places the window in screen pixels so the panel stays inside `bounds`.
    pub fn place(&self, anchor: Anchor, bounds: Rect) -> Rect {
        let (width, height) = self.pixels();
        let px = |dip: f32| (dip * self.scale).round() as i32;
        let (margin, gap) = (px(SHADOW), px(GAP));
        let panel_height = height - 2 * margin;
        let edge = px(4.);
        let above = anchor.y - gap - panel_height;
        let below = anchor.y + gap;
        let fits_above = above >= bounds.y + edge;
        let fits_below = below + panel_height <= bounds.y + bounds.height - edge;
        let top = match (anchor.up, fits_above, fits_below) {
            (true, true, _) | (false, true, false) => above,
            (false, _, true) | (true, false, true) => below,
            _ => anchor.y - panel_height / 2,
        };
        let top = top.clamp(
            bounds.y + edge,
            (bounds.y + bounds.height - edge - panel_height).max(bounds.y + edge),
        );
        let panel_width = width - 2 * margin;
        let left = (anchor.x - panel_width / 2).clamp(
            bounds.x + edge,
            (bounds.x + bounds.width - edge - panel_width).max(bounds.x + edge),
        );
        Rect {
            x: left - margin,
            y: top - margin,
            width,
            height,
        }
    }
}

/// What a menu item asks the coordinator to do. Applications are named by their ID and
/// windows by handle, both checked again before anything acts on them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Same as clicking the Dock icon: launch, switch, or choose a window.
    Select(String),
    Window(String, isize),
    NewInstance(PathBuf),
    Reveal(PathBuf),
    Pin(PathBuf),
    Unpin(String),
    Close(String, Vec<isize>),
    TaskManager,
    ShowDesktop,
    DateTime,
    Notifications,
    TrayActivate(usize),
    TrayToggle(usize),
    Quit,
}

/// Long lists end in a summary so the menu fits on screen.
const LIST_LIMIT: usize = 12;

fn windows_list(menu: &mut Menu<Command>, app: &AppGroup, foreground: Option<isize>) {
    for window in app.windows.iter().take(LIST_LIMIT) {
        let active = Some(window.id) == foreground;
        menu.item(
            window.title.clone(),
            Icon::Window { active },
            Some(Command::Window(app.app_id.clone(), window.id)),
        );
    }
    if app.windows.len() > LIST_LIMIT {
        menu.item(
            format!("还有 {} 个窗口", app.windows.len() - LIST_LIMIT),
            Icon::None,
            None,
        );
    }
}

fn app_header(menu: &mut Menu<Command>, app: &AppGroup) {
    let subtitle = match (app.windows.len(), app.pinned()) {
        (0, true) => "已固定 · 未运行".to_owned(),
        (0, false) => String::new(),
        (n, _) if app.active => format!("{n} 个窗口 · 前台"),
        (n, _) => format!("{n} 个窗口"),
    };
    let icon = app
        .executable()
        .map_or(Icon::None, |path| Icon::File(path.into()));
    menu.header(app.label.clone(), subtitle, icon);
    menu.separator();
}

/// The menu of one application's Dock icon or status bar title.
pub fn app_menu(app: &AppGroup, foreground: Option<isize>) -> Menu<Command> {
    let mut menu = Menu::new();
    app_header(&mut menu, app);
    windows_list(&mut menu, app, foreground);
    menu.separator();
    let id = app.app_id.clone();
    match app.executable() {
        Some(path) if app.windows.is_empty() => {
            menu.item(
                "打开",
                Icon::Glyph("\u{E8A7}"),
                Some(Command::Select(id.clone())),
            );
            menu.item(
                "打开文件所在位置",
                Icon::Glyph("\u{E838}"),
                Some(Command::Reveal(path.into())),
            );
        }
        Some(path) => {
            menu.item(
                "新建窗口",
                Icon::Glyph("\u{E710}"),
                Some(Command::NewInstance(path.into())),
            );
            menu.item(
                "打开文件所在位置",
                Icon::Glyph("\u{E838}"),
                Some(Command::Reveal(path.into())),
            );
        }
        None => {}
    }
    if app.pinned() {
        menu.item(
            "从 Dock 取消固定",
            Icon::Glyph("\u{E77A}"),
            Some(Command::Unpin(id.clone())),
        );
    } else if let Some(path) = app.executable() {
        menu.item(
            "固定到 Dock",
            Icon::Glyph("\u{E718}"),
            Some(Command::Pin(path.into())),
        );
    }
    if !app.windows.is_empty() {
        menu.separator();
        let ids = app.windows.iter().map(|w| w.id).collect();
        let label = if app.windows.len() > 1 {
            "关闭所有窗口"
        } else {
            "关闭窗口"
        };
        menu.item(
            label,
            Icon::Glyph("\u{E711}"),
            Some(Command::Close(id, ids)),
        )
        .danger = true;
    }
    menu
}

/// Lets the user pick one of several windows after a click on the Dock icon.
pub fn window_menu(app: &AppGroup, foreground: Option<isize>) -> Menu<Command> {
    let mut menu = Menu::new();
    app_header(&mut menu, app);
    windows_list(&mut menu, app, foreground);
    menu
}

/// Empty space on either bar and the status bar mark: every application, a few system
/// tools, and quitting DeskIsle.
pub fn bar_menu(apps: &[AppGroup]) -> Menu<Command> {
    let mut menu = Menu::new();
    let running = apps.iter().filter(|a| !a.windows.is_empty()).count();
    menu.header(
        "DeskIsle",
        format!("{running} 个应用正在运行"),
        Icon::Glyph("\u{E7C4}"),
    );
    menu.separator();
    for app in apps.iter().take(LIST_LIMIT) {
        let icon = app
            .executable()
            .map_or(Icon::Window { active: app.active }, |p| {
                Icon::File(p.into())
            });
        let item = menu.item(
            app.label.clone(),
            icon,
            Some(Command::Select(app.app_id.clone())),
        );
        item.detail = match app.windows.len() {
            _ if app.active => "前台".into(),
            0 => String::new(),
            1 => "运行中".into(),
            n => format!("{n} 个窗口"),
        };
    }
    if apps.len() > LIST_LIMIT {
        menu.item(
            format!("还有 {} 个应用", apps.len() - LIST_LIMIT),
            Icon::None,
            None,
        );
    }
    menu.separator();
    menu.item(
        "任务管理器",
        Icon::Glyph("\u{E9D9}"),
        Some(Command::TaskManager),
    );
    menu.item(
        "显示桌面",
        Icon::Glyph("\u{E7F4}"),
        Some(Command::ShowDesktop),
    );
    menu.separator();
    menu.item(
        "退出 DeskIsle",
        Icon::Glyph("\u{E7E8}"),
        Some(Command::Quit),
    )
    .danger = true;
    menu
}

pub fn clock_menu(date: &str, year: &str) -> Menu<Command> {
    let mut menu = Menu::new();
    menu.header(date, year, Icon::Glyph("\u{E787}"));
    menu.separator();
    menu.item(
        "通知和日历",
        Icon::Glyph("\u{E91C}"),
        Some(Command::Notifications),
    );
    menu.item(
        "日期和时间设置",
        Icon::Glyph("\u{E713}"),
        Some(Command::DateTime),
    );
    menu
}

/// A tray icon as the overflow menu lists it.
pub struct TrayEntry<'a> {
    pub index: usize,
    pub label: &'a str,
    /// Placed on the status bar right now.
    pub on_bar: bool,
    /// The user keeps it on the bar when there is room.
    pub kept: bool,
}

/// Icons that are not on the bar for activation, then every icon as a toggle.
pub fn tray_menu(icons: &[TrayEntry<'_>]) -> Menu<Command> {
    let mut menu = Menu::new();
    let hidden = icons.iter().filter(|i| !i.on_bar).count();
    menu.header(
        "通知区域",
        format!("{} 个图标 · {hidden} 个未显示", icons.len()),
        Icon::Glyph("\u{E946}"),
    );
    menu.separator();
    for icon in icons.iter().filter(|i| !i.on_bar) {
        menu.item(
            icon.label,
            Icon::Tray(icon.index),
            Some(Command::TrayActivate(icon.index)),
        );
    }
    menu.separator();
    menu.item("在状态栏显示", Icon::None, None);
    for icon in icons {
        menu.item(
            icon.label,
            Icon::Tray(icon.index),
            Some(Command::TrayToggle(icon.index)),
        )
        .checked = Some(icon.kept);
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(text: &str, size: f32, _: bool) -> f32 {
        text.chars().count() as f32 * size * 0.6
    }

    fn sample() -> Menu<u8> {
        let mut menu = Menu::new();
        menu.separator();
        menu.header("Editor", "2 个窗口", Icon::None);
        menu.item("first", Icon::Window { active: true }, Some(1));
        menu.item("disabled", Icon::None, None);
        menu.separator();
        menu.separator();
        menu.item("last", Icon::Glyph("\u{E711}"), Some(2)).danger = true;
        menu.separator();
        menu
    }

    fn apps() -> Vec<AppGroup> {
        use crate::dock::{PinnedApp, WindowInfo, group_apps};
        let pins = [PinnedApp {
            app_id: "editor".into(),
            label: "Editor".into(),
            icon_path: "editor.exe".into(),
            launch: "editor.exe".into(),
        }];
        let window = |id, app: &str| WindowInfo {
            id,
            app_id: app.into(),
            title: format!("{app} {id}"),
            label: app.into(),
            icon_path: format!("{app}.exe").into(),
        };
        group_apps(&pins, &[window(7, "viewer"), window(8, "viewer")], 8)
    }

    fn commands(menu: &Menu<Command>) -> Vec<Command> {
        (0..menu.entries.len())
            .filter_map(|i| menu.command(i).cloned())
            .collect()
    }

    #[test]
    fn each_icon_offers_actions_for_its_own_state() {
        let apps = apps();
        // A pinned application without windows opens or unpins; nothing to close.
        let idle = commands(&app_menu(&apps[0], Some(8)));
        assert!(idle.contains(&Command::Select("editor".into())));
        assert!(idle.contains(&Command::Unpin("editor".into())));
        assert!(
            !idle
                .iter()
                .any(|c| matches!(c, Command::Close(..) | Command::Pin(_)))
        );

        // A running, unpinned application lists its windows and can be pinned or closed.
        let running = commands(&app_menu(&apps[1], Some(8)));
        assert_eq!(
            running
                .iter()
                .filter(|c| matches!(c, Command::Window(..)))
                .count(),
            2
        );
        assert!(running.contains(&Command::Pin("viewer.exe".into())));
        assert!(running.contains(&Command::NewInstance("viewer.exe".into())));
        assert_eq!(
            running.last(),
            Some(&Command::Close("viewer".into(), vec![7, 8]))
        );
        assert!(!running.iter().any(|c| matches!(c, Command::Unpin(_))));

        // The foreground window is marked.
        let menu = app_menu(&apps[1], Some(8));
        assert!(menu.entries.iter().any(|e| matches!(
            e,
            Entry::Item(Item { icon: Icon::Window { active: true }, label, .. }) if label == "viewer 8"
        )));
    }

    #[test]
    fn blank_bar_and_tray_menus_differ_from_icon_menus() {
        let apps = apps();
        let bar = commands(&bar_menu(&apps));
        assert_eq!(bar.last(), Some(&Command::Quit));
        assert!(bar.contains(&Command::Select("viewer".into())));
        assert!(!commands(&app_menu(&apps[1], None)).contains(&Command::Quit));

        let icons = [
            TrayEntry {
                index: 0,
                label: "a",
                on_bar: true,
                kept: true,
            },
            TrayEntry {
                index: 3,
                label: "b",
                on_bar: false,
                kept: false,
            },
        ];
        assert_eq!(
            commands(&tray_menu(&icons)),
            [
                Command::TrayActivate(3),
                Command::TrayToggle(0),
                Command::TrayToggle(3)
            ]
        );
    }

    #[test]
    fn separators_only_divide_groups() {
        let mut menu = sample();
        let layout = MenuLayout::new(&mut menu, 96, measure);
        assert!(matches!(menu.entries[0], Entry::Header { .. }));
        assert!(matches!(menu.entries[3], Entry::Separator));
        assert!(matches!(menu.entries[4], Entry::Item(_)));
        assert_eq!(menu.entries.len(), 5);
        assert_eq!(layout.rows.len(), 5);
    }

    #[test]
    fn pointer_hits_only_enabled_items_at_every_scale() {
        for dpi in [96, 120, 144, 192] {
            let mut menu = sample();
            let layout = MenuLayout::new(&mut menu, dpi, measure);
            let s = layout.scale;
            let px = |dip: f32| (dip * s).ceil() as i32;
            let x = px(layout.width / 2.);
            let (top, bottom) = layout.rows[1];
            assert_eq!(layout.hit(&menu, x, px(top)), Hit::Item(1));
            assert_eq!(layout.hit(&menu, x, px(bottom) - 1), Hit::Item(1));
            let (top, _) = layout.rows[0];
            assert_eq!(layout.hit(&menu, x, px(top)), Hit::Inert);
            let (top, _) = layout.rows[2];
            assert_eq!(layout.hit(&menu, x, px(top)), Hit::Inert);
            assert_eq!(layout.hit(&menu, x, px(SHADOW) - 1), Hit::Outside);
            assert_eq!(layout.hit(&menu, 0, px(top)), Hit::Outside);
            assert_eq!(menu.command(4), Some(&2));
        }
    }

    #[test]
    fn keyboard_steps_over_inert_entries_and_wraps() {
        let mut menu = sample();
        MenuLayout::new(&mut menu, 96, measure);
        assert_eq!(menu.step(None, true), Some(1));
        assert_eq!(menu.step(None, false), Some(4));
        assert_eq!(menu.step(Some(1), true), Some(4));
        assert_eq!(menu.step(Some(4), true), Some(1));
        assert_eq!(menu.step(Some(1), false), Some(4));
        let mut empty = Menu::<u8>::new();
        empty.header("t", "", Icon::None);
        assert_eq!(empty.step(None, true), None);
    }

    #[test]
    fn width_follows_text_within_limits() {
        let mut short = sample();
        let mut long = sample();
        long.item("x".repeat(400), Icon::None, Some(3));
        let short = MenuLayout::new(&mut short, 96, measure);
        let long = MenuLayout::new(&mut long, 96, measure);
        assert_eq!(short.width, MIN_WIDTH + 2. * SHADOW);
        assert_eq!(long.width, MAX_WIDTH + 2. * SHADOW);
    }

    #[test]
    fn placement_opens_toward_room_and_stays_on_screen() {
        let bounds = Rect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        for dpi in [96, 144] {
            let mut menu = sample();
            let layout = MenuLayout::new(&mut menu, dpi, measure);
            let (w, h) = layout.pixels();
            let margin = (SHADOW * layout.scale).round() as i32;
            let panel = |r: Rect| {
                (
                    r.x + margin,
                    r.y + margin,
                    r.x + w - margin,
                    r.y + h - margin,
                )
            };

            // Above a dock icon near the bottom edge.
            let up = layout.place(
                Anchor {
                    x: -960,
                    y: 1000,
                    up: true,
                },
                bounds,
            );
            let (_, _, _, bottom) = panel(up);
            assert!(bottom < 1000);
            assert!((up.x + w / 2 + 960).abs() <= 1);

            // Below the status bar.
            let down = layout.place(
                Anchor {
                    x: -960,
                    y: 30,
                    up: false,
                },
                bounds,
            );
            assert!(panel(down).1 > 30);

            // No room above: flips below. Near the right edge: shifts left.
            let flipped = layout.place(
                Anchor {
                    x: -5,
                    y: 10,
                    up: true,
                },
                bounds,
            );
            let (left, top, right, _) = panel(flipped);
            assert!(top > 10);
            assert!(right <= 0 && left >= -1920);
        }
    }
}
