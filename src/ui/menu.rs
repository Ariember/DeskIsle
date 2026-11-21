//! DeskIsle's own context menus: entries, their layout in DIPs, placement next to the
//! clicked target, and pointer and keyboard selection. Drawing and the menu window live
//! in the renderer and platform layers.

use std::path::PathBuf;

use crate::dock::AppGroup;
use crate::geometry::Rect;
use crate::ui::calendar::{self, Month};
use crate::ui::icon::Symbol;
use crate::ui::lunar;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Icon {
    None,
    /// An icon of DeskIsle's own set.
    Symbol(Symbol),
    /// The Shell icon of a file.
    File(PathBuf),
    /// The current image of a tray icon, by index into the tray host's list, on a
    /// circle that turns solid while `on`.
    Tray {
        index: usize,
        on: bool,
    },
    /// Marks a window of an application; the foreground window is highlighted.
    Window {
        active: bool,
    },
    /// An icon in a circle that fills in when the option is on or selected.
    Choice {
        symbol: Symbol,
        on: bool,
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
    /// Chosen by clicking the leading icon instead of the rest of the row.
    pub icon_command: Option<C>,
    /// Chosen by a right click on the row or the menu key while it is focused.
    pub context: Option<C>,
    pub danger: bool,
}

/// A value the user drags, such as the volume. The coordinator applies the value while
/// the menu stays open.
#[derive(Clone, Debug)]
pub struct Slider<C> {
    pub target: SliderTarget,
    /// Percent, or `None` when the feature is unavailable.
    pub value: Option<u8>,
    pub symbol: Symbol,
    /// Chosen by clicking the icon at the start of the track, such as muting.
    pub button: Option<C>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderTarget {
    Volume,
    Brightness,
}

/// One of several quick toggles that share a row.
#[derive(Clone, Debug)]
pub struct Tile<C> {
    pub symbol: Symbol,
    pub title: String,
    pub detail: String,
    pub on: bool,
    pub command: Option<C>,
}

/// A month of the calendar with buttons to the previous month, back to today and to the
/// next month, in that order.
#[derive(Clone, Debug)]
pub struct Calendar<C> {
    pub month: Month,
    /// `None` hides the button back to today while today's month is shown, and disables
    /// the arrows at the ends of the lunar tables.
    pub buttons: [Option<C>; 3],
}

/// The calendar's buttons, as parts of its entry.
pub const CALENDAR_PREVIOUS: usize = 0;
pub const CALENDAR_TODAY: usize = 1;
pub const CALENDAR_NEXT: usize = 2;

#[derive(Clone, Debug)]
pub enum Entry<C> {
    Header {
        title: String,
        subtitle: String,
        icon: Icon,
    },
    Item(Item<C>),
    /// A small heading above a group.
    Label(String),
    Slider(Slider<C>),
    Tiles(Vec<Tile<C>>),
    /// What is playing, with buttons at the trailing edge.
    Player {
        title: String,
        detail: String,
        buttons: Vec<(Symbol, Option<C>)>,
    },
    Calendar(Calendar<C>),
    Separator,
}

#[derive(Clone, Debug)]
pub struct Menu<C> {
    pub entries: Vec<Entry<C>>,
    /// The narrowest the panel gets, in DIPs.
    pub min_width: f32,
    /// The widest the panel gets, in DIPs. Longer text ends in an ellipsis.
    pub max_width: f32,
}

impl<C> Menu<C> {
    /// A menu of exactly `width` DIPs, so DeskIsle's menus line up whatever they list.
    pub fn fixed(width: f32) -> Self {
        Self {
            entries: Vec::new(),
            min_width: width,
            max_width: width,
        }
    }

    pub fn label(&mut self, text: impl Into<String>) {
        self.entries.push(Entry::Label(text.into()));
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
            icon_command: None,
            context: None,
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
        if !matches!(
            self.entries.last(),
            None | Some(Entry::Separator | Entry::Header { .. } | Entry::Label(_))
        ) {
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

    /// The right-click command of an item.
    pub fn context_command(&self, index: usize) -> Option<&C> {
        match self.entries.get(index)? {
            Entry::Item(item) => item.context.as_ref(),
            _ => None,
        }
    }

    /// The command of part `part` of a tile row, the player, a slider's icon, or an
    /// item's leading icon.
    pub fn part_command(&self, index: usize, part: usize) -> Option<&C> {
        match self.entries.get(index)? {
            Entry::Item(item) if part == 0 => item.icon_command.as_ref(),
            Entry::Tiles(tiles) => tiles.get(part)?.command.as_ref(),
            Entry::Player { buttons, .. } => buttons.get(part)?.1.as_ref(),
            Entry::Slider(slider) if part == 0 => slider.button.as_ref(),
            Entry::Calendar(calendar) => calendar.buttons.get(part)?.as_ref(),
            _ => None,
        }
    }

    pub fn focused_command(&self, focus: Focus) -> Option<&C> {
        match focus {
            Focus::Item(index) => self.command(index),
            Focus::Part(index, part) => self.part_command(index, part),
            Focus::Slider(_) => None,
        }
    }

    /// The next enabled item after `from` (or before it when `forward` is false),
    /// wrapping around. Starts at either end when nothing is selected.
    pub fn step(&self, from: Option<Focus>, forward: bool) -> Option<Focus> {
        let mut controls = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            let parts = match entry {
                Entry::Item(item) => {
                    // The leading icon comes first, as it is drawn.
                    if item.icon_command.is_some() {
                        controls.push(Focus::Part(index, 0));
                    }
                    if item.command.is_some() {
                        controls.push(Focus::Item(index));
                    }
                    continue;
                }
                Entry::Slider(slider) => {
                    if slider.button.is_some() {
                        controls.push(Focus::Part(index, 0));
                    }
                    if slider.value.is_some() {
                        controls.push(Focus::Slider(index));
                    }
                    continue;
                }
                Entry::Tiles(tiles) => tiles.len(),
                Entry::Player { buttons, .. } => buttons.len(),
                Entry::Calendar(calendar) => calendar.buttons.len(),
                _ => continue,
            };
            controls.extend(
                (0..parts)
                    .filter(|&part| self.part_command(index, part).is_some())
                    .map(|part| Focus::Part(index, part)),
            );
        }
        let current = controls.iter().position(|&c| Some(c) == from);
        let next = if forward {
            current
                .and_then(|i| controls.get(i + 1))
                .or(controls.first())
        } else {
            current
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| controls.get(i))
                .or(controls.last())
        };
        next.copied()
    }
}

/// Transparent margin around the panel that holds its shadow, in DIPs.
pub const SHADOW: f32 = 8.;
pub const PADDING: f32 = 5.;
pub const ITEM_HEIGHT: f32 = 32.;
pub const HEADER_HEIGHT: f32 = 52.;
const SEPARATOR_HEIGHT: f32 = 9.;
pub const LABEL_HEIGHT: f32 = 26.;
pub const SLIDER_HEIGHT: f32 = 40.;
/// The slider's pill-shaped track, centered in its row.
pub const TRACK_HEIGHT: f32 = 26.;
pub const TILE_HEIGHT: f32 = 60.;
pub const TILE_GAP: f32 = 8.;
pub const PLAYER_HEIGHT: f32 = 56.;
pub const PLAYER_BUTTON: f32 = 34.;
/// The calendar: its month and buttons, the weekday names, and six weeks of days.
pub const CALENDAR_NAV: f32 = 38.;
pub const CALENDAR_WEEKDAYS: f32 = 24.;
pub const CALENDAR_CELL: f32 = 40.;
pub const CALENDAR_ARROW: f32 = 28.;
pub const CALENDAR_TODAY_WIDTH: f32 = 46.;
pub const CALENDAR_HEIGHT: f32 =
    CALENDAR_NAV + CALENDAR_WEEKDAYS + calendar::WEEKS as f32 * CALENDAR_CELL + 8.;
/// Inset of sliders, tiles and the player from the panel's padding.
pub const BLOCK_INSET: f32 = 6.;
/// Horizontal room before the label for icons and window marks.
pub const ICON_COLUMN: f32 = 34.;
/// Diameter of the round backing every icon of a menu row sits on, and its distance
/// from the start of the row.
pub const ICON_BADGE: f32 = 24.;
pub const ICON_INSET: f32 = 8.;
pub const TEXT_INSET: f32 = 12.;
pub const LABEL_SIZE: f32 = 13.;
pub const DETAIL_SIZE: f32 = 12.;
pub const TITLE_SIZE: f32 = 14.;
/// Standard width for status bar flyouts and application menus, in DIPs.
pub const DROPDOWN_WIDTH: f32 = 320.;
const MARK_MENU_WIDTH: f32 = 220.;
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
    /// Part of a tile row, the player, a slider's icon or an item's leading icon, with
    /// an enabled command.
    Part(usize, usize),
    /// The track of an available slider, with the value at the pointer.
    Slider(usize, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Item(usize),
    Part(usize, usize),
    Slider(usize),
}

impl Focus {
    /// The entry the focus is in.
    pub fn index(self) -> usize {
        match self {
            Self::Item(i) | Self::Part(i, _) | Self::Slider(i) => i,
        }
    }
}

impl Hit {
    pub fn focus(self) -> Option<Focus> {
        match self {
            Self::Item(i) => Some(Focus::Item(i)),
            Self::Part(i, p) => Some(Focus::Part(i, p)),
            Self::Slider(i, _) => Some(Focus::Slider(i)),
            _ => None,
        }
    }
}

/// A menu's geometry in DIPs, with the panel inset by [`SHADOW`] inside the window.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuLayout {
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    /// Top and bottom of each entry, aligned with `Menu::entries`.
    pub rows: Vec<(f32, f32)>,
    /// Reserved for leading icons when at least one item uses one.
    pub icon_column: f32,
}

impl MenuLayout {
    /// `measure(text, size, bold)` returns the width of a single line in DIPs.
    pub fn new<C>(menu: &mut Menu<C>, dpi: u32, measure: impl Fn(&str, f32, bool) -> f32) -> Self {
        if matches!(menu.entries.last(), Some(Entry::Separator)) {
            menu.entries.pop();
        }
        let icon_column = if menu
            .entries
            .iter()
            .any(|entry| matches!(entry, Entry::Item(item) if item.icon != Icon::None))
        {
            ICON_COLUMN
        } else {
            0.
        };
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
                        TEXT_INSET + icon_column + measure(&item.label, LABEL_SIZE, false);
                    if !item.detail.is_empty() {
                        width += 24. + measure(&item.detail, DETAIL_SIZE, false);
                    }
                    content = content.max(width + TEXT_INSET);
                    ITEM_HEIGHT
                }
                Entry::Label(text) => {
                    let width = measure(text, DETAIL_SIZE, true);
                    content = content.max(TEXT_INSET + width + TEXT_INSET);
                    LABEL_HEIGHT
                }
                Entry::Slider(_) => {
                    content = content.max(220.);
                    SLIDER_HEIGHT
                }
                Entry::Tiles(tiles) => {
                    let widest = tiles
                        .iter()
                        .map(|t| {
                            measure(&t.title, DETAIL_SIZE, true).max(measure(&t.detail, 11., false))
                        })
                        .fold(0., f32::max);
                    let n = tiles.len() as f32;
                    let row = n * (46. + widest + 10.) + (n - 1.).max(0.) * TILE_GAP;
                    content = content.max(row + 2. * BLOCK_INSET);
                    TILE_HEIGHT
                }
                Entry::Player {
                    title,
                    detail,
                    buttons,
                } => {
                    let text =
                        measure(title, LABEL_SIZE, true).max(measure(detail, DETAIL_SIZE, false));
                    let controls = buttons.len() as f32 * PLAYER_BUTTON;
                    content = content.max(2. * BLOCK_INSET + 50. + text.min(180.) + controls + 12.);
                    PLAYER_HEIGHT
                }
                Entry::Calendar(_) => {
                    content = content.max(7. * 40. + 2. * BLOCK_INSET);
                    CALENDAR_HEIGHT
                }
                Entry::Separator => SEPARATOR_HEIGHT,
            };
            rows.push((y, y + height));
            y += height;
        }
        let max = menu.max_width.max(1.);
        let min = menu.min_width.min(max);
        let panel = (content + 2. * PADDING).ceil().clamp(min, max);
        Self {
            scale: dpi as f32 / 96.,
            width: panel + 2. * SHADOW,
            height: y + PADDING + SHADOW,
            rows,
            icon_column,
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

    /// The round backing of the leading icon in entry `index`'s row.
    pub fn icon_badge(&self, index: usize) -> (f32, f32, f32, f32) {
        let (left, top, _, bottom) = self.row(index);
        let (x, y) = (left + ICON_INSET + ICON_BADGE / 2., (top + bottom) / 2.);
        let r = ICON_BADGE / 2.;
        (x - r, y - r, x + r, y + r)
    }

    /// Where the leading icon of entry `index` takes clicks of its own: the badge with
    /// the rest of the row's height and the gap around it.
    fn icon_target(&self, index: usize) -> (f32, f32, f32, f32) {
        let (left, top, _, bottom) = self.row(index);
        (left, top, left + 2. * ICON_INSET + ICON_BADGE, bottom)
    }

    /// The area of a slider, tile row or player inside entry `index`'s row.
    pub fn block(&self, index: usize) -> (f32, f32, f32, f32) {
        let (left, top, right, bottom) = self.row(index);
        (
            left + BLOCK_INSET,
            top + 4.,
            right - BLOCK_INSET,
            bottom - 4.,
        )
    }

    /// The pill track of the slider in entry `index`.
    pub fn track(&self, index: usize) -> (f32, f32, f32, f32) {
        let (left, _, right, _) = self.block(index);
        let (top, bottom) = self.rows[index];
        let middle = (top + bottom) / 2.;
        (
            left,
            middle - TRACK_HEIGHT / 2.,
            right,
            middle + TRACK_HEIGHT / 2.,
        )
    }

    /// The slider value under client pixel `x`, along the stretch of the track the
    /// knob's center can reach.
    pub fn slider_value(&self, index: usize, x: i32) -> u8 {
        let (left, _, right, _) = self.track(index);
        let half = TRACK_HEIGHT / 2.;
        let (start, end) = (left + half, right - half);
        let t = ((x as f32 / self.scale - start) / (end - start)).clamp(0., 1.);
        (t * 100.).round() as u8
    }

    /// Tile `part` of `count` tiles sharing entry `index`'s row.
    pub fn tile(&self, index: usize, part: usize, count: usize) -> (f32, f32, f32, f32) {
        let (left, top, right, bottom) = self.block(index);
        let count = count.max(1) as f32;
        let width = (right - left - (count - 1.) * TILE_GAP) / count;
        let l = left + part as f32 * (width + TILE_GAP);
        (l, top, l + width, bottom)
    }

    /// Button `part` of `count` buttons at the trailing edge of the player.
    pub fn player_button(&self, index: usize, part: usize, count: usize) -> (f32, f32, f32, f32) {
        let (_, top, right, bottom) = self.block(index);
        let middle = (top + bottom) / 2.;
        let l = right - 8. - (count - part) as f32 * PLAYER_BUTTON;
        let half = PLAYER_BUTTON / 2.;
        (l, middle - half, l + PLAYER_BUTTON, middle + half)
    }

    /// The month and buttons at the top of the calendar in entry `index`.
    pub fn calendar_nav(&self, index: usize) -> (f32, f32, f32, f32) {
        let (left, top, right, _) = self.block(index);
        (left, top, right, top + CALENDAR_NAV)
    }

    /// Button `part` of the calendar in entry `index`: the arrows on either side of the
    /// button back to today, at the trailing edge.
    pub fn calendar_button(&self, index: usize, part: usize) -> (f32, f32, f32, f32) {
        let (_, top, right, bottom) = self.calendar_nav(index);
        let middle = (top + bottom) / 2.;
        let next = right - CALENDAR_ARROW;
        let today = next - CALENDAR_TODAY_WIDTH;
        let (l, r) = match part {
            CALENDAR_PREVIOUS => (today - CALENDAR_ARROW, today),
            CALENDAR_TODAY => (today, next),
            _ => (next, right),
        };
        let half = CALENDAR_ARROW / 2.;
        (l, middle - half, r, middle + half)
    }

    /// The column of weekday `column` above the calendar's days, 0 for Sunday.
    pub fn calendar_weekday(&self, index: usize, column: usize) -> (f32, f32, f32, f32) {
        let (left, _, right, bottom) = self.calendar_nav(index);
        let width = (right - left) / 7.;
        let l = left + column as f32 * width;
        (l, bottom, l + width, bottom + CALENDAR_WEEKDAYS)
    }

    /// Day `cell` of the calendar's six weeks, counted from the first Sunday.
    pub fn calendar_cell(&self, index: usize, cell: usize) -> (f32, f32, f32, f32) {
        let (l, _, r, bottom) = self.calendar_weekday(index, cell % 7);
        let top = bottom + (cell / 7) as f32 * CALENDAR_CELL;
        (l, top, r, top + CALENDAR_CELL)
    }

    /// `x` and `y` are client pixels of the menu window.
    pub fn hit<C>(&self, menu: &Menu<C>, x: i32, y: i32) -> Hit {
        let pixel_x = x;
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        let (left, top, right, bottom) = self.panel();
        if !(left..right).contains(&x) || !(top..bottom).contains(&y) {
            return Hit::Outside;
        }
        let Some(index) = self.rows.iter().position(|&(t, b)| (t..b).contains(&y)) else {
            return Hit::Inert;
        };
        let inside =
            |(l, t, r, b): (f32, f32, f32, f32)| (l..r).contains(&x) && (t..b).contains(&y);
        let part = |part: usize| match menu.part_command(index, part) {
            Some(_) => Hit::Part(index, part),
            None => Hit::Inert,
        };
        match &menu.entries[index] {
            Entry::Item(item) if item.icon_command.is_some() && inside(self.icon_target(index)) => {
                Hit::Part(index, 0)
            }
            Entry::Item(item) if item.command.is_some() => Hit::Item(index),
            Entry::Tiles(tiles) => (0..tiles.len())
                .find(|&i| inside(self.tile(index, i, tiles.len())))
                .map_or(Hit::Inert, part),
            Entry::Player { buttons, .. } => (0..buttons.len())
                .find(|&i| inside(self.player_button(index, i, buttons.len())))
                .map_or(Hit::Inert, part),
            Entry::Calendar(calendar) => (0..calendar.buttons.len())
                .find(|&i| inside(self.calendar_button(index, i)))
                .map_or(Hit::Inert, part),
            Entry::Slider(slider) => {
                let track = self.track(index);
                if !inside(track) {
                    Hit::Inert
                } else if slider.button.is_some() && x < track.0 + TRACK_HEIGHT {
                    Hit::Part(index, 0)
                } else if slider.value.is_some() {
                    Hit::Slider(index, self.slider_value(index, pixel_x))
                } else {
                    Hit::Inert
                }
            }
            _ => Hit::Inert,
        }
    }

    /// The area covered by the focus highlight, including a margin around a slider
    /// so the highlight remains visible behind its opaque track.
    pub fn focus_bounds<C>(&self, menu: &Menu<C>, focus: Focus) -> (f32, f32, f32, f32) {
        match focus {
            Focus::Item(i) => self.row(i),
            Focus::Slider(i) => self.block(i),
            Focus::Part(i, p) => match &menu.entries[i] {
                Entry::Tiles(tiles) => self.tile(i, p, tiles.len()),
                Entry::Player { buttons, .. } => self.player_button(i, p, buttons.len()),
                Entry::Calendar(_) => self.calendar_button(i, p),
                Entry::Item(_) => {
                    let (l, t, r, b) = self.icon_target(i);
                    (l, t, r - ICON_INSET / 2., b)
                }
                Entry::Slider(_) => {
                    let (l, t, _, b) = self.block(i);
                    (l - 2., t, l + TRACK_HEIGHT + 2., b)
                }
                _ => unreachable!("focus belongs to an interactive part"),
            },
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
    CloseWindow(String, isize),
    CloseAllWindows(String, Vec<isize>),
    TaskManager,
    ShowDesktop,
    /// Sleeps the computer, then returns once it wakes.
    Sleep,
    /// Restarting, shutting down and signing out let applications keep unsaved work.
    Restart,
    ShutDown,
    SignOut,
    /// Locks the screen.
    Lock,
    Settings,
    /// Shows the About card with this computer's hardware and Windows release.
    About,
    DateTime,
    Notifications,
    /// Turns the clock menu's calendar to a month; the menu stays open.
    Calendar(i32, u8),
    /// Same as a left click on the tray icon in the status bar.
    TrayActivate(usize),
    /// Same as a right click on it: the application shows its own menu.
    TrayMenu(usize),
    /// Keeps the tray icon on the status bar or moves it into the overflow menu.
    TrayToggle(usize),
    /// Turns a page in the open tray list; true moves to the next page.
    TrayPage(bool),
    /// Opens a settings page or another URI with the Shell.
    Open(&'static str),
    /// Changes a system feature from a status bar flyout, which stays open.
    System(crate::ui::system::SystemCommand),
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

fn app_header(menu: &mut Menu<Command>, app: &AppGroup, has_tray_menu: bool) {
    // The rows below already list the windows and offer pinning, so the subtitle only
    // says what they cannot: whether the application is in front or not running.
    let subtitle = match app.windows.len() {
        0 if has_tray_menu => "后台运行",
        0 => "未运行",
        _ if app.active => "前台",
        _ => "",
    };
    let icon = app
        .executable()
        .map_or(Icon::None, |path| Icon::File(path.into()));
    menu.header(app.label.clone(), subtitle, icon);
    menu.separator();
}

/// The menu of one application's Dock icon or status bar title.
pub fn app_menu(
    app: &AppGroup,
    foreground: Option<isize>,
    tray_menus: &[(usize, &str)],
) -> Menu<Command> {
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    app_header(&mut menu, app, !tray_menus.is_empty());
    windows_list(&mut menu, app, foreground);
    menu.separator();
    let id = app.app_id.clone();
    match app.executable() {
        Some(path) if app.windows.is_empty() => {
            menu.item(
                "打开",
                Icon::Symbol(Symbol::Open),
                Some(Command::Select(id.clone())),
            );
            menu.item(
                "打开文件所在位置",
                Icon::Symbol(Symbol::Folder),
                Some(Command::Reveal(path.into())),
            );
        }
        Some(path) => {
            menu.item(
                "新建窗口",
                Icon::Symbol(Symbol::Plus),
                Some(Command::NewInstance(path.into())),
            );
            menu.item(
                "打开文件所在位置",
                Icon::Symbol(Symbol::Folder),
                Some(Command::Reveal(path.into())),
            );
        }
        None => {}
    }
    if app.pinned() {
        menu.item(
            "从 Dock 取消固定",
            Icon::Symbol(Symbol::Unpin),
            Some(Command::Unpin(id.clone())),
        );
    } else if let Some(path) = app.executable() {
        menu.item(
            "固定到 Dock",
            Icon::Symbol(Symbol::Pin),
            Some(Command::Pin(path.into())),
        );
    }
    if let Some(window) = app.current_window(foreground) {
        menu.separator();
        let close = menu.item(
            "关闭窗口",
            Icon::Symbol(Symbol::Close),
            Some(Command::CloseWindow(id.clone(), window.id)),
        );
        if app.windows.len() > 1 {
            close.detail = window.title.clone();
        }
        if tray_menus.is_empty() {
            let ids = app.windows.iter().map(|w| w.id).collect();
            let exit = menu.item(
                "退出程序",
                Icon::Symbol(Symbol::SignOut),
                Some(Command::CloseAllWindows(id, ids)),
            );
            exit.detail = "请求关闭所有窗口".into();
            exit.danger = true;
        }
    }
    if !tray_menus.is_empty() {
        menu.separator();
    }
    for &(index, label) in tray_menus {
        let item = menu.item(
            "退出程序…",
            Icon::Symbol(Symbol::SignOut),
            Some(Command::TrayMenu(index)),
        );
        item.detail = if tray_menus.len() == 1 {
            "打开应用托盘菜单".into()
        } else {
            label.lines().collect::<Vec<_>>().join(" ")
        };
    }
    menu
}

/// Lets the user pick one of several windows after a click on the Dock icon.
pub fn window_menu(app: &AppGroup, foreground: Option<isize>) -> Menu<Command> {
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    app_header(&mut menu, app, false);
    windows_list(&mut menu, app, foreground);
    menu
}

/// Empty space on either bar and a right click on the status bar mark: every
/// application, a few system tools, and quitting DeskIsle.
pub fn bar_menu(apps: &[AppGroup]) -> Menu<Command> {
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    let running = apps.iter().filter(|a| !a.windows.is_empty()).count();
    menu.header(
        "DeskIsle",
        format!("{running} 个应用正在运行"),
        Icon::Symbol(Symbol::Apps),
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
        Icon::Symbol(Symbol::Activity),
        Some(Command::TaskManager),
    );
    menu.item(
        "显示桌面",
        Icon::Symbol(Symbol::Desktop),
        Some(Command::ShowDesktop),
    );
    menu.separator();
    menu.item(
        "设置",
        Icon::Symbol(Symbol::Settings),
        Some(Command::Settings),
    );
    menu.item(
        "退出 DeskIsle",
        Icon::Symbol(Symbol::Power),
        Some(Command::Quit),
    )
    .danger = true;
    menu
}

/// A click on the status bar mark: this computer, Windows settings and tools, then
/// power and session commands. Those that end the session are marked as dangerous.
pub fn mark_menu() -> Menu<Command> {
    let mut menu = Menu::fixed(MARK_MENU_WIDTH);
    menu.item("关于本机", Icon::Symbol(Symbol::Info), Some(Command::About));
    menu.separator();
    menu.item(
        "管理面板",
        Icon::Symbol(Symbol::Sliders),
        Some(Command::Settings),
    );
    menu.item(
        "系统偏好设置",
        Icon::Symbol(Symbol::Settings),
        Some(Command::Open("ms-settings:")),
    );
    menu.item(
        "应用商店",
        Icon::Symbol(Symbol::Store),
        Some(Command::Open("ms-windows-store:")),
    );
    menu.separator();
    menu.item(
        "任务管理器",
        Icon::Symbol(Symbol::Activity),
        Some(Command::TaskManager),
    );
    menu.item(
        "显示桌面",
        Icon::Symbol(Symbol::Desktop),
        Some(Command::ShowDesktop),
    );
    menu.separator();
    menu.item("睡眠", Icon::Symbol(Symbol::Moon), Some(Command::Sleep));
    menu.item(
        "重新启动",
        Icon::Symbol(Symbol::Restart),
        Some(Command::Restart),
    )
    .danger = true;
    menu.item("关机", Icon::Symbol(Symbol::Power), Some(Command::ShutDown))
        .danger = true;
    menu.separator();
    menu.item("锁定屏幕", Icon::Symbol(Symbol::Lock), Some(Command::Lock));
    menu.item(
        "注销",
        Icon::Symbol(Symbol::SignOut),
        Some(Command::SignOut),
    )
    .danger = true;
    menu
}

/// The clock's menu: today's date in both calendars, then the calendar turned to `shown`,
/// a year and month.
pub fn clock_menu(today: (i32, u8, u8), shown: (i32, u8)) -> Menu<Command> {
    let (year, month, day) = today;
    let weekday = calendar::WEEKDAYS[usize::from(lunar::weekday(year, month, day))];
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    menu.header(
        format!("{month}月{day}日 星期{weekday}"),
        calendar::describe(today),
        Icon::Symbol(Symbol::Calendar),
    );
    // Headers usually run into the first group; the calendar wants a rule above it.
    menu.entries.push(Entry::Separator);
    let turn = |by| calendar::shift(shown.0, shown.1, by).map(|(y, m)| Command::Calendar(y, m));
    let back = (shown != (year, month)).then_some(Command::Calendar(year, month));
    menu.entries.push(Entry::Calendar(Calendar {
        month: Month::new(shown.0, shown.1, today),
        buttons: [turn(-1), back, turn(1)],
    }));
    menu.separator();
    menu.item(
        "通知和日历",
        Icon::Symbol(Symbol::Bell),
        Some(Command::Notifications),
    );
    menu.item(
        "日期和时间设置",
        Icon::Symbol(Symbol::Settings),
        Some(Command::DateTime),
    );
    menu
}

/// A tray icon as the overflow menu lists it.
pub struct TrayEntry<'a> {
    pub index: usize,
    /// The application's name.
    pub label: &'a str,
    /// Its tooltip, shown beside the name when it says something else, such as Clash
    /// clients reporting the current speed.
    pub status: &'a str,
    pub can_interact: bool,
    /// The user keeps it on the status bar; it still overflows when there is no room.
    pub kept: bool,
}

impl Menu<Command> {
    /// Recover the same row or leading icon after the live tray list changes.
    pub fn tray_focus(&self, command: &Command) -> Option<Focus> {
        self.entries.iter().enumerate().find_map(|(row, entry)| {
            let Entry::Item(item) = entry else {
                return None;
            };
            if item.command.as_ref() == Some(command) {
                Some(Focus::Item(row))
            } else if item.icon_command.as_ref() == Some(command) {
                Some(Focus::Part(row, 0))
            } else {
                None
            }
        })
    }

    /// Wheel and Page Up/Down use the same enabled controls as pointer input.
    pub fn page_step(&self, forward: bool) -> Option<&Command> {
        self.entries.iter().find_map(|entry| match entry {
            Entry::Calendar(calendar) => {
                let part = if forward {
                    CALENDAR_NEXT
                } else {
                    CALENDAR_PREVIOUS
                };
                calendar.buttons[part].as_ref()
            }
            Entry::Item(item) if item.command == Some(Command::TrayPage(forward)) => {
                item.command.as_ref()
            }
            _ => None,
        })
    }
}

/// Every application tray icon. A row clicks the icon as the status bar does, with
/// either button. Its leading icon sits on a solid circle while the user keeps it on
/// the bar, and clicking that icon toggles the choice.
pub fn tray_menu(
    icons: &[TrayEntry<'_>],
    page: &mut usize,
    focused_icon: Option<usize>,
    available_px: i32,
    dpi: u32,
) -> Menu<Command> {
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    let available = available_px as f32 * 96. / dpi as f32;
    let rows = ((available - GAP - 2. * (SHADOW + PADDING)) / ITEM_HEIGHT).floor() as usize;
    let paged = icons.len() > rows;
    let page_size = if paged { rows.saturating_sub(2) } else { rows }.max(1);
    let pages = icons.len().div_ceil(page_size);
    *page = focused_icon
        .and_then(|index| icons.iter().position(|icon| icon.index == index))
        .map_or_else(
            || (*page).min(pages.saturating_sub(1)),
            |row| row / page_size,
        );
    let page = *page;
    for icon in icons.iter().skip(page * page_size).take(page_size) {
        let item = menu.item(
            icon.label,
            Icon::Tray {
                index: icon.index,
                on: icon.kept,
            },
            icon.can_interact
                .then_some(Command::TrayActivate(icon.index)),
        );
        item.icon_command = Some(Command::TrayToggle(icon.index));
        item.context = icon.can_interact.then_some(Command::TrayMenu(icon.index));
        item.detail = tray_status(icon.label, icon.status);
        if !icon.can_interact {
            if item.detail.is_empty() {
                item.detail = "仅显示".into();
            } else {
                item.detail.insert_str(0, "仅显示 · ");
            }
        }
    }
    if pages > 1 {
        let previous = menu.item(
            "上一页",
            Icon::Symbol(Symbol::ChevronUp),
            (page > 0).then_some(Command::TrayPage(false)),
        );
        previous.detail = format!("{} / {}", page + 1, pages);
        menu.item(
            "下一页",
            Icon::Symbol(Symbol::ChevronDown),
            (page + 1 < pages).then_some(Command::TrayPage(true)),
        );
    }
    menu
}

/// A tooltip on one line, or nothing when it only repeats the name.
fn tray_status(name: &str, tooltip: &str) -> String {
    let line = tooltip.split_whitespace().collect::<Vec<_>>().join(" ");
    if line == name || line.is_empty() {
        String::new()
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(text: &str, size: f32, _: bool) -> f32 {
        text.chars().count() as f32 * size * 0.6
    }

    fn sample() -> Menu<u8> {
        let mut menu = Menu::fixed(DROPDOWN_WIDTH);
        menu.separator();
        menu.header("Editor", "2 个窗口", Icon::None);
        menu.item("first", Icon::Window { active: true }, Some(1));
        menu.item("disabled", Icon::None, None);
        menu.separator();
        menu.separator();
        menu.item("last", Icon::Symbol(Symbol::Close), Some(2))
            .danger = true;
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
        let idle = commands(&app_menu(&apps[0], Some(8), &[]));
        assert!(idle.contains(&Command::Select("editor".into())));
        assert!(idle.contains(&Command::Unpin("editor".into())));
        assert!(!idle.iter().any(|c| matches!(
            c,
            Command::CloseWindow(..) | Command::CloseAllWindows(..) | Command::Pin(_)
        )));

        // A running, unpinned application lists its windows and can be pinned or closed.
        let running = commands(&app_menu(&apps[1], Some(8), &[]));
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
            Some(&Command::CloseAllWindows("viewer".into(), vec![7, 8]))
        );
        assert!(running.contains(&Command::CloseWindow("viewer".into(), 8)));
        let tray = commands(&app_menu(&apps[1], None, &[(4, "Viewer")]));
        assert!(tray.contains(&Command::CloseWindow("viewer".into(), 7)));
        assert!(tray.contains(&Command::TrayMenu(4)));
        assert!(
            !tray
                .iter()
                .any(|c| matches!(c, Command::CloseAllWindows(..)))
        );
        assert!(!running.iter().any(|c| matches!(c, Command::Unpin(_))));

        // The foreground window is marked.
        let menu = app_menu(&apps[1], Some(8), &[]);
        assert!(menu.entries.iter().any(|e| matches!(
            e,
            Entry::Item(Item { icon: Icon::Window { active: true }, label, .. }) if label == "viewer 8"
        )));

        // The header only says what the rows cannot, and the menu lines up with the
        // status bar dropdowns.
        let subtitle = |menu: &Menu<Command>| match &menu.entries[0] {
            Entry::Header { subtitle, .. } => subtitle.clone(),
            _ => panic!("menus start with the application header"),
        };
        assert_eq!(subtitle(&menu), "前台");
        let mut idle = app_menu(&apps[0], Some(8), &[]);
        assert_eq!(subtitle(&idle), "未运行");
        let background = app_menu(&apps[0], None, &[(2, "Editor")]);
        assert_eq!(subtitle(&background), "后台运行");
        assert!(commands(&background).contains(&Command::TrayMenu(2)));
        assert!(
            !commands(&background)
                .iter()
                .any(|c| matches!(c, Command::CloseWindow(..)))
        );
        let layout = MenuLayout::new(&mut idle, 96, measure);
        assert_eq!(layout.width, DROPDOWN_WIDTH + 2. * SHADOW);
    }

    #[test]
    fn blank_bar_and_tray_menus_differ_from_icon_menus() {
        let apps = apps();
        let bar = commands(&bar_menu(&apps));
        assert_eq!(bar.last(), Some(&Command::Quit));
        assert!(bar.contains(&Command::Select("viewer".into())));
        assert!(!commands(&app_menu(&apps[1], None, &[])).contains(&Command::Quit));

        let icons = [
            TrayEntry {
                index: 0,
                label: "a",
                status: "a",
                can_interact: false,
                kept: true,
            },
            TrayEntry {
                index: 3,
                label: "Sparkle",
                status: "\u{2191} 943 B/s\n\u{2193} 0 B/s",
                can_interact: true,
                kept: false,
            },
        ];
        let menu = tray_menu(&icons, &mut 0, None, 720, 96);
        // One row per icon, whether or not it is on the bar, with no checkmarks.
        assert_eq!(commands(&menu), [Command::TrayActivate(3)]);
        assert_eq!(menu.context_command(0), None);
        assert_eq!(menu.step(None, true), Some(Focus::Part(0, 0)));
        assert_eq!(
            menu.step(Some(Focus::Part(0, 0)), true),
            Some(Focus::Part(1, 0))
        );
        assert_eq!(menu.part_command(0, 0), Some(&Command::TrayToggle(0)));
        assert_eq!(menu.context_command(1), Some(&Command::TrayMenu(3)));
        // The tooltip only shows when it adds something, on one line.
        let details: Vec<_> = menu
            .entries
            .iter()
            .map(|e| match e {
                Entry::Item(item) => (item.label.as_str(), item.detail.as_str()),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            details,
            [
                ("a", "仅显示"),
                ("Sparkle", "\u{2191} 943 B/s \u{2193} 0 B/s")
            ]
        );
        assert!(matches!(
            menu.entries[..],
            [
                Entry::Item(Item {
                    icon: Icon::Tray { index: 0, on: true },
                    ..
                }),
                Entry::Item(Item {
                    icon: Icon::Tray {
                        index: 3,
                        on: false
                    },
                    ..
                })
            ]
        ));
    }

    #[test]
    fn every_tray_registration_can_be_reached_on_screen() {
        let icons: Vec<_> = (0..53)
            .map(|n| TrayEntry {
                index: 2 * n + 1,
                label: "Tray app",
                status: "",
                can_interact: true,
                kept: false,
            })
            .collect();
        let bounds = Rect {
            x: -800,
            y: -600,
            width: 800,
            height: 600,
        };
        for dpi in [96, 144, 192] {
            let anchor = Anchor {
                x: -200,
                y: -600 + dpi as i32 / 3,
                up: false,
            };
            let mut seen = Vec::new();
            for page in 0..icons.len() {
                let mut selected = page;
                let mut menu = tray_menu(&icons, &mut selected, None, -anchor.y, dpi);
                assert_eq!(selected, page);
                let layout = MenuLayout::new(&mut menu, dpi, measure);
                let window = layout.place(anchor, bounds);
                assert!(window.y + window.height <= bounds.y + bounds.height);
                for (row, entry) in menu.entries.iter().enumerate() {
                    let Entry::Item(Item {
                        icon: Icon::Tray { index, .. },
                        ..
                    }) = entry
                    else {
                        continue;
                    };
                    let (_, top, _, bottom) = layout.row(row);
                    let x = (layout.width * layout.scale / 2.) as i32;
                    let y = ((top + bottom) * layout.scale / 2.) as i32;
                    assert_eq!(layout.hit(&menu, x, y), Hit::Item(row));
                    assert_eq!(menu.command(row), Some(&Command::TrayActivate(*index)));
                    assert_eq!(menu.context_command(row), Some(&Command::TrayMenu(*index)));
                    seen.push(*index);
                }
                assert_eq!(menu.page_step(false).is_some(), page > 0);
                if menu.page_step(true).is_none() {
                    break;
                }
            }
            assert_eq!(
                seen,
                icons.iter().map(|icon| icon.index).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn shrinking_tray_list_clamps_the_page_and_keeps_focus_on_the_same_control() {
        let icons = [3, 9, 11].map(|index| TrayEntry {
            index,
            label: "App",
            status: "",
            can_interact: true,
            kept: true,
        });
        let mut page = 2;
        let menu = tray_menu(&icons[1..], &mut page, None, 720, 96);
        assert_eq!(page, 0);
        assert_eq!(
            menu.tray_focus(&Command::TrayActivate(9)),
            Some(Focus::Item(0))
        );
        assert_eq!(
            menu.tray_focus(&Command::TrayToggle(11)),
            Some(Focus::Part(1, 0))
        );
        assert_eq!(menu.tray_focus(&Command::TrayActivate(3)), None);
        let menu = tray_menu(&icons, &mut page, Some(11), 108, 96);
        assert_eq!(page, 2);
        assert_eq!(
            menu.tray_focus(&Command::TrayToggle(11)),
            Some(Focus::Part(0, 0))
        );
    }

    #[test]
    fn tray_rows_split_their_icon_from_the_rest_of_the_row() {
        let icons = [TrayEntry {
            index: 2,
            label: "a",
            status: "",
            can_interact: true,
            kept: true,
        }];
        for dpi in [96, 120, 144, 192] {
            let mut menu = tray_menu(&icons, &mut 0, None, 720, 96);
            let layout = MenuLayout::new(&mut menu, dpi, measure);
            let px = |dip: f32| (dip * layout.scale).round() as i32;
            let (l, t, r, b) = layout.icon_badge(0);
            let y = px((t + b) / 2.);
            // The badge and the gap around it toggle the icon, the label clicks it.
            assert_eq!(layout.hit(&menu, px((l + r) / 2.), y), Hit::Part(0, 0));
            assert_eq!(
                layout.hit(&menu, px(SHADOW + PADDING) + 1, y),
                Hit::Part(0, 0)
            );
            assert_eq!(layout.hit(&menu, px(r + ICON_INSET + 2.), y), Hit::Item(0));
            assert_eq!(layout.hit(&menu, px(layout.width / 2.), y), Hit::Item(0));
            // The keyboard reaches the icon before its row.
            assert_eq!(menu.step(None, true), Some(Focus::Part(0, 0)));
            assert_eq!(
                menu.step(Some(Focus::Part(0, 0)), true),
                Some(Focus::Item(0))
            );
            assert_eq!(Focus::Part(0, 0).index(), 0);
        }
    }

    #[test]
    fn mark_menu_lists_system_tools_then_power_and_session() {
        let mut menu = mark_menu();
        assert_eq!(
            commands(&menu),
            [
                Command::About,
                Command::Settings,
                Command::Open("ms-settings:"),
                Command::Open("ms-windows-store:"),
                Command::TaskManager,
                Command::ShowDesktop,
                Command::Sleep,
                Command::Restart,
                Command::ShutDown,
                Command::Lock,
                Command::SignOut,
            ]
        );
        // Only the commands that end the session are drawn as dangerous.
        for entry in &menu.entries {
            if let Entry::Item(item) = entry {
                let ends = matches!(
                    item.command,
                    Some(Command::Restart | Command::ShutDown | Command::SignOut)
                );
                assert_eq!(item.danger, ends, "{}", item.label);
            }
        }
        let layout = MenuLayout::new(&mut menu, 96, measure);
        assert_eq!(layout.width, 220. + 2. * SHADOW);
        assert_eq!(layout.rows.len(), 15);
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
        let first = Some(Focus::Item(1));
        let last = Some(Focus::Item(4));
        assert_eq!(menu.step(None, true), first);
        assert_eq!(menu.step(None, false), last);
        assert_eq!(menu.step(first, true), last);
        assert_eq!(menu.step(last, true), first);
        assert_eq!(menu.step(first, false), last);
        let mut empty = Menu::<u8>::fixed(DROPDOWN_WIDTH);
        empty.header("t", "", Icon::None);
        assert_eq!(empty.step(None, true), None);
    }

    #[test]
    fn clock_menu_turns_its_calendar_and_offers_today_away_from_it() {
        let today = (2026, 11, 21);
        let menu = clock_menu(today, (2026, 11));
        let Some(Entry::Header {
            title, subtitle, ..
        }) = menu.entries.first()
        else {
            panic!("the clock menu opens with today's date");
        };
        assert_eq!(title, "11月21日 星期六");
        assert_eq!(subtitle, "农历十月十三 · 丙午马年");
        let index = menu
            .entries
            .iter()
            .position(|entry| matches!(entry, Entry::Calendar(_)))
            .expect("a calendar");
        let turn = |y, m| Some(Command::Calendar(y, m));
        assert_eq!(
            menu.part_command(index, CALENDAR_PREVIOUS).cloned(),
            turn(2026, 10)
        );
        assert_eq!(menu.part_command(index, CALENDAR_TODAY), None);
        assert_eq!(menu.page_step(true).cloned(), turn(2026, 12));
        let away = clock_menu(today, (2027, 3));
        assert_eq!(
            away.part_command(index, CALENDAR_TODAY).cloned(),
            turn(2026, 11)
        );
        assert_eq!(clock_menu(today, (2100, 12)).page_step(true), None);
        assert_eq!(clock_menu(today, (1901, 1)).page_step(false), None);
        // Keyboard focus reaches the arrows and the way back to today.
        let parts: Vec<_> = (0..4)
            .scan(None, |focus, _| {
                *focus = away.step(*focus, true);
                *focus
            })
            .collect();
        assert!(parts.contains(&Focus::Part(index, CALENDAR_TODAY)));

        for dpi in [96, 144, 192] {
            let mut menu = clock_menu(today, (2027, 3));
            let layout = MenuLayout::new(&mut menu, dpi, measure);
            let at = |(l, t, r, b): (f32, f32, f32, f32)| {
                let s = layout.scale;
                (((l + r) / 2. * s) as i32, ((t + b) / 2. * s) as i32)
            };
            for part in [CALENDAR_PREVIOUS, CALENDAR_TODAY, CALENDAR_NEXT] {
                let (x, y) = at(layout.calendar_button(index, part));
                assert_eq!(layout.hit(&menu, x, y), Hit::Part(index, part), "{dpi}");
            }
            let (x, y) = at(layout.calendar_cell(index, 10));
            assert_eq!(layout.hit(&menu, x, y), Hit::Inert);
            let last = layout.calendar_cell(index, calendar::WEEKS * 7 - 1);
            assert!(last.3 <= layout.rows[index].1);
            assert!(last.2 <= layout.panel().2 - PADDING);
        }
    }

    #[test]
    fn menus_keep_one_width_whatever_their_text() {
        // Long text ends in an ellipsis instead of widening the menu.
        for label in ["x".to_owned(), "x".repeat(400)] {
            let mut fixed = sample();
            fixed.item(label, Icon::None, Some(3));
            let fixed = MenuLayout::new(&mut fixed, 96, measure);
            assert_eq!(fixed.width, DROPDOWN_WIDTH + 2. * SHADOW);
        }
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
