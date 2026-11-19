use std::time::Instant;

use windows::Win32::{
    Foundation::HWND,
    Graphics::{DirectComposition::*, DirectWrite::*},
    UI::WindowsAndMessaging::HICON,
};
use windows::core::Result;

use crate::ui::menu::{self, Entry, Icon, Menu, MenuLayout};

use super::canvas::{
    Canvas, DANGER, INK, MINT, MUTED, PANEL, PANEL_TOP, Rgb, Text, paint, text_format,
};
use super::{Renderer, ease};

const MENU_RADIUS: f32 = 10.;
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
    rows: Vec<(f32, f32)>,
    hover: Option<usize>,
    /// The highlight's vertical offset in pixels.
    y: Tween,
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

    /// Paints `menu` into `hwnd`, a hidden menu window sized to `layout`, and fades it in.
    /// `tray` resolves tray icon indices while the tray host keeps owning the handles.
    pub fn open_menu<C>(
        &mut self,
        hwnd: HWND,
        menu: &Menu<C>,
        layout: &MenuLayout,
        dpi: u32,
        tray: &dyn Fn(usize) -> Option<HICON>,
    ) -> Result<()> {
        self.close_menu();
        let (width, height) = layout.pixels();
        let (left, top, right, bottom) = layout.panel();
        let background = paint(&self.device, width, height, dpi, |c| {
            c.shadow(left, top, right, bottom, MENU_RADIUS, menu::SHADOW - 2.)?;
            c.gradient(
                left,
                top,
                right,
                bottom,
                MENU_RADIUS,
                (PANEL_TOP, 0.985),
                (PANEL, 0.985),
            )?;
            c.stroke(left, top, right, bottom, MENU_RADIUS, (255, 255, 255), 0.12)
        })?;
        let content = paint(&self.device, width, height, dpi, |c| {
            for (entry, &(row_top, row_bottom)) in menu.entries.iter().zip(&layout.rows) {
                self.paint_entry(c, entry, layout, row_top, row_bottom, tray)?;
            }
            Ok(())
        })?;
        let (row_left, _, row_right, _) = layout.row(0);
        let highlight = |accent: Rgb| {
            paint(
                &self.device,
                ((row_right - row_left) * layout.scale).round() as i32,
                (menu::ITEM_HEIGHT * layout.scale).round() as i32,
                dpi,
                |c| {
                    let (w, h) = (row_right - row_left, menu::ITEM_HEIGHT);
                    c.fill(0., 1., w, h - 1., 6., accent, 0.17)?;
                    c.stroke(0., 1., w, h - 1., 6., accent, 0.22)?;
                    // A short accent bar at the leading edge marks the selected row.
                    c.round(2., h / 2. - 7., 5., h / 2. + 7., 1.5, accent)
                },
            )
        };
        let accent = highlight(MINT)?;
        let danger = highlight(DANGER)?;
        unsafe {
            let target = self.device.CreateTargetForHwnd(hwnd, true)?;
            let root = self.device.CreateVisual()?;
            root.SetContent(&background)?;
            let glow = self.device.CreateVisual()?;
            glow.SetContent(&accent)?;
            glow.SetOffsetX2((row_left * layout.scale).round())?;
            let glow_fade = self.device.CreateEffectGroup()?;
            glow_fade.SetOpacity2(0.)?;
            glow.SetEffect(&glow_fade)?;
            let text = self.device.CreateVisual()?;
            text.SetContent(&content)?;
            root.AddVisual(&glow, true, None)?;
            root.AddVisual(&text, true, None)?;

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
                rows: layout.rows.clone(),
                hover: None,
                y: Tween::at(0.),
            });
        }
        Ok(())
    }

    fn paint_entry<C>(
        &self,
        c: &Canvas,
        entry: &Entry<C>,
        layout: &MenuLayout,
        top: f32,
        bottom: f32,
        tray: &dyn Fn(usize) -> Option<HICON>,
    ) -> Result<()> {
        let (_, _, row_right, _) = layout.row(0);
        let (row_left, _, _, _) = layout.row(0);
        let middle = (top + bottom) / 2.;
        let icon_x = row_left + 8. + 12.;
        match entry {
            Entry::Separator => c.fill(
                row_left + 8.,
                middle - 0.5,
                row_right - 8.,
                middle + 0.5,
                0.,
                (255, 255, 255),
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
                    INK,
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
                    MUTED,
                    0.85,
                )
            }
            Entry::Item(item) => {
                let enabled = item.command.is_some();
                let (ink, alpha) = match (item.danger, enabled) {
                    (_, false) => (MUTED, 0.5),
                    (true, true) => (DANGER, 1.),
                    (false, true) => (INK, 1.),
                };
                match &item.icon {
                    Icon::Glyph(glyph) => c.draw_text(
                        &self.write,
                        glyph,
                        c.rect(icon_x - 10., top, icon_x + 10., bottom),
                        Text {
                            size: 15.,
                            glyph: true,
                            align: DWRITE_TEXT_ALIGNMENT_CENTER,
                            ..Text::default()
                        },
                        if item.danger { DANGER } else { MUTED },
                        alpha,
                    )?,
                    Icon::Window { active } => {
                        let (half, color) = if *active { (3.5, MINT) } else { (2.5, MUTED) };
                        c.fill(
                            icon_x - half,
                            middle - half,
                            icon_x + half,
                            middle + half,
                            half,
                            color,
                            alpha,
                        )?;
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
                let check = if layout.checks {
                    menu::CHECK_COLUMN
                } else {
                    0.
                };
                let label_left = row_left + 8. + menu::ICON_COLUMN;
                let label_right = row_right - menu::TEXT_INSET - check;
                let detail_width = if item.detail.is_empty() {
                    0.
                } else {
                    self.measure(&item.detail, menu::DETAIL_SIZE, false) + 16.
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
                    c.rect(label_left, top, label_right, bottom),
                    Text {
                        size: menu::DETAIL_SIZE,
                        align: DWRITE_TEXT_ALIGNMENT_TRAILING,
                        ..Text::default()
                    },
                    MUTED,
                    0.75 * alpha,
                )?;
                if item.checked == Some(true) {
                    let x = row_right - menu::TEXT_INSET - menu::CHECK_COLUMN / 2.;
                    c.glyph(
                        &self.write,
                        "\u{E73E}",
                        c.rect(x - 10., top, x + 10., bottom),
                        13.,
                        MINT,
                    )?;
                }
                Ok(())
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
            Icon::Tray(index) => tray(*index).and_then(|icon| self.hicon_bitmap(icon).ok()),
            Icon::Glyph(glyph) => {
                c.fill(x, y, x + size, y + size, size * 0.28, MINT, 0.16)?;
                c.glyph(
                    &self.write,
                    glyph,
                    c.rect(x, y, x + size, y + size),
                    size * 0.5,
                    MINT,
                )?;
                return Ok(true);
            }
            Icon::None | Icon::Window { .. } => None,
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

    /// Moves the highlight to entry `index`, sliding from the previous one.
    pub fn menu_hover(&mut self, index: Option<usize>, danger: bool) -> Result<()> {
        let Some(view) = self.menu.as_mut() else {
            return Ok(());
        };
        if view.hover == index {
            return Ok(());
        }
        unsafe {
            match (view.hover, index) {
                (_, None) => {
                    view.glow_fade
                        .SetOpacity(&ease(&self.device, 1., 0., HOVER_DURATION)?)?
                }
                (previous, Some(index)) => {
                    let y = (view.rows[index].0 * view.scale).round();
                    view.glow
                        .SetContent(if danger { &view.danger } else { &view.accent })?;
                    if previous.is_some() {
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
        view.hover = index;
        Ok(())
    }

    /// Releases the menu's composition target; call before destroying its window.
    pub fn close_menu(&mut self) {
        if self.menu.take().is_some() {
            let _ = unsafe { self.device.Commit() };
        }
    }
}
