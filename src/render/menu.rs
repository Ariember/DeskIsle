use std::time::Instant;

use windows::Win32::{
    Foundation::HWND,
    Graphics::{DirectComposition::*, DirectWrite::*},
    UI::WindowsAndMessaging::HICON,
};
use windows::core::Result;

use crate::ui::calendar::{self, Note};
use crate::ui::icon::Symbol;
use crate::ui::menu::{self, Calendar, Entry, Icon, Menu, MenuLayout, Slider};
use crate::ui::panel::Area;
use crate::ui::system::PLAYER_SYMBOL;

use super::canvas::{Canvas, Rgb, Text, paint, text_format};
use super::{Renderer, ease};

/// The panel's corner radius. The highlight inside it, inset by the panel padding,
/// uses a radius that keeps both corners concentric.
const MENU_RADIUS: f32 = 12.;
const HIGHLIGHT_RADIUS: f32 = MENU_RADIUS - menu::PADDING;
/// Diameter of the round backing every icon of a menu row sits on.
const ICON_BADGE: f32 = menu::ICON_BADGE;
/// How much of its round backing an icon covers.
const BADGE_ICON: f32 = 0.58;
/// A tray icon's image inside its round backing. Application images fill their
/// square, so they are drawn a little larger than DeskIsle's line icons.
const TRAY_BADGE_ICON: f32 = 16.;
/// The circle behind a tray icon the user keeps on the status bar. The menu palette is
/// monochrome, so its `accent` is near black (or white) and would swallow colorful
/// application icons; this is the same system blue the settings panel uses.
const TRAY_ON_LIGHT: Rgb = (0, 122, 255);
const TRAY_ON_DARK: Rgb = (10, 132, 255);
/// The icon at the start of a slider's track.
const SLIDER_ICON: f32 = 15.;
/// Marks a window in an application's list; filled for the foreground window.
const WINDOW_SYMBOL: Symbol = Symbol::Window;
/// The solid circle behind today in the calendar.
const TODAY_RADIUS: f32 = 17.;
const OPEN_DURATION: f32 = 0.12;
const HOVER_DURATION: f32 = 0.09;

/// The open context menu. Its surfaces are painted once; pointer movement only animates
/// the highlight's offset and opacity in the compositor.
pub(super) struct MenuView {
    // The target must outlive all commits to its root.
    _target: IDCompositionTarget,
    _root: IDCompositionVisual2,
    glow: IDCompositionVisual2,
    glow_fade: IDCompositionEffectGroup,
    accent: IDCompositionSurface,
    danger: IDCompositionSurface,
    scale: f32,
    highlight_width: f32,
    highlight_scale: IDCompositionScaleTransform,
    highlighted: bool,
    /// The highlight's vertical offset in pixels.
    y: Tween,
    /// Each slider's own layer by entry index, repainted alone while it is dragged.
    sliders: Vec<(usize, IDCompositionVisual2)>,
    /// The track size of the sliders in DIPs and the DPI they were painted at.
    track: (f32, f32, u32),
}

/// Mirrors an [`ease`] handed to the compositor, so a new one starts where it is now.
struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: f32,
}

impl Tween {
    fn new(from: f32, to: f32, duration: f32) -> Self {
        Self {
            from,
            to,
            start: Instant::now(),
            duration,
        }
    }

    fn at(value: f32) -> Self {
        Self::new(value, value, 1.)
    }

    fn now(&self) -> f32 {
        eased(self.from, self.to, self.start, self.duration)
    }
}

/// The value of an [`ease`] started at `start`, as the compositor shows it now.
fn eased(from: f32, to: f32, start: Instant, duration: f32) -> f32 {
    let t = (start.elapsed().as_secs_f32() / duration).clamp(0., 1.);
    to + (from - to) * (1. - t).powi(3)
}

impl Renderer {
    /// Width of one line of text in DIPs, estimated when DirectWrite cannot lay it out.
    pub fn measure(&self, text: &str, size: f32, bold: bool) -> f32 {
        let style = Text {
            size,
            bold,
            ..Text::default()
        };
        let units = text.encode_utf16().collect::<Vec<_>>();
        let measured = text_format(&self.write, style, size).and_then(|format| unsafe {
            let layout = self
                .write
                .CreateTextLayout(&units, &format, f32::MAX, f32::MAX)?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics)?;
            Ok(metrics.widthIncludingTrailingWhitespace)
        });
        measured.unwrap_or_else(|_| text.chars().count() as f32 * size * 0.62)
    }

    /// Paints `menu` into `hwnd`, a hidden menu window sized to `layout`, and fades it in
    /// when `animate`; a flyout rebuilt after a change appears in place instead.
    /// `tray` resolves tray icon indices while the tray host keeps owning the handles.
    pub fn open_menu<C>(
        &mut self,
        hwnd: HWND,
        menu: &Menu<C>,
        layout: &MenuLayout,
        dpi: u32,
        tray: &dyn Fn(usize) -> Option<HICON>,
        animate: bool,
    ) -> Result<()> {
        self.close_menu();
        let (width, height) = layout.pixels();
        let (left, top, right, bottom) = layout.panel();
        let background = paint(&self.device, self.palette, width, height, dpi, |c| {
            c.shadow(left, top, right, bottom, MENU_RADIUS, menu::SHADOW - 2.)?;
            c.gradient(
                left,
                top,
                right,
                bottom,
                MENU_RADIUS,
                (c.p.panel_top, 0.985),
                (c.p.panel, 0.985),
            )?;
            c.stroke(left, top, right, bottom, MENU_RADIUS, c.p.line, 0.12)
        })?;
        let content = paint(&self.device, self.palette, width, height, dpi, |c| {
            for (index, entry) in menu.entries.iter().enumerate() {
                self.paint_entry(c, entry, layout, index, tray)?;
            }
            Ok(())
        })?;
        let mut sliders = Vec::new();
        let mut track = (0., 0., dpi);
        for (index, entry) in menu.entries.iter().enumerate() {
            if let Entry::Slider(slider) = entry {
                let (l, t, r, b) = layout.track(index);
                track = (r - l, b - t, dpi);
                let surface = self.slider_surface(slider, track)?;
                unsafe {
                    let visual = self.device.CreateVisual()?;
                    visual.SetContent(&surface)?;
                    visual.SetOffsetX2((l * layout.scale).round())?;
                    visual.SetOffsetY2((t * layout.scale).round())?;
                    sliders.push((index, visual));
                }
            }
        }
        let (row_left, _, row_right, _) = layout.row(0);
        // A plain rounded wash, the same shape the status bar uses behind its targets.
        let highlight = |accent: Rgb, alpha: f32| {
            paint(
                &self.device,
                self.palette,
                ((row_right - row_left) * layout.scale).round() as i32,
                (menu::ITEM_HEIGHT * layout.scale).round() as i32,
                dpi,
                |c| {
                    let (w, h) = (row_right - row_left, menu::ITEM_HEIGHT);
                    c.fill(0., 1., w, h - 1., HIGHLIGHT_RADIUS, accent, alpha)
                },
            )
        };
        let accent = highlight(self.palette.accent, 0.1)?;
        let danger = highlight(self.palette.danger, 0.12)?;
        unsafe {
            let target = self.device.CreateTargetForHwnd(hwnd, true)?;
            let root = self.device.CreateVisual()?;
            root.SetContent(&background)?;
            let glow = self.device.CreateVisual()?;
            glow.SetContent(&accent)?;
            glow.SetOffsetX2((row_left * layout.scale).round())?;
            let highlight_scale = self.device.CreateScaleTransform()?;
            glow.SetTransform(&highlight_scale)?;
            let glow_fade = self.device.CreateEffectGroup()?;
            glow_fade.SetOpacity2(0.)?;
            glow.SetEffect(&glow_fade)?;
            let text = self.device.CreateVisual()?;
            text.SetContent(&content)?;
            root.AddVisual(&glow, true, None)?;
            root.AddVisual(&text, true, None)?;
            for (_, visual) in &sliders {
                root.AddVisual(visual, true, None)?;
            }

            if animate {
                // Open with a quick fade and a slight zoom toward the full size.
                let fade = self.device.CreateEffectGroup()?;
                fade.SetOpacity(&ease(&self.device, 0., 1., OPEN_DURATION)?)?;
                root.SetEffect(&fade)?;
                let zoom = self.device.CreateScaleTransform()?;
                zoom.SetCenterX2(width as f32 / 2.)?;
                zoom.SetCenterY2(height as f32 / 2.)?;
                let grow = ease(&self.device, 0.96, 1., OPEN_DURATION)?;
                zoom.SetScaleX(&grow)?;
                zoom.SetScaleY(&grow)?;
                root.SetTransform(&zoom)?;
            }
            target.SetRoot(&root)?;
            self.device.Commit()?;
            self.menu = Some(MenuView {
                _target: target,
                _root: root,
                glow,
                glow_fade,
                accent,
                danger,
                scale: layout.scale,
                highlight_width: row_right - row_left,
                highlight_scale,
                highlighted: false,
                y: Tween::at(0.),
                sliders,
                track,
            });
        }
        Ok(())
    }

    /// A slider's track, fill, knob and icon, `(width, height, dpi)` in size.
    fn slider_surface<C>(
        &self,
        slider: &Slider<C>,
        (w, h, dpi): (f32, f32, u32),
    ) -> Result<IDCompositionSurface> {
        let s = dpi as f32 / 96.;
        let size = ((w * s).round() as i32, (h * s).round() as i32);
        paint(&self.device, self.palette, size.0, size.1, dpi, |c| {
            let radius = h / 2.;
            c.fill(0., 0., w, h, radius, c.p.line, 0.1)?;
            let button = Area::new(0., 0., h, h);
            let Some(value) = slider.value else {
                return c.symbol_in(slider.symbol, button, SLIDER_ICON, c.p.muted, 0.6);
            };
            let x = radius + (w - h) * f32::from(value.min(100)) / 100.;
            c.fill(0., 0., x + radius, h, radius, c.p.accent, 1.)?;
            let knob = radius - 1.5;
            c.fill(
                x - knob,
                radius - knob,
                x + knob,
                radius + knob,
                knob,
                c.p.panel_top,
                1.,
            )?;
            c.stroke(
                x - knob,
                radius - knob,
                x + knob,
                radius + knob,
                knob,
                c.p.line,
                0.25,
            )?;
            // The knob covers the icon at the low end of the range.
            let ink = if x < h { c.p.ink } else { c.p.on_accent };
            c.symbol_in(slider.symbol, button, SLIDER_ICON, ink, 1.)
        })
    }

    /// Repaints the slider of entry `index` after its value changed.
    pub fn menu_slider<C>(&mut self, index: usize, slider: &Slider<C>) -> Result<()> {
        let Some(track) = self.menu.as_ref().map(|view| view.track) else {
            return Ok(());
        };
        let surface = self.slider_surface(slider, track)?;
        let view = self.menu.as_ref().expect("menu checked above");
        if let Some((_, visual)) = view.sliders.iter().find(|(i, _)| *i == index) {
            unsafe {
                visual.SetContent(&surface)?;
                self.device.Commit()?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn text_at(
        &self,
        c: &Canvas,
        text: &str,
        a: Area,
        size: f32,
        bold: bool,
        ink: Rgb,
        alpha: f32,
    ) -> Result<()> {
        c.draw_text(
            &self.write,
            text,
            c.rect(a.left, a.top, a.right, a.bottom),
            Text {
                size,
                bold,
                ..Text::default()
            },
            ink,
            alpha,
        )
    }

    /// One line of text centered in `a`.
    #[allow(clippy::too_many_arguments)]
    fn centered(
        &self,
        c: &Canvas,
        text: &str,
        a: Area,
        size: f32,
        bold: bool,
        ink: Rgb,
        alpha: f32,
    ) -> Result<()> {
        c.draw_text(
            &self.write,
            text,
            c.rect(a.left, a.top, a.right, a.bottom),
            Text {
                size,
                bold,
                align: DWRITE_TEXT_ALIGNMENT_CENTER,
            },
            ink,
            alpha,
        )
    }

    /// The month and its buttons, the weekday names and six weeks of days. Today sits on
    /// a solid circle; weekends are muted and days of the neighboring months faint. Under
    /// each number, festivals read in red, solar terms in full ink and lunar days muted.
    fn paint_calendar<C>(
        &self,
        c: &Canvas,
        calendar: &Calendar<C>,
        layout: &MenuLayout,
        index: usize,
    ) -> Result<()> {
        let month = &calendar.month;
        let (l, t, _, b) = layout.calendar_nav(index);
        let (buttons_left, ..) = layout.calendar_button(index, menu::CALENDAR_PREVIOUS);
        let title = Area::new(l + 6., t, buttons_left - 4., b);
        self.text_at(c, &month.title(), title, 15., true, c.p.ink, 1.)?;
        for (part, command) in calendar.buttons.iter().enumerate() {
            let (bl, bt, br, bb) = layout.calendar_button(index, part);
            let area = Area::new(bl, bt, br, bb);
            if part == menu::CALENDAR_TODAY {
                if command.is_some() {
                    c.fill(bl + 3., bt + 3., br - 3., bb - 3., 11., c.p.line, 0.07)?;
                    self.centered(c, "今天", area, 12., false, c.p.ink, 1.)?;
                }
                continue;
            }
            let symbol = if part == menu::CALENDAR_PREVIOUS {
                Symbol::ChevronLeft
            } else {
                Symbol::ChevronRight
            };
            let alpha = if command.is_some() { 1. } else { 0.3 };
            c.symbol_in(symbol, area, 16., c.p.ink, alpha)?;
        }
        for (column, name) in calendar::WEEKDAYS.iter().enumerate() {
            let (wl, wt, wr, wb) = layout.calendar_weekday(index, column);
            let alpha = if matches!(column, 0 | 6) { 0.65 } else { 1. };
            let area = Area::new(wl, wt, wr, wb);
            self.centered(c, name, area, 11.5, false, c.p.muted, alpha)?;
        }
        for (cell, day) in month.days.iter().enumerate() {
            let (cl, ct, cr, cb) = layout.calendar_cell(index, cell);
            let (x, y) = ((cl + cr) / 2., (ct + cb) / 2.);
            let fade = if day.current { 1. } else { 0.32 };
            let today = day.today && day.current;
            if today {
                let r = TODAY_RADIUS;
                c.fill(x - r, y - r, x + r, y + r, r, c.p.accent, 1.)?;
            }
            let (number, note, note_alpha) = match (today, day.kind) {
                (true, _) => (c.p.on_accent, c.p.on_accent, 0.85),
                (false, Note::Festival) => (c.p.ink, c.p.danger, 1.),
                (false, Note::Term) => (c.p.ink, c.p.ink, 0.9),
                (false, Note::Lunar) => (c.p.ink, c.p.muted, 0.8),
            };
            let number = if day.weekend && !today {
                c.p.muted
            } else {
                number
            };
            let digits = Area::new(cl, y - 16., cr, y + 2.);
            let text = day.day.to_string();
            self.centered(c, &text, digits, 14.5, today, number, fade)?;
            let below = Area::new(cl + 1., y + 2., cr - 1., y + 15.);
            self.centered(c, &day.note, below, 9.5, false, note, note_alpha * fade)?;
        }
        Ok(())
    }

    /// A circle of `size` centered on `x`, `y`: filled with the accent when `on`,
    /// otherwise a faint wash.
    fn backing(c: &Canvas, x: f32, y: f32, size: f32, on: bool) -> Result<()> {
        let r = size / 2.;
        if on {
            c.fill(x - r, y - r, x + r, y + r, r, c.p.accent, 1.)
        } else {
            c.fill(x - r, y - r, x + r, y + r, r, c.p.line, 0.09)
        }
    }

    /// An icon in a circle, filled with the accent when `on`.
    fn badge(&self, c: &Canvas, symbol: Symbol, x: f32, y: f32, size: f32, on: bool) -> Result<()> {
        Self::backing(c, x, y, size, on)?;
        let ink = if on { c.p.on_accent } else { c.p.ink };
        c.symbol(symbol, x, y, size * BADGE_ICON, ink, 1.)
    }

    /// A tray icon's image, on a blue circle while the user keeps it `on` the status
    /// bar. A registration without an image uses the same dot as the bar.
    fn tray_badge(&self, c: &Canvas, icon: Option<HICON>, x: f32, y: f32, on: bool) -> Result<()> {
        if on {
            let r = ICON_BADGE / 2.;
            let tint = if c.p.dark {
                TRAY_ON_DARK
            } else {
                TRAY_ON_LIGHT
            };
            c.fill(x - r, y - r, x + r, y + r, r, tint, 1.)?;
        }
        let Some(bitmap) = icon.and_then(|icon| self.hicon_bitmap(icon).ok()) else {
            let ink = if on { c.p.on_accent } else { c.p.muted };
            return c.round(x - 3., y - 3., x + 3., y + 3., 3., ink);
        };
        // Snap to whole pixels so small icons stay sharp.
        let px = |v: f32| (v * c.scale).round() / c.scale;
        let (l, t) = (x - TRAY_BADGE_ICON / 2., y - TRAY_BADGE_ICON / 2.);
        c.bitmap(
            &bitmap,
            c.rect(
                px(l),
                px(t),
                px(l + TRAY_BADGE_ICON),
                px(t + TRAY_BADGE_ICON),
            ),
        )
    }

    /// The icon of a command row on a faint circle of its own `tint`, so a destructive
    /// command reads red and a disabled one fades with its label.
    #[allow(clippy::too_many_arguments)]
    fn soft_badge(
        &self,
        c: &Canvas,
        symbol: Symbol,
        x: f32,
        y: f32,
        tint: Rgb,
        alpha: f32,
    ) -> Result<()> {
        let r = ICON_BADGE / 2.;
        c.fill(x - r, y - r, x + r, y + r, r, tint, 0.1 * alpha)?;
        c.symbol(symbol, x, y, ICON_BADGE * BADGE_ICON, tint, alpha)
    }

    fn paint_entry<C>(
        &self,
        c: &Canvas,
        entry: &Entry<C>,
        layout: &MenuLayout,
        index: usize,
        tray: &dyn Fn(usize) -> Option<HICON>,
    ) -> Result<()> {
        let (row_left, _, row_right, _) = layout.row(0);
        let (top, bottom) = layout.rows[index];
        let middle = (top + bottom) / 2.;
        // Every row's leading icon sits where the layout takes its clicks.
        let (badge_left, _, badge_right, _) = layout.icon_badge(index);
        let icon_x = (badge_left + badge_right) / 2.;
        match entry {
            Entry::Label(text) => {
                let area = Area::new(row_left + 8., top + 4., row_right - 8., bottom);
                self.text_at(c, text, area, menu::DETAIL_SIZE, true, c.p.muted, 1.)
            }
            // Drawn on its own layer.
            Entry::Slider(_) => Ok(()),
            Entry::Calendar(calendar) => self.paint_calendar(c, calendar, layout, index),
            Entry::Tiles(tiles) => {
                for (part, tile) in tiles.iter().enumerate() {
                    let (l, t, r, b) = layout.tile(index, part, tiles.len());
                    c.fill(l, t, r, b, 10., c.p.line, 0.055)?;
                    c.stroke(l, t, r, b, 10., c.p.line, 0.07)?;
                    let y = (t + b) / 2.;
                    // Three tiles share the fixed dropdown width; keep their text room.
                    self.badge(c, tile.symbol, l + 6. + 14., y, 28., tile.on)?;
                    let text = Area::new(l + 42., t, r - 4., y + 1.);
                    let alpha = if tile.command.is_some() { 1. } else { 0.5 };
                    self.text_at(
                        c,
                        &tile.title,
                        Area {
                            top: y - 17.,
                            ..text
                        },
                        12.,
                        true,
                        c.p.ink,
                        alpha,
                    )?;
                    let detail = Area::new(l + 42., y, r - 4., y + 16.);
                    self.text_at(c, &tile.detail, detail, 11., false, c.p.muted, 0.9)?;
                }
                Ok(())
            }
            Entry::Player {
                title,
                detail,
                buttons,
            } => {
                let (l, t, r, b) = layout.block(index);
                c.fill(l, t, r, b, 10., c.p.line, 0.055)?;
                c.stroke(l, t, r, b, 10., c.p.line, 0.07)?;
                let y = (t + b) / 2.;
                c.fill(l + 8., y - 18., l + 44., y + 18., 8., c.p.line, 0.08)?;
                let art = Area::new(l + 8., y - 18., l + 44., y + 18.);
                c.symbol_in(PLAYER_SYMBOL, art, 18., c.p.muted, 1.)?;
                let text_right = if buttons.is_empty() {
                    r - 8.
                } else {
                    layout.player_button(index, 0, buttons.len()).0 - 4.
                };
                let lines = if detail.is_empty() { 0. } else { 9. };
                let name = Area::new(l + 54., y - 12. - lines, text_right, y + 12. - lines);
                self.text_at(c, title, name, menu::LABEL_SIZE, true, c.p.ink, 1.)?;
                let by = Area::new(l + 54., y, text_right, y + 20.);
                self.text_at(c, detail, by, menu::DETAIL_SIZE, false, c.p.muted, 0.85)?;
                for (part, &(symbol, ref command)) in buttons.iter().enumerate() {
                    let (bl, bt, br, bb) = layout.player_button(index, part, buttons.len());
                    let alpha = if command.is_some() { 1. } else { 0.35 };
                    c.symbol_in(symbol, Area::new(bl, bt, br, bb), 16., c.p.ink, alpha)?;
                }
                Ok(())
            }
            Entry::Separator => c.fill(
                row_left + 8.,
                middle - 0.5,
                row_right - 8.,
                middle + 0.5,
                0.,
                c.p.line,
                0.09,
            ),
            Entry::Header {
                title,
                subtitle,
                icon,
            } => {
                let size = 30.;
                let x = row_left + 6.;
                let drawn = self.paint_icon(c, icon, x, middle - size / 2., size, tray)?;
                let text_x = if drawn { x + size + 10. } else { row_left + 8. };
                let lines = if subtitle.is_empty() { 0. } else { 9. };
                c.draw_text(
                    &self.write,
                    title,
                    c.rect(
                        text_x,
                        middle - 12. - lines,
                        row_right - 8.,
                        middle + 12. - lines,
                    ),
                    Text {
                        size: menu::TITLE_SIZE,
                        bold: true,
                        ..Text::default()
                    },
                    c.p.ink,
                    1.,
                )?;
                c.draw_text(
                    &self.write,
                    subtitle,
                    c.rect(text_x, middle, row_right - 8., middle + 20.),
                    Text {
                        size: menu::DETAIL_SIZE,
                        ..Text::default()
                    },
                    c.p.muted,
                    0.85,
                )
            }
            Entry::Item(item) => {
                let enabled = item.command.is_some();
                let (ink, alpha) = match (item.danger, enabled) {
                    (_, false) => (c.p.muted, 0.5),
                    (true, true) => (c.p.danger, 1.),
                    (false, true) => (c.p.ink, 1.),
                };
                match &item.icon {
                    // Icons, options and windows all sit on the same round backing.
                    Icon::Symbol(symbol) => {
                        let tint = if item.danger { c.p.danger } else { c.p.ink };
                        self.soft_badge(c, *symbol, icon_x, middle, tint, alpha)?;
                    }
                    Icon::Choice { symbol, on } => {
                        self.badge(c, *symbol, icon_x, middle, ICON_BADGE, *on)?;
                    }
                    Icon::Window { active } => {
                        self.badge(c, WINDOW_SYMBOL, icon_x, middle, ICON_BADGE, *active)?;
                    }
                    Icon::Tray { index, on } => {
                        self.tray_badge(c, tray(*index), icon_x, middle, *on)?;
                    }
                    icon => {
                        let size = 18.;
                        self.paint_icon(
                            c,
                            icon,
                            icon_x - size / 2.,
                            middle - size / 2.,
                            size,
                            tray,
                        )?;
                    }
                }
                let label_left = row_left + 8. + layout.icon_column;
                let label_right = row_right - menu::TEXT_INSET;
                // Long details, such as live tooltips, end in an ellipsis and leave the
                // label at least half the row.
                let detail_width = if item.detail.is_empty() {
                    0.
                } else {
                    (self.measure(&item.detail, menu::DETAIL_SIZE, false) + 16.)
                        .min((label_right - label_left) / 2.)
                };
                c.draw_text(
                    &self.write,
                    &item.label,
                    c.rect(label_left, top, label_right - detail_width, bottom),
                    Text {
                        size: menu::LABEL_SIZE,
                        ..Text::default()
                    },
                    ink,
                    alpha,
                )?;
                c.draw_text(
                    &self.write,
                    &item.detail,
                    c.rect(label_right - detail_width + 16., top, label_right, bottom),
                    Text {
                        size: menu::DETAIL_SIZE,
                        align: DWRITE_TEXT_ALIGNMENT_TRAILING,
                        ..Text::default()
                    },
                    c.p.muted,
                    0.75 * alpha,
                )
            }
        }
    }

    /// Draws a file or tray icon in a DIP square. Returns whether anything was drawn.
    fn paint_icon(
        &self,
        c: &Canvas,
        icon: &Icon,
        x: f32,
        y: f32,
        size: f32,
        tray: &dyn Fn(usize) -> Option<HICON>,
    ) -> Result<bool> {
        let bitmap = match icon {
            Icon::File(path) => self.shell_icon(path).ok().flatten(),
            Icon::Tray { index, .. } => tray(*index).and_then(|icon| self.hicon_bitmap(icon).ok()),
            Icon::Symbol(symbol) => {
                // Round like every other icon backing in the menus.
                let r = size / 2.;
                c.fill(x, y, x + size, y + size, r, c.p.accent, 0.12)?;
                c.symbol(*symbol, x + r, y + r, size * 0.55, c.p.accent, 1.)?;
                return Ok(true);
            }
            Icon::None | Icon::Window { .. } | Icon::Choice { .. } => None,
        };
        match bitmap {
            Some(bitmap) => {
                // Snap to whole pixels so small icons stay sharp.
                let px = |v: f32| (v * c.scale).round() / c.scale;
                c.bitmap(&bitmap, c.rect(px(x), px(y), px(x + size), px(y + size)))?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Moves the highlight to an enabled item, button or track.
    pub fn menu_hover(&mut self, bounds: Option<(f32, f32, f32, f32)>, danger: bool) -> Result<()> {
        let Some(view) = self.menu.as_mut() else {
            return Ok(());
        };
        unsafe {
            match bounds {
                None => view
                    .glow_fade
                    .SetOpacity(&ease(&self.device, 1., 0., HOVER_DURATION)?)?,
                Some((l, t, r, b)) => {
                    let y = (t * view.scale).round();
                    view.glow.SetOffsetX2((l * view.scale).round())?;
                    view.highlight_scale
                        .SetScaleX2((r - l) / view.highlight_width)?;
                    view.highlight_scale
                        .SetScaleY2((b - t) / menu::ITEM_HEIGHT)?;
                    view.glow
                        .SetContent(if danger { &view.danger } else { &view.accent })?;
                    if view.highlighted {
                        let from = view.y.now();
                        view.glow
                            .SetOffsetY(&ease(&self.device, from, y, HOVER_DURATION)?)?;
                        view.y = Tween::new(from, y, HOVER_DURATION);
                    } else {
                        view.glow.SetOffsetY2(y)?;
                        view.y = Tween::at(y);
                        view.glow_fade
                            .SetOpacity(&ease(&self.device, 0., 1., HOVER_DURATION)?)?;
                    }
                }
            }
            self.device.Commit()?;
        }
        view.highlighted = bounds.is_some();
        Ok(())
    }

    /// Releases the menu's composition target; call before destroying its window.
    pub fn close_menu(&mut self) {
        if self.menu.take().is_some() {
            let _ = unsafe { self.device.Commit() };
        }
    }
}
