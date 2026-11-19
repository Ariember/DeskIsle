use windows::Win32::{
    Foundation::HWND,
    Graphics::{DirectComposition::*, DirectWrite::*},
};
use windows::core::Result;

use crate::settings::{Adjust, SECTIONS};
use crate::ui::panel::{self, Action, Area, PanelLayout, Part};

use super::Renderer;
use super::canvas::{BAR, Canvas, EDGE, INK, MINT, MUTED, PANEL, PANEL_TOP, Rgb, Text, paint};

const WHITE: Rgb = (255, 255, 255);
const CLOSE_HOVER: Rgb = (196, 43, 28);

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
        let surface = paint(&self.device, width, height, dpi, |c| {
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
        c.gradient(0., 0., w, h, 0., (PANEL_TOP, 1.), (PANEL, 1.))?;
        let nav_right = layout.view.left - 8.;
        c.fill(0., 0., nav_right, h, 0., BAR, 0.55)?;
        c.fill(nav_right, 0., nav_right + 1., h, 0., WHITE, 0.06)?;

        // Caption: the DeskIsle mark, the title and the close button.
        let mid = panel::CAPTION / 2. - 4.;
        c.round(18., mid - 5., 22., mid + 5., 2., MINT)?;
        c.round(24., mid - 2., 28., mid + 5., 2., MINT)?;
        let title = Area {
            left: 38.,
            top: 0.,
            right: nav_right,
            bottom: panel::CAPTION - 8.,
        };
        self.label(c, "DeskIsle 设置", title, 13., true, INK, 1.)?;
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
        self.glyph_in(c, "\u{E8BB}", close, 10., INK, 0.9)?;

        for (index, &(a, section)) in layout.nav.iter().enumerate() {
            if index == layout.page {
                c.fill(a.left, a.top, a.right, a.bottom, 6., MINT, 0.13)?;
                c.round(
                    a.left,
                    a.middle() - 8.,
                    a.left + 3.,
                    a.middle() + 8.,
                    1.5,
                    MINT,
                )?;
            } else if hover(Action::Page(index)) {
                c.fill(a.left, a.top, a.right, a.bottom, 6., WHITE, 0.06)?;
            }
            let glyph = Area {
                left: a.left + 8.,
                right: a.left + 36.,
                ..a
            };
            let ink = if index == layout.page { MINT } else { MUTED };
            self.glyph_in(c, section.glyph(), glyph, 15., ink, 1.)?;
            let text = Area {
                left: a.left + 44.,
                right: a.right - 8.,
                ..a
            };
            self.label(c, section.title(), text, 13., index == layout.page, INK, 1.)?;
        }

        let view = layout.view;
        c.clipped(c.rect(view.left, view.top, view.right, view.bottom), || {
            let t = layout.title;
            let title = Area {
                top: t.top + 4.,
                ..t
            };
            self.label(c, SECTIONS[layout.page].title(), title, 20., true, INK, 1.)?;
            for (index, row) in layout.rows.iter().enumerate() {
                self.paint_row(c, layout, index, row)?;
            }
            Ok(())
        })?;

        // Footer with the page's reset button.
        c.fill(
            view.left,
            view.bottom,
            view.right,
            view.bottom + 1.,
            0.,
            WHITE,
            0.07,
        )?;
        let r = layout.reset;
        let alpha = if hover(Action::Reset) { 0.1 } else { 0.05 };
        c.fill(r.left, r.top, r.right, r.bottom, 6., WHITE, alpha)?;
        c.stroke(r.left, r.top, r.right, r.bottom, 6., WHITE, 0.12)?;
        self.label_aligned(
            c,
            "恢复默认",
            r,
            12.,
            false,
            INK,
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
        let hover = |adjust: Adjust| layout.hover == Some(Action::Change(row.key, adjust));
        let a = row.card;
        let lit = matches!(row.part, Part::Switch { .. }) && hover(Adjust::Toggle);
        c.fill(
            a.left,
            a.top,
            a.right,
            a.bottom,
            8.,
            WHITE,
            if lit { 0.075 } else { 0.045 },
        )?;
        c.stroke(a.left, a.top, a.right, a.bottom, 8., WHITE, 0.07)?;
        if layout.focus == Some(index) {
            c.stroke(a.left, a.top, a.right, a.bottom, 8., MINT, 0.9)?;
            c.stroke(
                a.left + 1.,
                a.top + 1.,
                a.right - 1.,
                a.bottom - 1.,
                7.,
                MINT,
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
        self.label(c, row.label, label, 13., false, INK, 1.)?;
        let help = Area {
            top: middle - 1.,
            bottom: a.bottom - 9.,
            ..text
        };
        self.label(c, row.help, help, 12., false, MUTED, 0.8)?;

        match &row.part {
            Part::Switch { area, on } => {
                let s = *area;
                let radius = (s.bottom - s.top) / 2.;
                let knob = radius - 5.;
                let x = if *on {
                    s.right - radius
                } else {
                    s.left + radius
                };
                if *on {
                    c.round(s.left, s.top, s.right, s.bottom, radius, MINT)?;
                    c.round(
                        x - knob,
                        s.middle() - knob,
                        x + knob,
                        s.middle() + knob,
                        knob,
                        PANEL,
                    )?;
                } else {
                    c.stroke(s.left, s.top, s.right, s.bottom, radius, MUTED, 0.8)?;
                    c.round(
                        x - knob,
                        s.middle() - knob,
                        x + knob,
                        s.middle() + knob,
                        knob,
                        MUTED,
                    )?;
                }
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
                        WHITE,
                        fill,
                    )?;
                    let alpha = if *enabled { 1. } else { 0.35 };
                    self.glyph_in(c, glyph, *area, 11., INK, alpha)?;
                }
                self.label_aligned(
                    c,
                    text,
                    *value,
                    13.,
                    true,
                    INK,
                    1.,
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                )?;
            }
            Part::Segments { options, selected } => {
                if let (Some(first), Some(last)) = (options.first(), options.last()) {
                    let (l, t, r, b) = (first.0.left, first.0.top, last.0.right, last.0.bottom);
                    c.fill(l, t, r, b, 7., (0, 0, 0), 0.25)?;
                    c.stroke(l, t, r, b, 7., EDGE, 0.8)?;
                }
                for (i, (s, label)) in options.iter().enumerate() {
                    let (ink, bold) = if i == *selected {
                        c.fill(
                            s.left + 2.,
                            s.top + 2.,
                            s.right - 2.,
                            s.bottom - 2.,
                            5.,
                            MINT,
                            0.2,
                        )?;
                        c.stroke(
                            s.left + 2.,
                            s.top + 2.,
                            s.right - 2.,
                            s.bottom - 2.,
                            5.,
                            MINT,
                            0.55,
                        )?;
                        (INK, true)
                    } else {
                        if hover(Adjust::Pick(i)) {
                            c.fill(
                                s.left + 2.,
                                s.top + 2.,
                                s.right - 2.,
                                s.bottom - 2.,
                                5.,
                                WHITE,
                                0.07,
                            )?;
                        }
                        (MUTED, false)
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
