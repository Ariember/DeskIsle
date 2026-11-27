use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::DirectWrite::{DWRITE_TEXT_ALIGNMENT_CENTER, IDWriteFactory};
use windows::core::Result;

use super::canvas::{Canvas, Text, paint};
use super::{Bar, Renderer};
use crate::ui::icon::Symbol;
use crate::ui::panel::Area;
use crate::ui::update_prompt::{BODY_SIZE, Prompt, PromptAction, PromptLayout, WIDTH};

impl Renderer {
    pub fn draw_update_prompt(
        &mut self,
        hwnd: HWND,
        layout: &PromptLayout,
        hover: Option<PromptAction>,
        focus: PromptAction,
    ) -> Result<()> {
        if self.update_prompt.as_ref().map(|(window, _)| *window) != Some(hwnd) {
            self.close_update_prompt();
            self.update_prompt = Some((hwnd, Bar::new(&self.device, hwnd)?));
        }
        let (w, h) = layout.pixels();
        let surface = paint(&self.device, self.palette, w, h, layout.dpi, |c| {
            paint_prompt(c, &self.write, layout, hover, focus)
        })?;
        let (_, target) = self.update_prompt.as_ref().expect("bound above");
        unsafe {
            target.root.SetContent(&surface)?;
            self.device.Commit()
        }
    }

    pub fn close_update_prompt(&mut self) {
        if self.update_prompt.take().is_some() {
            let _ = unsafe { self.device.Commit() };
        }
    }
}

fn paint_prompt(
    c: &Canvas,
    write: &IDWriteFactory,
    layout: &PromptLayout,
    hover: Option<PromptAction>,
    focus: PromptAction,
) -> Result<()> {
    let p = c.p;
    let text = |s: &str, a: Area, style: Text, color| {
        c.draw_text(
            write,
            s,
            c.rect(a.left, a.top, a.right, a.bottom),
            style,
            color,
            1.,
        )
    };
    c.gradient(
        0.,
        0.,
        WIDTH,
        layout.height,
        0.,
        (p.panel_top, 1.),
        (p.panel, 1.),
    )?;
    text(
        "DeskIsle 软件更新",
        Area::new(24., 8., WIDTH - 48., 38.),
        Text {
            size: 12.,
            bold: true,
            ..Text::default()
        },
        p.muted,
    )?;
    let close = layout.close;
    if hover == Some(PromptAction::Close) || focus == PromptAction::Close {
        c.fill(
            close.left,
            close.top,
            close.right,
            close.bottom,
            8.,
            p.line,
            0.08,
        )?;
    }
    c.symbol_in(Symbol::Close, close, 14., p.muted, 1.)?;

    let (symbol, tint) = match layout.prompt {
        Prompt::Current => (
            Symbol::Check,
            if p.dark {
                (102, 221, 175)
            } else {
                (19, 122, 85)
            },
        ),
        Prompt::Available(_) => (Symbol::Restart, p.accent),
        Prompt::Failed(_) => (Symbol::Info, p.danger),
        Prompt::Unsupported => (Symbol::Info, p.muted),
    };
    let tile = Area::new(24., 56., 72., 104.);
    c.gradient(
        tile.left,
        tile.top,
        tile.right,
        tile.bottom,
        14.,
        (tint, 0.18),
        (tint, 0.07),
    )?;
    c.stroke(
        tile.left,
        tile.top,
        tile.right,
        tile.bottom,
        14.,
        tint,
        0.19,
    )?;
    c.symbol_in(symbol, tile, 28., tint, 1.)?;
    text(
        layout.prompt.headline(),
        Area::new(88., 50., WIDTH - 17., 88.),
        Text {
            size: 20.,
            bold: true,
            ..Text::default()
        },
        p.ink,
    )?;
    for (s, area) in layout.visible_lines() {
        text(
            s,
            area,
            Text {
                size: BODY_SIZE,
                ..Text::default()
            },
            p.muted,
        )?;
    }
    if let Some(a) = layout.scrollbar() {
        c.fill(a.left, a.top, a.right, a.bottom, 1.5, p.line, 0.3)?;
    }

    let footer = layout.dismiss.top - 14.;
    c.fill(0., footer, WIDTH, layout.height, 0., p.panel_top, 0.48)?;
    c.fill(0., footer, WIDTH, footer + 1. / c.scale, 0., p.line, 0.09)?;
    let buttons = [
        (Some(layout.dismiss), PromptAction::Dismiss),
        (layout.install, PromptAction::Install),
    ];
    for (area, action) in buttons {
        let Some(a) = area else {
            continue;
        };
        let primary = action == PromptAction::Install || !layout.has_install();
        let face = if primary { p.accent } else { p.line };
        let alpha = if primary { 1. } else { 0.07 };
        c.fill(a.left, a.top, a.right, a.bottom, 8., face, alpha)?;
        if hover == Some(action) {
            c.fill(a.left, a.top, a.right, a.bottom, 8., p.line, 0.09)?;
        }
        if focus == action {
            c.stroke_dip(
                a.left - 2.5,
                a.top - 2.5,
                a.right + 2.5,
                a.bottom + 2.5,
                10.,
                1.5,
                p.accent,
                0.75,
            )?;
        }
        text(
            layout.label(action),
            a,
            Text {
                size: 13.,
                bold: primary,
                align: DWRITE_TEXT_ALIGNMENT_CENTER,
            },
            if primary { p.on_accent } else { p.ink },
        )?;
    }
    Ok(())
}
