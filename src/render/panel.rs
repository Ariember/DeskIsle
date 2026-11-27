use std::borrow::Cow;
use windows::Win32::{
    Foundation::HWND,
    Globalization::{LCMAP_TRADITIONAL_CHINESE, LCMapStringEx},
    Graphics::{
        Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP,
        DirectComposition::*,
        DirectWrite::{
            DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT,
            DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_WORD_WRAPPING_WRAP,
        },
    },
};
use windows::core::{Error, Result, w};

use crate::settings::{
    Adjust, DOCK_DISPLAY_MODES, Key, MonitorKey, SECTIONS, Section, TopBarBackground,
};
use crate::ui::icon::Symbol;
use crate::ui::panel::{
    self, Action, Area, InstalledHealth, MarketState, PanelLayout, Part, PluginFocus, PluginTab,
    PluginsPage,
};

use super::Renderer;
use super::canvas::{Canvas, Rgb, Text, accent_color, paint, text_format};

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
        Section::Plugins => (33, 131, 109),
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
        Section::Plugins => Symbol::Puzzle,
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
        let draw_wallpaper = |feathered: bool| -> Result<()> {
            c.gradient(l, t, r, b, 0., ((106, 144, 196), 1.), ((176, 204, 228), 1.))?;
            for (x, y, start, end) in [
                (0.30, 5., (156, 114, 232), (52, 68, 158)),
                (0.17, 15., (224, 134, 212), (132, 54, 134)),
                (0.02, 26., (255, 174, 146), (197, 79, 119)),
                (-0.18, 37., (255, 212, 138), (231, 114, 90)),
            ] {
                let band = |spread: f32, shift_y: f32, alpha| {
                    c.gradient(
                        l + w * x - spread,
                        t + y + shift_y - spread,
                        l + w * (x + 0.66) + spread,
                        b + 34. + shift_y + spread,
                        32. + spread,
                        (start, alpha),
                        (end, alpha),
                    )
                };
                if feathered {
                    for (spread, alpha) in [
                        (6., 0.10),
                        (4., 0.18),
                        (2., 0.28),
                        (0., 0.40),
                        (-2., 0.56),
                        (-4., 0.78),
                        (-6., 1.),
                    ] {
                        band(spread, 2.2, alpha)?;
                    }
                } else {
                    band(0., 0., 1.)?;
                }
            }
            Ok(())
        };
        draw_wallpaper(false)?;
        if style == TopBarBackground::Frosted {
            // Only the bar strip diffuses and refracts the wallpaper; the desktop below stays sharp.
            c.clipped(c.rect(l, t, r, t + bar_h), || draw_wallpaper(true))?;
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
    sidebar: Rgb,
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
                sidebar: (45, 44, 48),
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
                sidebar: (232, 230, 237),
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
    /// again after the device was rebuilt.
    pub fn draw_panel(
        &mut self,
        hwnd: HWND,
        layout: &PanelLayout,
        dpi: u32,
        translucent: bool,
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
            self.paint_panel(c, layout, translucent)
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

    /// Multi-line top-aligned text with word wrapping and character ellipsis trimming.
    fn label_wrapped(
        &self,
        c: &Canvas,
        text: &str,
        a: Area,
        size: f32,
        ink: Rgb,
        alpha: f32,
    ) -> Result<()> {
        let rect = c.rect(a.left, a.top, a.right, a.bottom);
        if rect.right <= rect.left || rect.bottom <= rect.top || text.is_empty() {
            return Ok(());
        }
        let caption = panel_caption(text, self.traditional_panel)?;
        let format = text_format(
            &self.write,
            c.p.typography,
            Text {
                size,
                ..Text::default()
            },
            c.scale,
        )?;
        unsafe {
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            c.ctx.DrawText(
                &caption.encode_utf16().collect::<Vec<_>>(),
                &format,
                &rect,
                &c.brush_alpha(ink, alpha)?,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
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

    /// Primary call-to-action button filled with the user's accent color.
    fn accent_button(&self, c: &Canvas, t: &Tones, a: Area, radius: f32, lit: bool) -> Result<()> {
        c.fill(
            a.left,
            a.top + 0.5,
            a.right,
            a.bottom + 1.,
            radius,
            BLACK,
            t.face_shadow * 0.85,
        )?;
        let base = if lit {
            mix(t.accent, WHITE, 0.08)
        } else {
            t.accent
        };
        c.gradient(
            a.left,
            a.top,
            a.right,
            a.bottom,
            radius,
            (mix(base, WHITE, 0.14), 1.),
            (base, 1.),
        )?;
        c.stroke(a.left, a.top, a.right, a.bottom, radius, WHITE, 0.18)
    }

    /// Draws `title` followed by a compact rounded status/version pill when `badge` is given.
    #[allow(clippy::too_many_arguments)]
    fn title_with_pill(
        &self,
        c: &Canvas,
        title: &str,
        area: Area,
        badge: Option<(&str, Rgb, Rgb, f32)>,
    ) -> Result<()> {
        let Some((badge_text, fill, ink, fill_alpha)) = badge else {
            return self.label(c, title, area, 13., true, c.p.ink, 1.);
        };
        let badge_caption = panel_caption(badge_text, self.traditional_panel)?;
        let pill_w = (self.measure(&badge_caption, 10.5, false) + 12.).clamp(34., 118.);
        let max_title_right = (area.right - pill_w - 7.).max(area.left + 48.);
        let title_caption = panel_caption(title, self.traditional_panel)?;
        let measured_title = self.measure(&title_caption, 13., true);
        let title_right = (area.left + measured_title + 2.).min(max_title_right);
        self.label(
            c,
            title,
            Area {
                right: title_right,
                ..area
            },
            13.,
            true,
            c.p.ink,
            1.,
        )?;
        let pill_left = title_right + 6.;
        if pill_left + 28. <= area.right {
            let mid = area.middle();
            let pill = Area::new(
                pill_left,
                mid - 8.5,
                (pill_left + pill_w).min(area.right),
                mid + 8.5,
            );
            c.fill(
                pill.left,
                pill.top,
                pill.right,
                pill.bottom,
                8.5,
                fill,
                fill_alpha,
            )?;
            c.stroke(
                pill.left,
                pill.top,
                pill.right,
                pill.bottom,
                8.5,
                fill,
                (fill_alpha * 1.6).min(0.45),
            )?;
            self.label_aligned(
                c,
                badge_text,
                pill,
                10.5,
                false,
                ink,
                1.,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
        }
        Ok(())
    }

    fn paint_panel(&self, c: &Canvas, layout: &PanelLayout, translucent: bool) -> Result<()> {
        let t = Tones::new(c.p.dark, c.p.accent);
        let (w, h) = (layout.width, layout.height);
        let hover = |action: Action| layout.hover == Some(action);
        let side = panel::SIDEBAR;
        c.round(side, 0., w, h, 0., t.window)?;

        // The sidebar runs the full height, under the window lights, as in macOS.
        let opacity = if translucent { 0.8 } else { 1. };
        c.fill(0., 0., side, h, 0., t.sidebar, opacity)?;
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
            let ink = if selected { c.p.on_accent } else { c.p.ink };
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
        let hairline = 1. / c.scale;
        if layout.scrolled_top {
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
        if layout.scrolled_bottom {
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
            if let Some(plugins) = &layout.plugins {
                self.paint_plugins_page(c, &t, layout, plugins)?;
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
        if let Some((button, label, action)) = layout.footer_link {
            self.button_face(c, &t, button, 6., hover(action))?;
            if layout.focus == Some(layout.footer_focus()) {
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
                label,
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

    fn paint_plugins_page(
        &self,
        c: &Canvas,
        t: &Tones,
        layout: &PanelLayout,
        plugins: &PluginsPage,
    ) -> Result<()> {
        let view = layout.view;
        let hover = |action: Action| layout.hover == Some(action);
        let focus_ring = layout.focus_ring();
        let teal: Rgb = section_color(Section::Plugins);
        let amber: Rgb = if c.p.dark {
            (238, 168, 58)
        } else {
            (196, 116, 14)
        };
        let badge_alpha = if c.p.dark { 0.20 } else { 0.12 };

        // Toolbar: segmented filter on the left and refresh/browse button on the right.
        if plugins.tabs_track.bottom > view.top && plugins.tabs_track.top < view.bottom {
            let tr = plugins.tabs_track;
            c.fill(
                tr.left,
                tr.top,
                tr.right,
                tr.bottom,
                7.,
                c.p.line,
                t.track * 0.75,
            )?;
            let selected_idx = plugins.tab.index();
            for (i, &(seg, _)) in plugins.tabs.iter().enumerate().skip(1) {
                if i != selected_idx && i - 1 != selected_idx {
                    c.fill(
                        seg.left,
                        seg.top + 7.,
                        seg.left + 1. / c.scale,
                        seg.bottom - 7.,
                        0.,
                        c.p.line,
                        0.14,
                    )?;
                }
            }
            let installed_attention =
                plugins.view.installed.iter().any(|item| {
                    item.health == InstalledHealth::Error || item.market_update.is_some()
                });
            let installed_error = plugins
                .view
                .installed
                .iter()
                .any(|item| item.health == InstalledHealth::Error);
            for &(seg, tab) in &plugins.tabs {
                let inner = Area::new(seg.left + 2., seg.top + 2., seg.right - 2., seg.bottom - 2.);
                let active = tab == plugins.tab;
                if active {
                    self.button_face(c, t, inner, 5.5, false)?;
                } else if hover(Action::PluginTab(tab)) {
                    c.fill(
                        inner.left,
                        inner.top,
                        inner.right,
                        inner.bottom,
                        5.5,
                        c.p.line,
                        0.06,
                    )?;
                }
                let text = match tab {
                    PluginTab::All => Cow::Borrowed("全部"),
                    PluginTab::Market => {
                        if plugins.view.loaded && !plugins.view.items.is_empty() {
                            Cow::Owned(format!("市场 {}", plugins.view.items.len()))
                        } else {
                            Cow::Borrowed("插件市场")
                        }
                    }
                    PluginTab::Installed => {
                        if !plugins.view.installed.is_empty() {
                            Cow::Owned(format!("已安装 {}", plugins.view.installed.len()))
                        } else {
                            Cow::Borrowed("已安装")
                        }
                    }
                };
                self.label_aligned(
                    c,
                    &text,
                    seg,
                    12.,
                    active,
                    c.p.ink,
                    if active { 1. } else { 0.82 },
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                )?;
                if tab == PluginTab::Installed && !active && installed_attention {
                    let dot = if installed_error {
                        c.p.danger
                    } else {
                        t.accent
                    };
                    c.round(
                        seg.right - 10.,
                        seg.top + 5.,
                        seg.right - 5.,
                        seg.top + 10.,
                        2.5,
                        dot,
                    )?;
                }
            }
            if matches!(focus_ring, Some((PluginFocus::Tabs, _))) {
                c.stroke(
                    tr.left - 1.,
                    tr.top - 1.,
                    tr.right + 1.,
                    tr.bottom + 1.,
                    8.,
                    t.accent,
                    0.95,
                )?;
            }

            let rb = plugins.refresh_button;
            let rb_lit = plugins.refresh_action.is_some_and(hover);
            self.button_face(c, t, rb, 6., rb_lit)?;
            if matches!(focus_ring, Some((PluginFocus::Refresh, _))) {
                c.stroke(rb.left, rb.top, rb.right, rb.bottom, 6., t.accent, 1.)?;
            }
            let rb_ink = if plugins.refresh_action.is_some() {
                c.p.ink
            } else {
                c.p.muted
            };
            let icon = if !plugins.view.loaded && plugins.view.error.is_none() {
                Symbol::Store
            } else {
                Symbol::Restart
            };
            c.symbol(icon, rb.left + 16., rb.middle(), 12.5, rb_ink, 0.9)?;
            self.label_aligned(
                c,
                plugins.refresh_label,
                Area {
                    left: rb.left + 25.,
                    right: rb.right - 6.,
                    ..rb
                },
                12.,
                false,
                rb_ink,
                1.,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
        }

        // Installed plugins section.
        if let Some(installed) = &plugins.installed {
            if installed.header.bottom > view.top && installed.header.top < view.bottom {
                self.label(
                    c,
                    "已安装",
                    Area {
                        right: installed.header.left + 120.,
                        ..installed.header
                    },
                    11.5,
                    true,
                    c.p.muted,
                    1.,
                )?;
                self.label_aligned(
                    c,
                    &installed.meta,
                    Area {
                        left: installed.header.left + 120.,
                        ..installed.header
                    },
                    11.,
                    installed.meta_accent,
                    if installed.meta_accent {
                        t.accent
                    } else {
                        c.p.muted
                    },
                    1.,
                    DWRITE_TEXT_ALIGNMENT_TRAILING,
                )?;
            }
            let g = installed.group;
            if g.bottom > view.top && g.top < view.bottom {
                c.fill(
                    g.left,
                    g.top,
                    g.right,
                    g.bottom,
                    GROUP_RADIUS,
                    c.p.line,
                    t.group,
                )?;
                c.stroke(
                    g.left,
                    g.top,
                    g.right,
                    g.bottom,
                    GROUP_RADIUS,
                    c.p.line,
                    t.group_rim,
                )?;
                if let Some(btn) = installed.empty_button {
                    let mid_x = (g.left + g.right) / 2.;
                    self.label_aligned(
                        c,
                        "尚未安装任何插件",
                        Area::new(g.left + 16., g.top + 8., g.right - 16., g.top + 28.),
                        13.,
                        true,
                        c.p.ink,
                        1.,
                        DWRITE_TEXT_ALIGNMENT_CENTER,
                    )?;
                    self.label_aligned(
                        c,
                        "可从插件市场一键安装桌面卡片，或通过命令行发布本地插件",
                        Area::new(g.left + 16., g.top + 28., g.right - 16., g.top + 47.),
                        11.5,
                        false,
                        c.p.muted,
                        1.,
                        DWRITE_TEXT_ALIGNMENT_CENTER,
                    )?;
                    let action = Action::PluginTab(PluginTab::Market);
                    self.accent_button(c, t, btn, 6., hover(action))?;
                    if matches!(focus_ring, Some((PluginFocus::EmptyBrowseMarket, _))) {
                        c.stroke(
                            btn.left - 2.,
                            btn.top - 2.,
                            btn.right + 2.,
                            btn.bottom + 2.,
                            8.,
                            t.accent,
                            1.,
                        )?;
                    }
                    self.label_aligned(
                        c,
                        "浏览插件市场",
                        Area::new(mid_x - 56., btn.top, mid_x + 56., btn.bottom),
                        12.5,
                        true,
                        c.p.on_accent,
                        1.,
                        DWRITE_TEXT_ALIGNMENT_CENTER,
                    )?;
                } else {
                    for row in &installed.rows {
                        let a = row.area;
                        if a.bottom <= view.top || a.top >= view.bottom {
                            continue;
                        }
                        if row.index > 0 {
                            c.fill(
                                a.left + 60.,
                                a.top,
                                a.right - 16.,
                                a.top + 1. / c.scale,
                                0.,
                                c.p.line,
                                t.separator,
                            )?;
                        }
                        let item = &plugins.view.installed[row.index];
                        let mid = a.middle();
                        let tile = Area::new(a.left + 14., mid - 18., a.left + 50., mid + 18.);
                        let (icon, tone) = match item.health {
                            InstalledHealth::Ready => (Symbol::Puzzle, teal),
                            InstalledHealth::Setup => (Symbol::Sliders, amber),
                            InstalledHealth::Error => (Symbol::Info, c.p.danger),
                        };
                        c.fill(
                            tile.left,
                            tile.top,
                            tile.right,
                            tile.bottom,
                            9.,
                            tone,
                            badge_alpha,
                        )?;
                        c.stroke(
                            tile.left,
                            tile.top,
                            tile.right,
                            tile.bottom,
                            9.,
                            tone,
                            badge_alpha * 1.4,
                        )?;
                        c.symbol_in(icon, tile, 18., tone, 1.)?;

                        let controls_left = row
                            .update_button
                            .map(|(b, _)| b.left)
                            .or(row.remove_button.map(|b| b.left))
                            .unwrap_or(a.right - 16.)
                            - 10.;
                        let title_area =
                            Area::new(a.left + 60., a.top + 10., controls_left, mid + 1.);
                        let pill_owned: Option<(String, Rgb, Rgb, f32)> =
                            if let Some((_, next_v)) = &item.market_update {
                                let cur_v = item.version.as_deref().unwrap_or("?");
                                Some((
                                    format!("v{cur_v} → v{next_v}"),
                                    t.accent,
                                    t.accent,
                                    badge_alpha,
                                ))
                            } else if let Some(v) = &item.version {
                                Some((format!("v{v}"), c.p.line, c.p.muted, 0.06))
                            } else if item.health == InstalledHealth::Error {
                                Some(("异常".into(), c.p.danger, c.p.danger, badge_alpha))
                            } else {
                                None
                            };
                        self.title_with_pill(
                            c,
                            &item.title,
                            title_area,
                            pill_owned
                                .as_ref()
                                .map(|(s, fill, ink, al)| (s.as_str(), *fill, *ink, *al)),
                        )?;
                        self.label(
                            c,
                            &item.summary,
                            Area::new(a.left + 60., mid, controls_left, a.bottom - 9.),
                            11.5,
                            false,
                            if item.health == InstalledHealth::Error {
                                c.p.danger
                            } else {
                                c.p.muted
                            },
                            1.,
                        )?;

                        if let Some((btn, market_idx)) = row.update_button {
                            let busy_updating =
                                plugins.view.busy == Some(Action::InstallPlugin(market_idx));
                            let can_update = plugins.view.busy.is_none();
                            if can_update {
                                self.accent_button(
                                    c,
                                    t,
                                    btn,
                                    6.,
                                    hover(Action::InstallPlugin(market_idx)),
                                )?;
                            } else {
                                self.button_face(c, t, btn, 6., false)?;
                            }
                            if focus_ring == Some((PluginFocus::InstalledUpdate(market_idx), btn)) {
                                c.stroke(
                                    btn.left - 2.,
                                    btn.top - 2.,
                                    btn.right + 2.,
                                    btn.bottom + 2.,
                                    8.,
                                    t.accent,
                                    1.,
                                )?;
                            }
                            self.label_aligned(
                                c,
                                if busy_updating {
                                    "更新中…"
                                } else {
                                    "更新"
                                },
                                btn,
                                12.5,
                                can_update,
                                if can_update { c.p.on_accent } else { c.p.muted },
                                1.,
                                DWRITE_TEXT_ALIGNMENT_CENTER,
                            )?;
                        }

                        if let Some(btn) = row.remove_button {
                            let busy_removing =
                                plugins.view.busy == Some(Action::RemovePlugin(row.index));
                            let over = row.remove_action.is_some_and(hover);
                            self.button_face(c, t, btn, 6., false)?;
                            if over {
                                c.fill(
                                    btn.left,
                                    btn.top,
                                    btn.right,
                                    btn.bottom,
                                    6.,
                                    c.p.danger,
                                    if c.p.dark { 0.20 } else { 0.11 },
                                )?;
                                c.stroke(
                                    btn.left, btn.top, btn.right, btn.bottom, 6., c.p.danger, 0.45,
                                )?;
                            }
                            if focus_ring == Some((PluginFocus::InstalledRemove(row.index), btn)) {
                                c.stroke(
                                    btn.left, btn.top, btn.right, btn.bottom, 6., t.accent, 1.,
                                )?;
                            }
                            let ink = if over {
                                c.p.danger
                            } else if row.remove_action.is_some() {
                                c.p.ink
                            } else {
                                c.p.muted
                            };
                            self.label_aligned(
                                c,
                                if busy_removing {
                                    "移除中…"
                                } else {
                                    "移除"
                                },
                                btn,
                                12.5,
                                false,
                                ink,
                                1.,
                                DWRITE_TEXT_ALIGNMENT_CENTER,
                            )?;
                        }
                    }
                }
            }
        }

        // Plugin market section.
        if let Some(market) = &plugins.market {
            if market.header.bottom > view.top && market.header.top < view.bottom {
                self.label(
                    c,
                    "插件市场",
                    Area {
                        right: market.header.left + 110.,
                        ..market.header
                    },
                    11.5,
                    true,
                    c.p.muted,
                    1.,
                )?;
                self.label_aligned(
                    c,
                    &market.source_label,
                    Area {
                        left: market.header.left + 110.,
                        ..market.header
                    },
                    11.,
                    false,
                    c.p.muted,
                    1.,
                    DWRITE_TEXT_ALIGNMENT_TRAILING,
                )?;
            }
            if let Some((banner, retry)) = market.error_banner
                && banner.bottom > view.top
                && banner.top < view.bottom
            {
                c.fill(
                    banner.left,
                    banner.top,
                    banner.right,
                    banner.bottom,
                    8.,
                    c.p.danger,
                    if c.p.dark { 0.14 } else { 0.08 },
                )?;
                c.stroke(
                    banner.left,
                    banner.top,
                    banner.right,
                    banner.bottom,
                    8.,
                    c.p.danger,
                    0.30,
                )?;
                c.symbol(
                    Symbol::Info,
                    banner.left + 20.,
                    banner.middle(),
                    15.,
                    c.p.danger,
                    1.,
                )?;
                let msg = plugins.view.error.as_deref().unwrap_or("同步失败");
                self.label(
                    c,
                    &format!("刷新失败：{msg}"),
                    Area::new(
                        banner.left + 34.,
                        banner.top,
                        retry.left - 8.,
                        banner.bottom,
                    ),
                    11.5,
                    false,
                    c.p.danger,
                    1.,
                )?;
                self.button_face(c, t, retry, 5.5, plugins.refresh_action.is_some_and(hover))?;
                self.label_aligned(
                    c,
                    "重试",
                    retry,
                    12.,
                    false,
                    if plugins.refresh_action.is_some() {
                        c.p.ink
                    } else {
                        c.p.muted
                    },
                    1.,
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                )?;
            }

            let g = market.group;
            if g.bottom > view.top && g.top < view.bottom {
                let is_empty_error = plugins.view.items.is_empty() && plugins.view.error.is_some();
                c.fill(
                    g.left,
                    g.top,
                    g.right,
                    g.bottom,
                    GROUP_RADIUS,
                    if is_empty_error { c.p.danger } else { c.p.line },
                    if is_empty_error {
                        if c.p.dark { 0.10 } else { 0.05 }
                    } else {
                        t.group
                    },
                )?;
                c.stroke(
                    g.left,
                    g.top,
                    g.right,
                    g.bottom,
                    GROUP_RADIUS,
                    if is_empty_error { c.p.danger } else { c.p.line },
                    if is_empty_error { 0.25 } else { t.group_rim },
                )?;

                if plugins.view.items.is_empty() {
                    let mid = g.middle();
                    let tile = Area::new(g.left + 16., mid - 20., g.left + 56., mid + 20.);
                    let (icon, tone) = if is_empty_error {
                        (Symbol::Info, c.p.danger)
                    } else {
                        (Symbol::Store, teal)
                    };
                    c.fill(
                        tile.left,
                        tile.top,
                        tile.right,
                        tile.bottom,
                        10.,
                        tone,
                        badge_alpha,
                    )?;
                    c.stroke(
                        tile.left,
                        tile.top,
                        tile.right,
                        tile.bottom,
                        10.,
                        tone,
                        badge_alpha * 1.4,
                    )?;
                    c.symbol_in(icon, tile, 20., tone, 1.)?;

                    let text_right = market
                        .state_button
                        .map_or(g.right - 16., |(btn, _)| btn.left - 12.);
                    let (heading, detail): (&str, &str) =
                        if plugins.view.busy == Some(Action::BrowseMarket) {
                            ("正在连接插件市场…", "正在读取桌面卡片索引")
                        } else if let Some(err) = &plugins.view.error {
                            ("无法连接插件市场", err.as_str())
                        } else if !plugins.view.loaded {
                            (
                                "探索桌面卡片插件",
                                "浏览官方市场提供的桌面磁贴卡片，一键下载并自动核验完整性",
                            )
                        } else {
                            (
                                "市场中暂无可用插件",
                                "当前索引没有可安装的卡片包，稍后可点击右上角刷新重试",
                            )
                        };
                    self.label(
                        c,
                        heading,
                        Area::new(g.left + 68., g.top + 12., text_right, g.top + 34.),
                        13.,
                        true,
                        c.p.ink,
                        1.,
                    )?;
                    self.label_wrapped(
                        c,
                        detail,
                        Area::new(g.left + 68., g.top + 35., text_right, g.bottom - 10.),
                        11.5,
                        if is_empty_error {
                            c.p.danger
                        } else {
                            c.p.muted
                        },
                        1.,
                    )?;
                    if let Some((btn, label)) = market.state_button {
                        let over = plugins.refresh_action.is_some_and(hover);
                        if !is_empty_error {
                            self.accent_button(c, t, btn, 6., over)?;
                            self.label_aligned(
                                c,
                                label,
                                btn,
                                12.5,
                                true,
                                c.p.on_accent,
                                1.,
                                DWRITE_TEXT_ALIGNMENT_CENTER,
                            )?;
                        } else {
                            self.button_face(c, t, btn, 6., over)?;
                            self.label_aligned(
                                c,
                                label,
                                btn,
                                12.5,
                                false,
                                c.p.ink,
                                1.,
                                DWRITE_TEXT_ALIGNMENT_CENTER,
                            )?;
                        }
                    }
                } else {
                    for index in market.visible(view) {
                        let row = market.row(index);
                        let button = market.button(index);
                        let item = &plugins.view.items[index];
                        let action = market.action(index);
                        if index > 0 {
                            c.fill(
                                row.left + 64.,
                                row.top,
                                row.right - 16.,
                                row.top + 1. / c.scale,
                                0.,
                                c.p.line,
                                t.separator,
                            )?;
                        }
                        let tile =
                            Area::new(row.left + 14., row.top + 15., row.left + 52., row.top + 53.);
                        let (icon, tone) = match item.state {
                            MarketState::Installed => (Symbol::Cards, teal),
                            MarketState::Update => (Symbol::Cards, t.accent),
                            MarketState::Reinstall => (Symbol::Restart, amber),
                            MarketState::Install => (Symbol::Cards, t.accent),
                        };
                        c.fill(
                            tile.left,
                            tile.top,
                            tile.right,
                            tile.bottom,
                            9.5,
                            tone,
                            badge_alpha,
                        )?;
                        c.stroke(
                            tile.left,
                            tile.top,
                            tile.right,
                            tile.bottom,
                            9.5,
                            tone,
                            badge_alpha * 1.35,
                        )?;
                        c.symbol_in(icon, tile, 19., tone, 1.)?;

                        let text_right = button.left - 12.;
                        let pill: (String, Rgb, Rgb, f32) = match item.state {
                            MarketState::Update => (
                                format!(
                                    "v{} → v{}",
                                    item.installed_version.as_deref().unwrap_or("?"),
                                    item.version
                                ),
                                t.accent,
                                t.accent,
                                badge_alpha,
                            ),
                            MarketState::Reinstall => (
                                format!("v{} · 已改动", item.version),
                                amber,
                                amber,
                                badge_alpha,
                            ),
                            _ => (format!("v{}", item.version), c.p.line, c.p.muted, 0.06),
                        };
                        self.title_with_pill(
                            c,
                            &item.name,
                            Area::new(row.left + 64., row.top + 10., text_right, row.top + 32.),
                            Some((pill.0.as_str(), pill.1, pill.2, pill.3)),
                        )?;
                        self.label_wrapped(
                            c,
                            &item.description,
                            Area::new(row.left + 64., row.top + 34., text_right, row.bottom - 9.),
                            11.5,
                            c.p.muted,
                            1.,
                        )?;

                        let label = market.button_label(index);
                        let is_busy_item = plugins.view.busy == Some(Action::InstallPlugin(index));
                        if is_busy_item {
                            self.button_face(c, t, button, 6., false)?;
                            self.label_aligned(
                                c,
                                label,
                                button,
                                12.5,
                                false,
                                c.p.muted,
                                1.,
                                DWRITE_TEXT_ALIGNMENT_CENTER,
                            )?;
                        } else {
                            match item.state {
                                MarketState::Installed => {
                                    c.fill(
                                        button.left,
                                        button.top,
                                        button.right,
                                        button.bottom,
                                        6.,
                                        teal,
                                        if c.p.dark { 0.16 } else { 0.10 },
                                    )?;
                                    c.stroke(
                                        button.left,
                                        button.top,
                                        button.right,
                                        button.bottom,
                                        6.,
                                        teal,
                                        0.26,
                                    )?;
                                    c.symbol(
                                        Symbol::Check,
                                        button.left + 15.,
                                        button.middle(),
                                        11.5,
                                        teal,
                                        1.,
                                    )?;
                                    self.label_aligned(
                                        c,
                                        label,
                                        Area {
                                            left: button.left + 22.,
                                            right: button.right - 4.,
                                            ..button
                                        },
                                        12.,
                                        false,
                                        teal,
                                        1.,
                                        DWRITE_TEXT_ALIGNMENT_CENTER,
                                    )?;
                                }
                                MarketState::Install | MarketState::Update => {
                                    let can_act = action.is_some();
                                    if can_act {
                                        self.accent_button(
                                            c,
                                            t,
                                            button,
                                            6.,
                                            action.is_some_and(hover),
                                        )?;
                                    } else {
                                        self.button_face(c, t, button, 6., false)?;
                                    }
                                    self.label_aligned(
                                        c,
                                        label,
                                        button,
                                        12.5,
                                        can_act,
                                        if can_act { c.p.on_accent } else { c.p.muted },
                                        1.,
                                        DWRITE_TEXT_ALIGNMENT_CENTER,
                                    )?;
                                }
                                MarketState::Reinstall => {
                                    self.button_face(c, t, button, 6., action.is_some_and(hover))?;
                                    self.label_aligned(
                                        c,
                                        label,
                                        button,
                                        12.5,
                                        false,
                                        if action.is_some() { c.p.ink } else { c.p.muted },
                                        1.,
                                        DWRITE_TEXT_ALIGNMENT_CENTER,
                                    )?;
                                }
                            }
                        }
                        if focus_ring == Some((PluginFocus::MarketItem(index), button)) {
                            c.stroke(
                                button.left - 2.,
                                button.top - 2.,
                                button.right + 2.,
                                button.bottom + 2.,
                                8.,
                                t.accent,
                                1.,
                            )?;
                        }
                    }
                }
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
        // Use the proportions and colors of the project's app icon.
        let a = about.icon;
        let size = a.right - a.left;
        let x = (a.left + a.right) / 2.;
        let y = a.middle();
        c.shadow(a, size * 0.23, 3., 0.10)?;
        c.gradient_stops(
            a.left,
            a.top,
            a.right,
            a.bottom,
            size * 0.23,
            &[
                (0., ((91, 140, 255), 1.)),
                (0.55, ((122, 92, 250), 1.)),
                (1., ((34, 193, 195), 1.)),
            ],
        )?;
        c.ring(x, y, size * 0.25, size * 0.075, WHITE)?;
        let dot = size * 0.092;
        c.round(x - dot, y - dot, x + dot, y + dot, dot, WHITE)?;

        let align = if about.compact {
            DWRITE_TEXT_ALIGNMENT_LEADING
        } else {
            DWRITE_TEXT_ALIGNMENT_CENTER
        };
        self.label_aligned(
            c,
            "DeskIsle",
            about.name,
            if about.compact { 20. } else { 28. },
            true,
            c.p.ink,
            1.,
            align,
        )?;
        self.label_aligned(
            c,
            concat!("版本 ", env!("CARGO_PKG_VERSION")),
            about.version,
            13.,
            false,
            c.p.muted,
            1.,
            align,
        )?;

        let check = about.check_button;
        self.accent_button(c, t, check, 8., layout.hover == Some(Action::CheckUpdates))?;
        self.label_aligned(
            c,
            about.check_label,
            check,
            13.,
            true,
            c.p.on_accent,
            1.,
            DWRITE_TEXT_ALIGNMENT_CENTER,
        )?;

        let [first, last] = &about.links;
        let (left, right) = (first.row.left, first.row.right);
        let (top, bottom) = (first.row.top, last.row.bottom);
        c.fill(left, top, right, bottom, GROUP_RADIUS, c.p.line, t.group)?;
        c.stroke(
            left,
            top,
            right,
            bottom,
            GROUP_RADIUS,
            c.p.line,
            t.group_rim,
        )?;
        c.fill(
            left + 16.,
            last.row.top,
            right - 16.,
            last.row.top + 1. / c.scale,
            0.,
            c.p.line,
            t.separator,
        )?;
        for link in &about.links {
            let row = link.row;
            if layout.hover == Some(link.action) {
                c.fill(
                    left + 2.,
                    row.top + 2.,
                    right - 2.,
                    row.bottom - 2.,
                    8.,
                    c.p.line,
                    0.06,
                )?;
            }
            self.label(
                c,
                link.title,
                Area {
                    left: left + 16.,
                    right: right - 40.,
                    ..row
                },
                13.,
                false,
                c.p.ink,
                1.,
            )?;
            c.symbol(Symbol::Open, right - 22., row.middle(), 15., c.p.muted, 1.)?;
        }

        let quit = layout.footer_button;
        self.button_face(c, t, quit, 6., layout.hover == Some(Action::Quit))?;
        self.label_aligned(
            c,
            "退出程序",
            quit,
            13.,
            false,
            c.p.ink,
            1.,
            DWRITE_TEXT_ALIGNMENT_CENTER,
        )?;
        if let Some((area, _)) = layout
            .focus
            .and_then(|index| about.buttons().chain([(quit, Action::Quit)]).nth(index))
        {
            c.stroke_dip(
                area.left - 1.,
                area.top - 1.,
                area.right + 1.,
                area.bottom + 1.,
                8.,
                2.,
                t.accent,
                1.,
            )?;
        }
        Ok(())
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
        if row.key == Key::DesktopCards {
            let on = matches!(row.part, Part::Switch { on: true, .. });
            let tile = Area::new(a.left + 14., middle - 18., a.left + 50., middle + 18.);
            if on {
                let teal = section_color(Section::Plugins);
                c.gradient(
                    tile.left,
                    tile.top,
                    tile.right,
                    tile.bottom,
                    9.,
                    (mix(teal, WHITE, 0.22), 1.),
                    (teal, 1.),
                )?;
                c.stroke(
                    tile.left,
                    tile.top,
                    tile.right,
                    tile.bottom,
                    9.,
                    BLACK,
                    0.08,
                )?;
                c.symbol_in(Symbol::Cards, tile, 19., WHITE, 1.)?;
            } else {
                c.fill(
                    tile.left,
                    tile.top,
                    tile.right,
                    tile.bottom,
                    9.,
                    c.p.line,
                    t.track,
                )?;
                c.symbol_in(Symbol::Cards, tile, 19., c.p.muted, 1.)?;
            }
        }
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
        self.label(
            c,
            row.label,
            label,
            13.,
            row.key == Key::DesktopCards,
            c.p.ink,
            1.,
        )?;
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
            let help_ink = if row.key == Key::DesktopCards
                && matches!(row.part, Part::Switch { on: true, .. })
                && layout.plugins.as_ref().is_some_and(|p| {
                    p.view
                        .installed
                        .iter()
                        .any(|item| item.health == InstalledHealth::Error)
                }) {
                c.p.danger
            } else {
                c.p.muted
            };
            self.label(c, &row.help, help, 11., false, help_ink, 1.)?;
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
    use crate::ui::panel::{
        InstalledHealth, InstalledItem, MarketItem, MarketState, MarketView, PanelState, PluginTab,
    };
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

    #[test]
    #[ignore = "shows a settings panel over a test window on an interactive Windows desktop"]
    fn sidebar_overlays_live_content_with_a_uniform_theme_tint() -> Result<()> {
        use crate::platform::{Desktop, PanelWindow, backdrop::Backdrop, create_bars, monitor};
        use std::time::{Duration, Instant};
        use windows::Win32::Graphics::Dwm::DwmFlush;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, HWND_TOPMOST, MSG, PM_REMOVE, PeekMessageW, SWP_NOACTIVATE,
            SWP_SHOWWINDOW, SetWindowPos,
        };

        let _desktop = Desktop::init()?;
        let (top, bottom) = create_bars()?;
        let mut settings = Settings::default();
        let work = monitor()?.work;
        let layout = PanelLayout::new(&mut PanelState::default(), &settings, 96, work);
        let area = layout.place(work);
        let background = HiddenWindow::create()?;
        let panel = PanelWindow::create(area, layout.caption_pixels(), false)?;
        let mut renderer = Renderer::new(top.0, bottom.0, false, &settings)?;
        let layer = super::super::Bar::new(&renderer.device, background.0)?;
        let device = renderer.device.clone();
        let palette = renderer.palette;
        let set_background = |color: Option<Rgb>| -> Result<()> {
            let (w, h) = (area.width, area.height);
            let surface = paint(&device, palette, w, h, 96, |c| {
                if let Some(rgb) = color {
                    return c.round(0., 0., w as f32, h as f32, 0., rgb);
                }
                let stripes = Backdrop {
                    width: w as u32,
                    height: h as u32,
                    pixels: (0..w * h)
                        .flat_map(|i| {
                            let value = if i % w % 6 < 3 { 20 } else { 235 };
                            [value, value, value, 255]
                        })
                        .collect(),
                };
                c.backdrop(&stripes, Area::new(0., 0., w as f32, h as f32), 0.)
            })?;
            unsafe {
                layer.root.SetContent(&surface)?;
                device.Commit()?;
                device.WaitForCommitCompletion()
            }
        };
        let show = |hwnd, r: Rect| unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                r.x,
                r.y,
                r.width,
                r.height,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            )
        };
        // Sample an unselected navigation label, the empty sidebar beside it, and
        // the opaque gutter between the sidebar and the scrollable page.
        let row = layout.nav[1].0;
        let sidebar_width = panel::SIDEBAR as i32; // 96 DPI
        let strip = Rect {
            x: area.x + row.left as i32 + 37,
            y: area.y + row.top as i32,
            width: sidebar_width + 10 - (row.left as i32 + 37),
            height: (row.bottom - row.top) as i32,
        };
        let label_samples = |frame: &Backdrop| {
            let rows = frame.pixels.chunks_exact(4 * strip.width as usize);
            let middle = rows.clone().nth(strip.height as usize / 2).unwrap();
            let red_at = |x| middle[(x - strip.x) as usize * 4 + 2];
            let sidebar = red_at(area.x + sidebar_width - 36);
            let page = red_at(area.x + sidebar_width + 8);
            let ink = rows
                .flat_map(|row| row[..32 * 4].as_chunks::<4>().0)
                .map(|pixel| pixel[2])
                .max_by_key(|value| value.abs_diff(sidebar))
                .unwrap();
            [sidebar, page, ink]
        };
        let wait_for = |description: &str,
                        rect: Rect,
                        sample: &dyn Fn(&Backdrop) -> [u8; 3],
                        matches: &dyn Fn([u8; 3]) -> bool|
         -> Result<[u8; 3]> {
            let start = Instant::now();
            loop {
                let mut msg = MSG::default();
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        DispatchMessageW(&msg);
                    }
                    DwmFlush()?;
                }
                let values = sample(&Backdrop::capture(rect, 0, false)?);
                if matches(values) {
                    return Ok(values);
                }
                assert!(
                    start.elapsed() < Duration::from_secs(3),
                    "{description}: {values:?}",
                );
            }
        };
        let wait_labels = |description: &str, matches: &dyn Fn([u8; 3]) -> bool| {
            wait_for(description, strip, &label_samples, matches)
        };
        set_background(Some(BLACK))?;
        show(background.0, area)?;
        wait_labels("test window visible", &|[side, page, _]| {
            side < 20 && page < 20
        })?;
        renderer.draw_panel(panel.0, &layout, 96, true)?;
        show(panel.0, area)?;
        wait_labels("light theme over black", &|[side, page, ink]| {
            (145..225).contains(&side) && page > 230 && ink < 70
        })?;
        set_background(Some(WHITE))?;
        wait_labels("light theme over white", &|[side, page, ink]| {
            side >= 225 && page > 230 && ink < 70
        })?;
        renderer.set_appearance(true, &settings);
        renderer.draw_panel(panel.0, &layout, 96, true)?;
        wait_labels("dark theme over white", &|[side, page, ink]| {
            (60..150).contains(&side) && page < 60 && ink > 205
        })?;

        set_background(None)?;
        let probe = Rect {
            width: sidebar_width,
            ..area
        };
        let stripe_samples = |frame: &Backdrop| {
            // Two pixels on the same dark stripe and one on the next light stripe.
            let red = |x, y| frame.pixels[((y * probe.width + x) * 4 + 2) as usize];
            [red(7, 20), red(7, probe.height - 30), red(10, 20)]
        };
        let mut clear_stripes = None;
        for blur in [0, 100] {
            settings.set(Key::Backdrop, blur);
            renderer.set_appearance(false, &settings);
            renderer.draw_panel(panel.0, &layout, 96, true)?;
            let stripes = wait_for(
                "stripes show through a uniform mask",
                probe,
                &stripe_samples,
                &|[top, bottom, light]| {
                    top > 140
                        && top.abs_diff(bottom) <= 2
                        && (20..65).contains(&light.saturating_sub(top))
                },
            )?;
            if let Some(clear) = clear_stripes {
                assert_eq!(stripes, clear, "global glass blur changed the sidebar");
            }
            clear_stripes = Some(stripes);
        }
        renderer.draw_panel(panel.0, &layout, 96, false)?;
        wait_for(
            "opaque mask hides all stripes",
            probe,
            &stripe_samples,
            &|[a, b, c]| a == b && b == c,
        )?;
        Ok(())
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
        let market = MarketView {
            installed: [InstalledItem {
                id: "shortcuts".into(),
                title: "快捷键速查".into(),
                version: Some("1.0.0".into()),
                market_update: Some((0, "1.1.0".into())),
                summary: "2 张卡片 · 显示桌面、切换窗口".into(),
                cards: 2,
                health: InstalledHealth::Ready,
                removable: true,
            }]
            .into(),
            source: Some("官方市场 · 0123456".into()),
            loaded: true,
            items: [
                MarketItem {
                    id: "shortcuts".into(),
                    name: "快捷键速查".into(),
                    version: "1.1.0".into(),
                    installed_version: Some("1.0.0".into()),
                    description: "常用 Windows 与 DeskIsle 快捷键速查卡片".into(),
                    state: MarketState::Update,
                },
                MarketItem {
                    id: "pomodoro".into(),
                    name: "番茄钟专注".into(),
                    version: "0.2.0".into(),
                    installed_version: None,
                    description: "在桌面展示当前专注倒计时与今日完成轮数".into(),
                    state: MarketState::Install,
                },
            ]
            .into(),
            ..Default::default()
        };
        for dark in [false, true] {
            renderer.set_appearance(dark, &settings);
            for section in [
                Section::General,
                Section::Appearance,
                Section::Dock,
                Section::TopBar,
                Section::Plugins,
                Section::About,
            ] {
                let mut state = PanelState {
                    page: SECTIONS.iter().position(|&s| s == section).unwrap(),
                    ..Default::default()
                };
                loop {
                    let layout =
                        PanelLayout::with_market(&mut state, &settings, 120, work, &market);
                    renderer.draw_panel(panel.0, &layout, 120, dark)?;
                    if state.scroll >= layout.max_scroll {
                        break;
                    }
                    state.scroll = (state.scroll + layout.view.bottom - layout.view.top)
                        .min(layout.max_scroll);
                }
                if section == Section::Plugins {
                    for (tab, extra_view) in [
                        (PluginTab::Installed, MarketView::default()),
                        (
                            PluginTab::All,
                            MarketView {
                                error: Some("连接插件市场超时".into()),
                                ..market.clone()
                            },
                        ),
                    ] {
                        state.set_plugin_tab(tab);
                        let layout =
                            PanelLayout::with_market(&mut state, &settings, 120, work, &extra_view);
                        renderer.draw_panel(panel.0, &layout, 120, dark)?;
                    }
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
