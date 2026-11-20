use windows::Win32::{
    Foundation::HWND,
    Graphics::{DirectComposition::*, DirectWrite::*},
};
use windows::core::Result;

use crate::settings::{Adjust, Key, PreviewKey, SECTIONS, Section};
use crate::ui::panel::{self, Action, Area, PanelLayout, Part};

use super::Renderer;
use super::canvas::{Canvas, Rgb, Text, paint};

const CLOSE_HOVER: Rgb = (196, 43, 28);
const SELECTION: Rgb = (0, 102, 204);
const WHITE: Rgb = (255, 255, 255);

fn section_color(section: Section) -> Rgb {
    match section {
        Section::Appearance => (141, 83, 180),
        Section::Dock => (27, 112, 178),
        Section::System => (87, 102, 116),
        Section::Clock => (15, 128, 138),
        Section::Monitor => (198, 64, 78),
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
    pub fn draw_panel(&mut self, hwnd: HWND, layout: &PanelLayout, dpi: u32) -> Result<()> {
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
            self.paint_panel(c, layout)
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

    fn paint_panel(&self, c: &Canvas, layout: &PanelLayout) -> Result<()> {
        let (w, h) = (layout.width, layout.height);
        let hover = |action: Action| layout.hover == Some(action);
        c.round(0., 0., w, h, 0., c.p.panel_top)?;
        let nav_right = layout.nav[0].0.right + 8.;
        c.round(0., 0., nav_right, h, 0., c.p.panel)?;
        c.fill(0., 0., nav_right, h, 0., c.p.line, 0.045)?;
        c.fill(nav_right, 0., nav_right + 1., h, 0., c.p.line, 0.14)?;

        // Caption: the DeskIsle mark, the title and the close button.
        let mid = panel::CAPTION / 2.;
        let mark = Area::new(15., mid - 12., 39., mid + 12.);
        c.round(mark.left, mark.top, mark.right, mark.bottom, 6., c.p.accent)?;
        self.label_aligned(
            c,
            "◇",
            mark,
            19.,
            true,
            c.p.on_accent,
            1.,
            DWRITE_TEXT_ALIGNMENT_CENTER,
        )?;
        let title = Area {
            left: 48.,
            top: 0.,
            right: nav_right,
            bottom: panel::CAPTION,
        };
        self.label(c, "DeskIsle", title, 16., true, c.p.ink, 1.)?;
        let close = layout.close;
        if hover(Action::Close) {
            c.round(
                close.left,
                close.top,
                close.right,
                close.bottom,
                0.,
                CLOSE_HOVER,
            )?;
        }
        let close_ink = if hover(Action::Close) { WHITE } else { c.p.ink };
        self.glyph_in(c, "\u{E8BB}", close, 10., close_ink, 0.9)?;

        for (index, &(a, section)) in layout.nav.iter().enumerate() {
            if index == layout.page {
                c.round(a.left, a.top, a.right, a.bottom, 6., SELECTION)?;
            } else if hover(Action::Page(index)) {
                c.fill(a.left, a.top, a.right, a.bottom, 6., c.p.line, 0.06)?;
            }
            let glyph = Area::new(
                a.left + 9.,
                a.middle() - 10.,
                a.left + 29.,
                a.middle() + 10.,
            );
            c.round(
                glyph.left,
                glyph.top,
                glyph.right,
                glyph.bottom,
                5.,
                section_color(section),
            )?;
            self.glyph_in(c, section.glyph(), glyph, 13., WHITE, 1.)?;
            let text = Area {
                left: a.left + 38.,
                right: a.right - 8.,
                ..a
            };
            self.label(
                c,
                section.title(),
                text,
                13.,
                index == layout.page,
                if index == layout.page { WHITE } else { c.p.ink },
                1.,
            )?;
        }

        let view = layout.view;
        let section = SECTIONS[layout.page];
        self.label(c, section.title(), layout.title, 18., true, c.p.ink, 1.)?;
        c.fill(
            nav_right,
            panel::CAPTION - 1.,
            w,
            panel::CAPTION,
            0.,
            c.p.line,
            0.10,
        )?;
        c.clipped(c.rect(view.left, view.top, view.right, view.bottom), || {
            for (index, row) in layout.rows.iter().enumerate() {
                if row.card.bottom > view.top && row.card.top < view.bottom {
                    self.paint_row(c, layout, index, row)?;
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
                0.5,
            )?;
        }

        // Footer with the page's reset button.
        c.fill(
            view.left,
            view.bottom,
            view.right,
            view.bottom + 1.,
            0.,
            c.p.line,
            0.07,
        )?;
        let r = layout.reset;
        if section.is_preview() {
            let note = Area {
                left: view.left,
                right: r.left - 8.,
                ..r
            };
            self.label(
                c,
                "界面预览 · 仅本次运行有效",
                note,
                12.,
                false,
                c.p.muted,
                1.,
            )?;
        }
        let alpha = if hover(Action::Reset) { 0.1 } else { 0.05 };
        c.fill(r.left, r.top, r.right, r.bottom, 6., c.p.line, alpha)?;
        c.stroke(r.left, r.top, r.right, r.bottom, 6., c.p.line, 0.12)?;
        self.label_aligned(
            c,
            "恢复默认",
            r,
            12.,
            false,
            c.p.ink,
            1.,
            DWRITE_TEXT_ALIGNMENT_CENTER,
        )
    }

    fn paint_row(
        &self,
        c: &Canvas,
        layout: &PanelLayout,
        index: usize,
        row: &panel::Row,
    ) -> Result<()> {
        let a = row.card;
        let lit = layout.hover == Some(Action::Change(row.key, Adjust::Toggle));
        c.fill(
            a.left,
            a.top,
            a.right,
            a.bottom,
            10.,
            c.p.line,
            if lit { 0.075 } else { 0.045 },
        )?;
        if layout.focus == Some(index) {
            c.stroke(a.left, a.top, a.right, a.bottom, 10., SELECTION, 1.)?;
            c.stroke(
                a.left + 1.,
                a.top + 1.,
                a.right - 1.,
                a.bottom - 1.,
                9.,
                SELECTION,
                0.5,
            )?;
        }
        let text = panel::text_area(row);
        let middle = a.middle();
        let label = Area {
            top: a.top + 8.,
            bottom: middle + 2.,
            ..text
        };
        self.label(c, row.label, label, 14., true, c.p.ink, 1.)?;
        if let Some(detail) = &row.detail {
            let icon = Area::new(a.left + 12., middle - 21., text.left - 12., middle + 21.);
            self.paint_monitor_icon(c, row.key, icon)?;
            if let Some(area) = detail.label_area {
                let caption = Area {
                    left: area.left + 4.,
                    ..area
                };
                self.label(c, detail.label, caption, 12., false, c.p.muted, 1.)?;
            }
            match &detail.part {
                Part::Switch { area, on } => {
                    let (l, t, r, b) = (
                        area.left + 3.,
                        area.top + 3.,
                        area.right - 3.,
                        area.bottom - 3.,
                    );
                    if *on {
                        c.round(l, t, r, b, 4., SELECTION)?;
                        self.glyph_in(c, "\u{E73E}", *area, 12., WHITE, 1.)?;
                    } else {
                        c.stroke(l, t, r, b, 4., c.p.muted, 0.85)?;
                    }
                }
                Part::Segments { options, selected } => {
                    for (i, (area, label)) in options.iter().enumerate() {
                        let selected = i == *selected;
                        let over =
                            layout.hover == Some(Action::Change(detail.key, Adjust::Pick(i)));
                        let (fill, alpha, ink) = if selected {
                            (SELECTION, 1., WHITE)
                        } else {
                            (c.p.line, if over { 0.18 } else { 0.08 }, c.p.ink)
                        };
                        c.fill(
                            area.left,
                            area.top,
                            area.right,
                            area.bottom,
                            10.,
                            fill,
                            alpha,
                        )?;
                        self.label_aligned(
                            c,
                            label,
                            *area,
                            11.,
                            selected,
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
            self.label(c, row.help, help, 12., false, c.p.muted, 1.)?;
        }
        self.paint_panel_control(c, layout, row.key, &row.part)
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
        layout: &PanelLayout,
        key: Key,
        part: &Part,
    ) -> Result<()> {
        let hover = |adjust: Adjust| layout.hover == Some(Action::Change(key, adjust));
        match part {
            Part::Switch { area, on } => {
                let s = *area;
                let radius = (s.bottom - s.top) / 2.;
                let knob = radius - 2.;
                let x = if *on {
                    s.right - radius
                } else {
                    s.left + radius
                };
                if *on {
                    c.round(s.left, s.top, s.right, s.bottom, radius, SELECTION)?;
                } else {
                    c.fill(s.left, s.top, s.right, s.bottom, radius, c.p.line, 0.2)?;
                    c.stroke(s.left, s.top, s.right, s.bottom, radius, c.p.muted, 0.45)?;
                }
                c.round(
                    x - knob,
                    s.middle() - knob,
                    x + knob,
                    s.middle() + knob,
                    knob,
                    WHITE,
                )?;
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
                    let fill = if *enabled && hover(Adjust::Step(step)) {
                        0.14
                    } else {
                        0.06
                    };
                    c.fill(
                        area.left,
                        area.top,
                        area.right,
                        area.bottom,
                        6.,
                        c.p.line,
                        fill,
                    )?;
                    let alpha = if *enabled { 1. } else { 0.35 };
                    self.glyph_in(c, glyph, *area, 11., c.p.ink, alpha)?;
                }
                self.label_aligned(
                    c,
                    text,
                    *value,
                    13.,
                    true,
                    c.p.ink,
                    1.,
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                )?;
            }
            Part::Segments { options, selected } => {
                if let (Some(first), Some(last)) = (options.first(), options.last()) {
                    let (l, t, r, b) = (first.0.left, first.0.top, last.0.right, last.0.bottom);
                    c.fill(l, t, r, b, 7., c.p.line, 0.05)?;
                    c.stroke(l, t, r, b, 7., c.p.edge, 0.8)?;
                }
                for (i, (s, label)) in options.iter().enumerate() {
                    let (ink, bold) = if i == *selected {
                        c.round(
                            s.left + 2.,
                            s.top + 2.,
                            s.right - 2.,
                            s.bottom - 2.,
                            5.,
                            SELECTION,
                        )?;
                        (WHITE, true)
                    } else {
                        if hover(Adjust::Pick(i)) {
                            c.fill(
                                s.left + 2.,
                                s.top + 2.,
                                s.right - 2.,
                                s.bottom - 2.,
                                5.,
                                c.p.line,
                                0.07,
                            )?;
                        }
                        (c.p.muted, false)
                    };
                    self.label_aligned(
                        c,
                        label,
                        *s,
                        12.,
                        bold,
                        ink,
                        1.,
                        DWRITE_TEXT_ALIGNMENT_CENTER,
                    )?;
                }
            }
        }
        Ok(())
    }
}
