use std::borrow::Cow;
use windows::Win32::{
    Foundation::HWND,
    Globalization::{LCMAP_TRADITIONAL_CHINESE, LCMapStringEx},
    Graphics::{DirectComposition::*, DirectWrite::*},
};
use windows::core::{Error, Result, w};

use crate::settings::{
    Adjust, DOCK_DISPLAY_MODES, Key, MonitorKey, SECTIONS, Section, TopBarBackground,
};
use crate::ui::icon::Symbol;
use crate::ui::panel::{self, Action, Area, PanelLayout, Part};

use super::Renderer;
use super::canvas::{Canvas, Rgb, Text, accent_color, paint};

const WHITE: Rgb = (255, 255, 255);
const BLACK: Rgb = (0, 0, 0);
/// The macOS close light, its rim and the cross it shows under the pointer.
const CLOSE_LIGHT: Rgb = (255, 95, 87);
const CLOSE_CROSS: Rgb = (110, 12, 8);
const LIGHT_RADIUS: f32 = 6.;
const GROUP_RADIUS: f32 = 10.;

/// Only panel-owned captions pass here. Each language keeps its own name in the selector.
fn panel_caption(text: &str, traditional: bool) -> Result<Cow<'_, str>> {
    if !traditional || text.is_ascii() || matches!(text, "简体中文" | "繁體中文") {
        return Ok(Cow::Borrowed(text));
    }
    let source: Vec<_> = text.encode_utf16().collect();
    let map = |destination: Option<&mut [u16]>| unsafe {
        LCMapStringEx(
            w!("zh-TW"),
            LCMAP_TRADITIONAL_CHINESE,
            &source,
            destination,
            None,
            None,
            Default::default(),
        )
    };
    let size = map(None);
    if size == 0 {
        return Err(Error::from_thread());
    }
    let mut result = vec![0; size as usize];
    if map(Some(&mut result)) == 0 {
        return Err(Error::from_thread());
    }
    Ok(String::from_utf16_lossy(&result).into())
}

fn section_color(section: Section) -> Rgb {
    match section {
        Section::General => (87, 102, 116),
        Section::Appearance => (154, 84, 159),
        Section::Dock => (27, 112, 178),
        Section::TopBar => (94, 83, 166),
        Section::Clock => (15, 128, 138),
        Section::Monitor => (198, 64, 78),
        Section::About => (114, 120, 130),
    }
}

/// The icon on a page's colored tile in the sidebar.
fn section_symbol(section: Section) -> Symbol {
    match section {
        Section::General => Symbol::Computer,
        Section::Appearance => Symbol::Palette,
        Section::Dock => Symbol::Dock,
        Section::TopBar => Symbol::Window,
        Section::Clock => Symbol::Clock,
        Section::Monitor => Symbol::Chip,
        Section::About => Symbol::Info,
    }
}

/// Blends `a` toward `b` by `t`, for the highlight at the top of an icon tile.
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

/// A small desktop with a colorful wallpaper so each top-bar style's transparency and
/// contrast are immediately recognizable.
fn top_bar_preview(c: &Canvas, a: Area, style: TopBarBackground) -> Result<()> {
    let (l, t, r, b) = (a.left, a.top, a.right, a.bottom);
    let w = r - l;
    let bar_h = 18.;
    let mid = t + bar_h / 2.;
    let palette = c.p.for_top_bar(style, None);
    let shadow = super::top_bar_shadow(style, None);
    let (sr, sa) = if palette.light_ground() {
        ((255, 255, 255), 0.55)
    } else {
        ((0, 0, 0), 0.38)
    };
    let dy = 0.7;
    c.clipped_round(l, t, r, b, 7., || {
        c.gradient(l, t, r, b, 0., ((118, 152, 186), 1.), ((184, 206, 222), 1.))?;
        for (x, y, start, end) in [
            (0.30, 5., (156, 114, 232), (52, 68, 158)),
            (0.17, 15., (224, 134, 212), (132, 54, 134)),
            (0.02, 26., (255, 174, 146), (197, 79, 119)),
            (-0.18, 37., (255, 212, 138), (231, 114, 90)),
        ] {
            let band = |spread: f32, alpha| {
                c.gradient(
                    l + w * x - spread,
                    t + y - spread,
                    l + w * (x + 0.66) + spread,
                    b + 34. + spread,
                    32. + spread,
                    (start, alpha),
                    (end, alpha),
                )
            };
            if style == TopBarBackground::Frosted {
                // Feather the miniature's wallpaper, which is drawn inside the panel
                // rather than behind a separate window with a real DWM backdrop.
                for (spread, alpha) in [
                    (6., 0.08),
                    (4., 0.16),
                    (2., 0.24),
                    (0., 0.32),
                    (-2., 0.44),
                    (-4., 0.67),
                    (-6., 1.),
                ] {
                    band(spread, alpha)?;
                }
            } else {
                band(0., 1.)?;
            }
        }
        c.top_background(Area::new(l, t, r, t + bar_h), style, true)?;
        if shadow {
            c.ring_alpha(l + 12., mid + dy, 3.6, 1.5, sr, sa)?;
            c.symbol(Symbol::Volume(3), r - 36., mid + dy, 9.5, sr, sa)?;
            c.symbol(Symbol::Search, r - 22.5, mid + dy, 9., sr, sa)?;
            c.symbol(Symbol::Controls, r - 9.5, mid + dy, 10., sr, sa)?;
        }
        c.ring(l + 12., mid, 3.6, 1.5, palette.accent)?;
        c.symbol(Symbol::Volume(3), r - 36., mid, 9.5, palette.ink, 1.)?;
        c.symbol(Symbol::Search, r - 22.5, mid, 9., palette.ink, 1.)?;
        c.symbol(Symbol::Controls, r - 9.5, mid, 10., palette.ink, 1.)
    })
}

/// The settings window's tones, modeled on macOS System Settings: a tinted sidebar,
/// a plain content area and rows grouped into one rounded list.
struct Tones {
    window: Rgb,
    sidebar_top: Rgb,
    sidebar_bottom: Rgb,
    /// Laid over the acrylic backdrop, which already brings its own tint and noise.
    sidebar_glass: (Rgb, f32),
    divider: (Rgb, f32),
    /// Alphas of `line` for the grouped list's fill and rim.
    group: f32,
    group_rim: f32,
    separator: f32,
    accent: Rgb,
    /// Push buttons and the selected segment.
    face: Rgb,
    face_rim: (Rgb, f32),
    face_shadow: f32,
    /// Alpha of `line` for a switch that is off and for segment tracks.
    track: f32,
    /// The minimize and zoom lights, which the panel does not offer.
    idle_light: Rgb,
    light_rim: f32,
}

impl Tones {
    fn new(dark: bool, accent: Rgb) -> Self {
        if dark {
            Self {
                window: (30, 30, 31),
                sidebar_top: (50, 49, 53),
                sidebar_bottom: (40, 39, 43),
                sidebar_glass: ((44, 43, 48), 0.3),
                divider: (BLACK, 0.55),
                group: 0.05,
                group_rim: 0.07,
                separator: 0.08,
                accent,
                face: (92, 92, 95),
                face_rim: (WHITE, 0.08),
                face_shadow: 0.3,
                track: 0.16,
                idle_light: (78, 78, 82),
                light_rim: 0.35,
            }
        } else {
            Self {
                window: (248, 248, 248),
                sidebar_top: (237, 235, 241),
                sidebar_bottom: (227, 225, 232),
                sidebar_glass: ((236, 234, 240), 0.28),
                divider: (BLACK, 0.1),
                group: 0.035,
                group_rim: 0.06,
                separator: 0.07,
                accent,
                face: WHITE,
                face_rim: (BLACK, 0.13),
                face_shadow: 0.07,
                track: 0.1,
                idle_light: (208, 207, 212),
                light_rim: 0.12,
            }
        }
    }
}

/// The settings panel's composition target. The panel repaints one surface per change;
/// it changes only on input, so nothing runs while it sits open.
pub(super) struct PanelView {
    hwnd: HWND,
    // The target must outlive all commits to its root.
    _target: IDCompositionTarget,
    root: IDCompositionVisual2,
}

impl Renderer {
    /// Paints `layout` into `hwnd`, the settings window, binding to it on first use and
    /// again after the device was rebuilt. With `glass` the sidebar is left translucent
    /// over the backdrop DWM draws behind it.
    pub fn draw_panel(
        &mut self,
        hwnd: HWND,
        layout: &PanelLayout,
        dpi: u32,
        glass: bool,
    ) -> Result<()> {
        if self.panel.as_ref().map(|view| view.hwnd) != Some(hwnd) {
            self.close_panel();
            unsafe {
                let target = self.device.CreateTargetForHwnd(hwnd, true)?;
                let root = self.device.CreateVisual()?;
                target.SetRoot(&root)?;
                self.panel = Some(PanelView {
                    hwnd,
                    _target: target,
                    root,
                });
            }
        }
        let (width, height) = layout.pixels();
        let surface = paint(&self.device, self.palette, width, height, dpi, |c| {
            self.paint_panel(c, layout, glass)
        })?;
        let view = self.panel.as_ref().expect("panel bound above");
        unsafe {
            view.root.SetContent(&surface)?;
            self.device.Commit()
        }
    }

    /// Releases the panel's composition target; call before destroying its window.
    pub fn close_panel(&mut self) {
        if self.panel.take().is_some() {
            let _ = unsafe { self.device.Commit() };
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn label(
        &self,
        c: &Canvas,
        text: &str,
        a: Area,
        size: f32,
        bold: bool,
        ink: Rgb,
        alpha: f32,
    ) -> Result<()> {
        self.label_aligned(
            c,
            text,
            a,
            size,
            bold,
            ink,
            alpha,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn label_aligned(
        &self,
        c: &Canvas,
        text: &str,
        a: Area,
        size: f32,
        bold: bool,
        ink: Rgb,
        alpha: f32,
        align: DWRITE_TEXT_ALIGNMENT,
    ) -> Result<()> {
        let style = Text { size, bold, align };
        c.draw_text(
            &self.write,
            &panel_caption(text, self.traditional_panel)?,
            c.rect(a.left, a.top, a.right, a.bottom),
            style,
            ink,
            alpha,
        )
    }

    /// A macOS push button: a raised face with a hairline rim and a soft drop below.
    fn button_face(&self, c: &Canvas, t: &Tones, a: Area, radius: f32, lit: bool) -> Result<()> {
        c.fill(
            a.left,
            a.top + 0.5,
            a.right,
            a.bottom + 1.,
            radius,
            BLACK,
            t.face_shadow,
        )?;
        c.round(a.left, a.top, a.right, a.bottom, radius, t.face)?;
        if lit {
            c.fill(a.left, a.top, a.right, a.bottom, radius, c.p.line, 0.05)?;
        }
        c.stroke(
            a.left,
            a.top,
            a.right,
            a.bottom,
            radius,
            t.face_rim.0,
            t.face_rim.1,
        )
    }

    fn paint_panel(&self, c: &Canvas, layout: &PanelLayout, glass: bool) -> Result<()> {
        let t = Tones::new(c.p.dark, c.p.accent);
        let (w, h) = (layout.width, layout.height);
        let hover = |action: Action| layout.hover == Some(action);
        let side = panel::SIDEBAR;
        c.round(side, 0., w, h, 0., t.window)?;

        // The sidebar runs the full height, under the window lights, as in macOS.
        if glass {
            let (tint, alpha) = t.sidebar_glass;
            let glass_strength = layout.backdrop as f32 / 100.;
            c.fill(
                0.,
                0.,
                side,
                h,
                0.,
                tint,
                1. - (1. - alpha) * glass_strength,
            )?;
        } else {
            c.gradient(
                0.,
                0.,
                side,
                h,
                0.,
                (t.sidebar_top, 1.),
                (t.sidebar_bottom, 1.),
            )?;
        }
        c.fill(
            side,
            0.,
            side + 1. / c.scale,
            h,
            0.,
            t.divider.0,
            t.divider.1,
        )?;

        // Window lights: close is live, minimize and zoom are drawn disabled because the
        // panel offers neither.
        let close = layout.close;
        let (x0, mid) = ((close.left + close.right) / 2., close.middle());
        for i in 0..3 {
            let x = x0 + i as f32 * panel::LIGHT_GAP;
            let r = LIGHT_RADIUS;
            let fill = if i == 0 { CLOSE_LIGHT } else { t.idle_light };
            c.round(x - r, mid - r, x + r, mid + r, r, fill)?;
            c.stroke(x - r, mid - r, x + r, mid + r, r, BLACK, t.light_rim)?;
        }
        if hover(Action::Close) {
            let cross = Area::new(x0 - 6., mid - 6., x0 + 6., mid + 6.);
            c.symbol_in(Symbol::Close, cross, 10., CLOSE_CROSS, 0.9)?;
        }

        for (index, &(a, section)) in layout.nav.iter().enumerate() {
            let selected = index == layout.page;
            if selected {
                c.round(a.left, a.top, a.right, a.bottom, 6., t.accent)?;
            } else if hover(Action::Page(index)) {
                c.fill(a.left, a.top, a.right, a.bottom, 6., c.p.line, 0.06)?;
            }
            let tile = Area::new(
                a.left + 8.,
                a.middle() - 10.,
                a.left + 28.,
                a.middle() + 10.,
            );
            let base = section_color(section);
            c.gradient(
                tile.left,
                tile.top,
                tile.right,
                tile.bottom,
                5.,
                (mix(base, WHITE, 0.22), 1.),
                (base, 1.),
            )?;
            c.stroke(
                tile.left,
                tile.top,
                tile.right,
                tile.bottom,
                5.,
                BLACK,
                0.08,
            )?;
            c.symbol_in(section_symbol(section), tile, 14., WHITE, 1.)?;
            let text = Area {
                left: a.left + 37.,
                right: a.right - 8.,
                ..a
            };
            let ink = if selected { WHITE } else { c.p.ink };
            self.label(c, section.title(), text, 13., false, ink, 1.)?;
        }

        let view = layout.view;
        let section = SECTIONS[layout.page];
        self.label(c, section.title(), layout.title, 15., true, c.p.ink, 1.)?;
        if let Some(about) = &layout.about {
            return self.paint_about_page(c, &t, layout, about);
        }
        // Like a macOS toolbar, the hairline under the title shows once content scrolls
        // beneath it, and the footer's once content continues below.
        let first = layout.rows.first().map(|row| row.card);
        let last = layout.rows.last().map(|row| row.card);
        let hairline = 1. / c.scale;
        if first.is_some_and(|card| card.top < view.top - 0.5) {
            c.fill(
                side + hairline,
                view.top - hairline,
                w,
                view.top,
                0.,
                c.p.line,
                0.1,
            )?;
        }
        if last.is_some_and(|card| card.bottom > view.bottom + 0.5) {
            c.fill(
                side + hairline,
                view.bottom,
                w,
                view.bottom + hairline,
                0.,
                c.p.line,
                0.1,
            )?;
        }

        c.clipped(c.rect(view.left, view.top, view.right, view.bottom), || {
            for group in &layout.groups {
                let (l, top, r, bottom) = (group.left, group.top, group.right, group.bottom);
                c.fill(l, top, r, bottom, GROUP_RADIUS, c.p.line, t.group)?;
                c.stroke(l, top, r, bottom, GROUP_RADIUS, c.p.line, t.group_rim)?;
            }
            for (index, row) in layout.rows.iter().enumerate() {
                if row.card.bottom > view.top && row.card.top < view.bottom {
                    self.paint_row(c, &t, layout, index, row)?;
                    if !row.enabled {
                        let a = row.card;
                        c.fill(
                            a.left + 1.,
                            a.top + 1.,
                            a.right - 1.,
                            a.bottom - 1.,
                            GROUP_RADIUS,
                            mix(t.window, c.p.line, t.group),
                            0.48,
                        )?;
                    }
                }
            }
            Ok(())
        })?;

        if let Some(thumb) = layout.scroll_thumb {
            c.fill(
                thumb.left,
                thumb.top,
                thumb.right,
                thumb.bottom,
                2.,
                c.p.muted,
                0.45,
            )?;
        }

        // Footer with the page's reset button.
        let r = layout.footer_button;
        let note = match section {
            Section::Clock => Some("界面预览 · 仅本次运行有效"),
            Section::Monitor => {
                Some("CPU 核温取自已运行的 Core Temp，GPU 温度取决于驱动。无读数显示 —")
            }
            _ => None,
        };
        if let Some(text) = note {
            let note = Area {
                left: view.left + 2.,
                right: r.left - 8.,
                ..r
            };
            self.label(c, text, note, 11., false, c.p.muted, 1.)?;
        }
        if let Some(button) = layout.display_settings {
            self.button_face(c, &t, button, 6., hover(panel::DISPLAY_SETTINGS))?;
            if layout.focus == Some(layout.rows.len()) {
                c.stroke(
                    button.left,
                    button.top,
                    button.right,
                    button.bottom,
                    6.,
                    t.accent,
                    1.,
                )?;
            }
            self.label_aligned(
                c,
                "屏幕刷新率…",
                button,
                13.,
                false,
                c.p.ink,
                1.,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
        }
        self.button_face(c, &t, r, 6., hover(Action::Reset))?;
        if section == Section::Appearance && layout.focus == Some(layout.rows.len()) {
            c.stroke(r.left, r.top, r.right, r.bottom, 6., t.accent, 1.)?;
        }
        self.label_aligned(
            c,
            "恢复默认",
            r,
            13.,
            false,
            c.p.ink,
            1.,
            DWRITE_TEXT_ALIGNMENT_CENTER,
        )?;
        if let Some(modes) = &layout.modes {
            self.button_face(c, &t, modes.area, 6., false)?;
            for (index, (area, label)) in modes.rows().enumerate() {
                let active = index == modes.highlight;
                if active {
                    c.round(
                        area.left + 2.,
                        area.top + 1.,
                        area.right - 2.,
                        area.bottom - 1.,
                        4.,
                        t.accent,
                    )?;
                }
                let ink = if active { WHITE } else { c.p.ink };
                if index == modes.selected {
                    let check = Area {
                        right: area.left + 22.,
                        ..area
                    };
                    c.symbol_in(Symbol::Check, check, 12., ink, 1.)?;
                }
                self.label(
                    c,
                    label,
                    Area {
                        left: area.left + 24.,
                        ..area
                    },
                    12.,
                    false,
                    ink,
                    1.,
                )?;
            }
        }
        Ok(())
    }

    fn paint_about_page(
        &self,
        c: &Canvas,
        t: &Tones,
        layout: &PanelLayout,
        about: &panel::AboutPage,
    ) -> Result<()> {
        let h = about.heading;
        let mid = h.middle();
        let x = h.left + 26.;
        // The ring and island from the status bar mark.
        c.stroke(x - 16., mid - 16., x + 16., mid + 16., 16., t.accent, 1.)?;
        c.round(x - 5., mid - 5., x + 5., mid + 5., 5., t.accent)?;
        self.label(
            c,
            "DeskIsle",
            Area::new(h.left + 58., h.top, about.check_button.left - 8., mid + 3.),
            20.,
            true,
            c.p.ink,
            1.,
        )?;
        self.label(
            c,
            concat!("版本 ", env!("CARGO_PKG_VERSION")),
            Area::new(
                h.left + 58.,
                mid + 2.,
                about.check_button.left - 8.,
                h.bottom,
            ),
            13.,
            false,
            c.p.muted,
            1.,
        )?;

        let [first, last] = &about.links;
        let group = Area {
            bottom: last.row.bottom,
            ..first.row
        };
        c.fill(
            group.left,
            group.top,
            group.right,
            group.bottom,
            GROUP_RADIUS,
            c.p.line,
            t.group,
        )?;
        c.stroke(
            group.left,
            group.top,
            group.right,
            group.bottom,
            GROUP_RADIUS,
            c.p.line,
            t.group_rim,
        )?;
        c.fill(
            group.left + 16.,
            last.row.top,
            group.right - 16.,
            last.row.top + 1. / c.scale,
            0.,
            c.p.line,
            t.separator,
        )?;
        for link in &about.links {
            let label = Area {
                left: link.row.left + 16.,
                right: link.button.left - 8.,
                ..link.row
            };
            self.label(c, link.title, label, 13., false, c.p.ink, 1.)?;
        }
        for (index, (button, title, action)) in about.buttons().enumerate() {
            self.button_face(c, t, button, 6., layout.hover == Some(action))?;
            if layout.focus == Some(index) {
                c.stroke(
                    button.left,
                    button.top,
                    button.right,
                    button.bottom,
                    6.,
                    t.accent,
                    1.,
                )?;
            }
            self.label_aligned(
                c,
                title,
                button,
                13.,
                false,
                c.p.ink,
                1.,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
        }
        let q = layout.footer_button;
        let accent = if layout.hover == Some(Action::Quit) {
            mix(t.accent, BLACK, 0.1)
        } else {
            t.accent
        };
        c.round(q.left, q.top, q.right, q.bottom, 6., accent)?;
        if layout.focus == Some(about.buttons().count()) {
            c.stroke(
                q.left - 2.,
                q.top - 2.,
                q.right + 2.,
                q.bottom + 2.,
                8.,
                t.accent,
                1.,
            )?;
        }
        self.label_aligned(
            c,
            "退出程序",
            q,
            13.,
            false,
            WHITE,
            1.,
            DWRITE_TEXT_ALIGNMENT_CENTER,
        )
    }

    fn paint_row(
        &self,
        c: &Canvas,
        t: &Tones,
        layout: &PanelLayout,
        index: usize,
        row: &panel::Row,
    ) -> Result<()> {
        let a = row.card;
        if index > 0 && layout.rows[index - 1].card.bottom == a.top {
            // Inset separators between the grouped rows.
            let y = a.top;
            c.fill(
                a.left + 16.,
                y,
                a.right - 16.,
                y + 1. / c.scale,
                0.,
                c.p.line,
                t.separator,
            )?;
        }
        if layout.hover == Some(Action::Change(row.key, Adjust::Toggle)) {
            c.fill(
                a.left + 4.,
                a.top + 4.,
                a.right - 4.,
                a.bottom - 4.,
                7.,
                c.p.line,
                0.04,
            )?;
        }
        if layout.focus == Some(index) {
            // A macOS focus ring: the accent at full strength with a softer halo inside.
            c.stroke(
                a.left + 1.,
                a.top + 1.,
                a.right - 1.,
                a.bottom - 1.,
                GROUP_RADIUS - 1.,
                t.accent,
                0.9,
            )?;
            c.stroke(
                a.left + 2.,
                a.top + 2.,
                a.right - 2.,
                a.bottom - 2.,
                GROUP_RADIUS - 2.,
                t.accent,
                0.4,
            )?;
        }
        let text = panel::text_area(row);
        let stacked = matches!(row.part, Part::Slider { .. }) || panel::stacked_choice(row.key);
        let middle = if stacked { a.top + 25. } else { a.middle() };
        let label = Area {
            top: a.top + 8.,
            bottom: middle + 2.,
            ..text
        };
        let label = if row.key == Key::Accent {
            let icon = Area {
                right: label.left + 18.,
                ..label
            };
            c.symbol_in(Symbol::Palette, icon, 15., c.p.muted, 1.)?;
            Area {
                left: icon.right + 4.,
                ..label
            }
        } else {
            label
        };
        self.label(c, row.label, label, 13., false, c.p.ink, 1.)?;
        if let Some(detail) = &row.detail {
            let icon = Area::new(a.left + 12., middle - 21., text.left - 12., middle + 21.);
            self.paint_monitor_icon(c, row.key, icon)?;
            if let Some(area) = detail.label_area {
                let caption = Area {
                    left: area.left + 4.,
                    ..area
                };
                self.label(c, detail.label, caption, 11., false, c.p.muted, 1.)?;
            }
            match &detail.part {
                Part::Switch { area, on } => {
                    // A macOS checkbox.
                    let (l, tp, r, b) = (
                        area.left + 4.,
                        area.top + 4.,
                        area.right - 4.,
                        area.bottom - 4.,
                    );
                    if *on {
                        c.round(l, tp, r, b, 4., t.accent)?;
                        c.symbol_in(Symbol::Check, *area, 12., WHITE, 1.)?;
                    } else {
                        self.button_face(c, t, Area::new(l, tp, r, b), 4., false)?;
                    }
                }
                Part::Segments { options, selected } => {
                    for (i, (area, label)) in options.iter().enumerate() {
                        let selected = i == *selected;
                        let over =
                            layout.hover == Some(Action::Change(detail.key, Adjust::Pick(i)));
                        let (fill, alpha, ink) = if selected {
                            (t.accent, 1., WHITE)
                        } else {
                            (c.p.line, if over { 0.12 } else { 0.06 }, c.p.ink)
                        };
                        c.fill(
                            area.left,
                            area.top,
                            area.right,
                            area.bottom,
                            6.,
                            fill,
                            alpha,
                        )?;
                        self.label_aligned(
                            c,
                            label,
                            *area,
                            11.,
                            false,
                            ink,
                            1.,
                            DWRITE_TEXT_ALIGNMENT_CENTER,
                        )?;
                    }
                }
                _ => {}
            }
        } else {
            let help = Area {
                top: middle - 1.,
                bottom: a.bottom - 9.,
                ..text
            };
            let help = if stacked {
                Area {
                    bottom: a.top + 47.,
                    ..help
                }
            } else {
                help
            };
            self.label(c, row.help, help, 11., false, c.p.muted, 1.)?;
        }
        self.paint_panel_control(c, t, layout, row.key, &row.part)
    }

    fn paint_monitor_icon(&self, c: &Canvas, key: Key, icon: Area) -> Result<()> {
        let symbol = match key {
            Key::Monitor(MonitorKey::Temperature) => Symbol::Thermometer,
            Key::Monitor(MonitorKey::Memory) => Symbol::Memory,
            Key::Monitor(MonitorKey::Cpu) => Symbol::Chip,
            Key::Monitor(MonitorKey::Disk) => Symbol::Disk,
            _ => return Ok(()),
        };
        c.symbol_in(symbol, icon, 30., c.p.ink, 1.)
    }

    fn paint_panel_control(
        &self,
        c: &Canvas,
        t: &Tones,
        layout: &PanelLayout,
        key: Key,
        part: &Part,
    ) -> Result<()> {
        let hover = |adjust: Adjust| layout.hover == Some(Action::Change(key, adjust));
        match part {
            Part::Dropdown { area, selected } => {
                self.button_face(c, t, *area, 6., layout.hover == Some(Action::OpenModes))?;
                let text = Area {
                    left: area.left + 9.,
                    right: area.right - 21.,
                    ..*area
                };
                self.label(
                    c,
                    DOCK_DISPLAY_MODES[*selected].label,
                    text,
                    12.,
                    false,
                    c.p.ink,
                    1.,
                )?;
                let arrow = Area {
                    left: text.right,
                    right: area.right - 4.,
                    ..*area
                };
                c.symbol_in(Symbol::ChevronDown, arrow, 11., c.p.muted, 1.)?;
            }
            Part::Switch { area, on } => {
                // A macOS switch: a capsule track and a white knob with a soft shadow.
                let s = *area;
                let radius = (s.bottom - s.top) / 2.;
                let knob = radius - 2.;
                let x = if *on {
                    s.right - radius
                } else {
                    s.left + radius
                };
                if *on {
                    c.round(s.left, s.top, s.right, s.bottom, radius, t.accent)?;
                } else {
                    c.fill(s.left, s.top, s.right, s.bottom, radius, c.p.line, t.track)?;
                }
                let y = s.middle();
                c.fill(
                    x - knob - 0.5,
                    y - knob,
                    x + knob + 0.5,
                    y + knob + 1.5,
                    knob + 0.5,
                    BLACK,
                    0.18,
                )?;
                c.round(x - knob, y - knob, x + knob, y + knob, knob, WHITE)?;
            }
            Part::Slider {
                area,
                min,
                max,
                value,
                unit,
                ..
            } => {
                let (left, right) = panel::slider_track(*area);
                let y = area.middle();
                let x = left + (right - left) * (*value - *min) as f32 / (*max - *min) as f32;
                c.fill(left, y - 2., right, y + 2., 2., c.p.line, t.track)?;
                if x > left {
                    c.round(left, y - 2., x, y + 2., 2., t.accent)?;
                }
                self.button_face(c, t, Area::new(x - 5., y - 10., x + 5., y + 10.), 5., false)?;
                let readout_top = area.top
                    - if matches!(key, Key::Appearance(_)) {
                        30.
                    } else {
                        45.
                    };
                let readout =
                    Area::new(area.right - 76., readout_top, area.right, readout_top + 20.);
                self.label_aligned(
                    c,
                    &format!("{value} {unit}"),
                    readout,
                    12.,
                    false,
                    c.p.ink,
                    1.,
                    DWRITE_TEXT_ALIGNMENT_TRAILING,
                )?;
            }
            Part::Segments { options, selected } if key == Key::Accent => {
                for (i, (area, _)) in options.iter().enumerate() {
                    let (x, y) = ((area.left + area.right) / 2., area.middle());
                    let color = accent_color(i as i32, c.p.dark);
                    c.round(x - 9., y - 9., x + 9., y + 9., 9., color)?;
                    if i == *selected {
                        c.round(x - 3., y - 3., x + 3., y + 3., 3., WHITE)?;
                    } else if hover(Adjust::Pick(i)) {
                        c.stroke(x - 11., y - 11., x + 11., y + 11., 11., color, 0.65)?;
                    }
                }
            }
            Part::Segments { options, selected }
                if matches!(key, Key::Theme | Key::TopBarBackground) =>
            {
                for (i, (area, label)) in options.iter().enumerate() {
                    let x = (area.left + area.right) / 2.;
                    let top = area.top + 3.;
                    let (width, height) = if key == Key::TopBarBackground {
                        (area.right - area.left - 8., 56.)
                    } else {
                        (((area.right - area.left) * 0.70).min(66.), 34.)
                    };
                    let preview = Area::new(x - width / 2., top, x + width / 2., top + height);
                    if key == Key::TopBarBackground {
                        top_bar_preview(c, preview, TopBarBackground::from_index(i as i32))?;
                    } else {
                        // System and scheduled modes show both appearances.
                        let light = (216, 221, 228);
                        let dark = (55, 58, 65);
                        let base = if i == 2 { dark } else { light };
                        c.round(preview.left, top, preview.right, preview.bottom, 5., base)?;
                        if i == 0 || i == 3 {
                            c.clipped(c.rect(x, top, preview.right, preview.bottom), || {
                                c.round(preview.left, top, preview.right, preview.bottom, 5., dark)
                            })?;
                        }
                        let icon = match i {
                            0 => Symbol::Computer,
                            1 => Symbol::Sun,
                            2 => Symbol::Moon,
                            _ => Symbol::Clock,
                        };
                        let tile =
                            Area::new(preview.left + 3., top + 2., preview.left + 23., top + 22.);
                        c.symbol_in(icon, tile, 13., if i == 2 { WHITE } else { dark }, 0.9)?;
                        c.fill(
                            x - width * 0.28,
                            top + 24.,
                            x + width * 0.28,
                            top + 30.,
                            3.,
                            WHITE,
                            0.65,
                        )?;
                    }
                    let selected = i == *selected;
                    let rim = if selected { t.accent } else { c.p.line };
                    if key == Key::TopBarBackground && selected {
                        c.stroke_dip(
                            preview.left - 1.5,
                            top - 1.5,
                            preview.right + 1.5,
                            preview.bottom + 1.5,
                            8.5,
                            2.,
                            rim,
                            1.,
                        )?;
                    } else {
                        c.stroke(
                            preview.left - 1.,
                            top - 1.,
                            preview.right + 1.,
                            preview.bottom + 1.,
                            if key == Key::TopBarBackground { 8. } else { 6. },
                            rim,
                            if selected {
                                1.
                            } else if hover(Adjust::Pick(i)) {
                                0.5
                            } else {
                                0.18
                            },
                        )?;
                    }
                    let caption = Area {
                        top: preview.bottom + 3.,
                        ..*area
                    };
                    self.label_aligned(
                        c,
                        label,
                        caption,
                        12.,
                        selected,
                        c.p.ink,
                        1.,
                        DWRITE_TEXT_ALIGNMENT_CENTER,
                    )?;
                }
            }
            Part::Segments { options, selected } => {
                // A macOS segmented control: a recessed track with the chosen segment
                // raised on it, and hairlines between the unselected ones.
                if let (Some(first), Some(last)) = (options.first(), options.last()) {
                    let (l, tp, r, b) = (first.0.left, first.0.top, last.0.right, last.0.bottom);
                    c.fill(l, tp, r, b, 7., c.p.line, t.track * 0.7)?;
                }
                for (i, (s, _)) in options.iter().enumerate().skip(1) {
                    if i != *selected && i - 1 != *selected {
                        c.fill(
                            s.left,
                            s.top + 7.,
                            s.left + 1. / c.scale,
                            s.bottom - 7.,
                            0.,
                            c.p.line,
                            0.14,
                        )?;
                    }
                }
                for (i, (s, label)) in options.iter().enumerate() {
                    let inner = Area::new(s.left + 2., s.top + 2., s.right - 2., s.bottom - 2.);
                    if i == *selected {
                        self.button_face(c, t, inner, 5., false)?;
                    } else if hover(Adjust::Pick(i)) {
                        c.fill(
                            inner.left,
                            inner.top,
                            inner.right,
                            inner.bottom,
                            5.,
                            c.p.line,
                            0.06,
                        )?;
                    }
                    self.label_aligned(
                        c,
                        label,
                        *s,
                        12.,
                        false,
                        c.p.ink,
                        if i == *selected { 1. } else { 0.85 },
                        DWRITE_TEXT_ALIGNMENT_CENTER,
                    )?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod locale_tests {
    use super::panel_caption;

    #[test]
    fn panel_script_changes_while_language_names_stay_recognizable() {
        assert_eq!(panel_caption("常规", true).unwrap(), "常規");
        assert_eq!(panel_caption("常规", false).unwrap(), "常规");
        assert_eq!(panel_caption("简体中文", true).unwrap(), "简体中文");
        assert_eq!(panel_caption("繁體中文", false).unwrap(), "繁體中文");
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::geometry::Rect;
    use crate::settings::Settings;
    use crate::ui::panel::PanelState;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
    };
    use windows::core::w;

    struct HiddenWindow(HWND);

    impl HiddenWindow {
        fn create() -> Result<Self> {
            // The built-in STATIC class needs no shared test class registration.
            unsafe {
                CreateWindowExW(
                    WS_EX_NOREDIRECTIONBITMAP,
                    w!("STATIC"),
                    None,
                    WS_POPUP,
                    0,
                    0,
                    900,
                    700,
                    None,
                    None,
                    None,
                    None,
                )
                .map(Self)
            }
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    #[ignore = "requires a Windows desktop compositor"]
    fn settings_pages_commit_in_both_appearances() -> Result<()> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        let result = paint_pages();
        // All windows and graphics objects were owned by this test thread.
        unsafe { CoUninitialize() };
        result
    }

    fn paint_pages() -> Result<()> {
        let top = HiddenWindow::create()?;
        let bottom = HiddenWindow::create()?;
        let panel = HiddenWindow::create()?;
        let work = Rect {
            x: 0,
            y: 0,
            width: 900,
            height: 700,
        };
        let mut settings = Settings::default();
        settings.set(Key::FontScale, 120);
        let mut renderer = Renderer::new(top.0, bottom.0, false, &settings)?;
        for dark in [false, true] {
            renderer.set_appearance(dark, &settings);
            for section in [
                Section::General,
                Section::Appearance,
                Section::Dock,
                Section::TopBar,
                Section::About,
            ] {
                let mut state = PanelState {
                    page: SECTIONS.iter().position(|&s| s == section).unwrap(),
                    ..Default::default()
                };
                loop {
                    let layout = PanelLayout::new(&mut state, &settings, 120, work);
                    renderer.draw_panel(panel.0, &layout, 120, dark)?;
                    if state.scroll >= layout.max_scroll {
                        break;
                    }
                    state.scroll = (state.scroll + layout.view.bottom - layout.view.top)
                        .min(layout.max_scroll);
                }
                if section == Section::Dock {
                    state.mode_highlight = Some(settings.get(Key::DockDisplayMode) as usize);
                    let layout = PanelLayout::new(&mut state, &settings, 120, work);
                    renderer.draw_panel(panel.0, &layout, 120, dark)?;
                }
            }
        }
        unsafe { renderer.device.WaitForCommitCompletion()? };
        renderer.check_device()
    }
}
