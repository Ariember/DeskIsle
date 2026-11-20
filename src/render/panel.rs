use windows::Win32::{
    Foundation::HWND,
    Graphics::{DirectComposition::*, DirectWrite::*},
};
use windows::core::Result;

use crate::settings::{Adjust, Key, PreviewKey, SECTIONS, Section};
use crate::ui::panel::{self, Action, Area, PanelLayout, Part};

use super::Renderer;
use super::canvas::{Canvas, Rgb, Text, paint};

const WHITE: Rgb = (255, 255, 255);
const BLACK: Rgb = (0, 0, 0);
/// The macOS close light, its rim and the cross it shows under the pointer.
const CLOSE_LIGHT: Rgb = (255, 95, 87);
const CLOSE_CROSS: Rgb = (110, 12, 8);
const LIGHT_RADIUS: f32 = 6.;
const GROUP_RADIUS: f32 = 10.;

fn section_color(section: Section) -> Rgb {
    match section {
        Section::Appearance => (141, 83, 180),
        Section::Dock => (27, 112, 178),
        Section::System => (87, 102, 116),
        Section::Clock => (15, 128, 138),
        Section::Monitor => (198, 64, 78),
    }
}

/// Blends `a` toward `b` by `t`, for the highlight at the top of an icon tile.
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
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
    fn new(dark: bool) -> Self {
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
                accent: (10, 132, 255),
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
                accent: (0, 122, 255),
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
        let style = Text {
            size,
            bold,
            align,
            ..Text::default()
        };
        c.draw_text(
            &self.write,
            text,
            c.rect(a.left, a.top, a.right, a.bottom),
            style,
            ink,
            alpha,
        )
    }

    fn glyph_in(
        &self,
        c: &Canvas,
        glyph: &str,
        a: Area,
        size: f32,
        ink: Rgb,
        alpha: f32,
    ) -> Result<()> {
        let style = Text {
            size,
            glyph: true,
            align: DWRITE_TEXT_ALIGNMENT_CENTER,
            ..Text::default()
        };
        c.draw_text(
            &self.write,
            glyph,
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
        let t = Tones::new(c.p.dark);
        let (w, h) = (layout.width, layout.height);
        let hover = |action: Action| layout.hover == Some(action);
        let side = panel::SIDEBAR;
        c.round(side, 0., w, h, 0., t.window)?;

        // The sidebar runs the full height, under the window lights, as in macOS.
        if glass {
            let (tint, alpha) = t.sidebar_glass;
            c.fill(0., 0., side, h, 0., tint, alpha)?;
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
            self.glyph_in(c, "\u{E8BB}", cross, 7., CLOSE_CROSS, 0.9)?;
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
            self.glyph_in(c, section.glyph(), tile, 12., WHITE, 1.)?;
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
            if let (Some(first), Some(last)) = (first, last) {
                let (l, top, r, bottom) = (first.left, first.top, first.right, last.bottom);
                c.fill(l, top, r, bottom, GROUP_RADIUS, c.p.line, t.group)?;
                c.stroke(l, top, r, bottom, GROUP_RADIUS, c.p.line, t.group_rim)?;
            }
            for (index, row) in layout.rows.iter().enumerate() {
                if row.card.bottom > view.top && row.card.top < view.bottom {
                    self.paint_row(c, &t, layout, index, row)?;
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
        let r = layout.reset;
        if section.is_preview() {
            let note = Area {
                left: view.left + 2.,
                right: r.left - 8.,
                ..r
            };
            self.label(
                c,
                "界面预览 · 仅本次运行有效",
                note,
                11.,
                false,
                c.p.muted,
                1.,
            )?;
        }
        self.button_face(c, &t, r, 6., hover(Action::Reset))?;
        self.label_aligned(
            c,
            "恢复默认",
            r,
            13.,
            false,
            c.p.ink,
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
        if index > 0 {
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
        let middle = a.middle();
        let label = Area {
            top: a.top + 8.,
            bottom: middle + 2.,
            ..text
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
                        self.glyph_in(c, "\u{E73E}", *area, 10., WHITE, 1.)?;
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
            self.label(c, row.help, help, 11., false, c.p.muted, 1.)?;
        }
        self.paint_panel_control(c, t, layout, row.key, &row.part)
    }

    fn paint_monitor_icon(&self, c: &Canvas, key: Key, icon: Area) -> Result<()> {
        let middle = icon.middle();
        let x = (icon.left + icon.right) / 2.;
        match key {
            Key::Preview(PreviewKey::Temperature) => {
                c.stroke(
                    x - 3.5,
                    middle - 15.,
                    x + 3.5,
                    middle + 7.,
                    3.5,
                    c.p.ink,
                    1.,
                )?;
                c.round(x - 6., middle + 2., x + 6., middle + 14., 6., c.p.ink)?;
                c.round(x - 1.5, middle - 6., x + 1.5, middle + 6., 1.5, c.p.ink)?;
                for dy in [-10., -5., 0.] {
                    c.round(x + 6., middle + dy, x + 10., middle + dy + 2., 1., c.p.ink)?;
                }
            }
            Key::Preview(PreviewKey::Memory) => {
                c.stroke(x - 15., middle - 8., x + 15., middle + 8., 3., c.p.ink, 1.)?;
                for dx in [-10., -5., 0., 5., 10.] {
                    for dy in [-12., 9.] {
                        c.round(
                            x + dx - 1.,
                            middle + dy,
                            x + dx + 1.,
                            middle + dy + 3.,
                            1.,
                            c.p.ink,
                        )?;
                    }
                }
            }
            Key::Preview(PreviewKey::Cpu) => {
                self.glyph_in(c, "\u{E950}", icon, 29., c.p.ink, 1.)?
            }
            Key::Preview(PreviewKey::Disk) => {
                self.glyph_in(c, "\u{EDA2}", icon, 29., c.p.ink, 1.)?
            }
            _ => {}
        }
        Ok(())
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
            Part::Stepper {
                minus,
                value,
                plus,
                text,
                can_down,
                can_up,
            } => {
                for (area, glyph, enabled, step) in [
                    (minus, "\u{E738}", can_down, -1),
                    (plus, "\u{E710}", can_up, 1),
                ] {
                    let lit = *enabled && hover(Adjust::Step(step));
                    self.button_face(c, t, *area, 6., lit)?;
                    let alpha = if *enabled { 0.9 } else { 0.3 };
                    self.glyph_in(c, glyph, *area, 10., c.p.ink, alpha)?;
                }
                self.label_aligned(
                    c,
                    text,
                    *value,
                    13.,
                    false,
                    c.p.ink,
                    1.,
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                )?;
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
