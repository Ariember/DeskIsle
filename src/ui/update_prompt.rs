//! Content, geometry and input targets for the software update card, in DIPs.

use super::panel::Area;
use crate::geometry::Rect;

pub const WIDTH: f32 = 384.;
const MARGIN: f32 = 24.;
const TEXT_LEFT: f32 = 88.;
const DETAIL_TOP: f32 = 87.;
const LINE: f32 = 23.;
const MAX_LINES: usize = 5;
pub const BODY_SIZE: f32 = 13.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    Current,
    Available(String),
    Failed(String),
    Unsupported,
}

impl Prompt {
    pub fn body(&self) -> String {
        let current = env!("CARGO_PKG_VERSION");
        match self {
            Self::Current => format!("当前版本 {current}"),
            Self::Available(next) => {
                format!("版本 {current} → {next}\n下载后将为当前用户安装，并重启 DeskIsle。")
            }
            Self::Failed(message) => message.clone(),
            Self::Unsupported => "自动更新仅适用于 Windows x64。".to_owned(),
        }
    }

    pub fn headline(&self) -> &'static str {
        match self {
            Self::Current => "已是最新版本",
            Self::Available(_) => "发现新版本",
            Self::Failed(_) => "更新未完成",
            Self::Unsupported => "当前设备不支持自动更新",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptAction {
    Close,
    Dismiss,
    Install,
}

#[derive(Clone, Debug)]
pub struct PromptLayout {
    pub prompt: Prompt,
    pub focus: PromptAction,
    pub dpi: u32,
    pub close: Area,
    pub dismiss: Area,
    pub install: Option<Area>,
    pub height: f32,
    lines: Vec<String>,
    first_line: usize,
}

/// Wrap both CJK text and unbroken diagnostic identifiers, preferring whitespace
/// when an ASCII word would otherwise split. Preserve explicit paragraph breaks.
fn wrap(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        if measure(paragraph) <= width {
            lines.push(paragraph.to_owned());
            continue;
        }
        let mut line = String::new();
        for ch in paragraph.chars() {
            let end = line.len();
            line.push(ch);
            if end != 0 && measure(&line) > width {
                let split = if ch.is_ascii_alphanumeric() {
                    line[..end]
                        .rfind(char::is_whitespace)
                        .filter(|i| *i > 0)
                        .unwrap_or(end)
                } else {
                    end
                };
                lines.push(line[..split].trim_end().to_owned());
                line = line[split..].trim_start().to_owned();
            }
        }
        lines.push(line);
    }
    lines
}

impl PromptLayout {
    pub fn new(prompt: &Prompt, dpi: u32, measure: impl Fn(&str, f32, bool) -> f32) -> Self {
        let lines = wrap(&prompt.body(), WIDTH - MARGIN - TEXT_LEFT - 8., |s| {
            measure(s, BODY_SIZE, false)
        });
        let content_bottom = DETAIL_TOP + lines.len().clamp(1, MAX_LINES) as f32 * LINE;
        let button_top = content_bottom + 34.;
        let button_bottom = button_top + 34.;
        let right = WIDTH - MARGIN;
        let install = matches!(prompt, Prompt::Available(_))
            .then(|| Area::new(right - 116., button_top, right, button_bottom));
        let dismiss_right = install.map_or(right, |button| button.left - 10.);
        Self {
            prompt: prompt.clone(),
            focus: PromptAction::Dismiss,
            dpi,
            close: Area::new(WIDTH - 39., 7., WIDTH - 7., 39.),
            dismiss: Area::new(
                dismiss_right - 80.,
                button_top,
                dismiss_right,
                button_bottom,
            ),
            install,
            height: button_bottom + 17.,
            lines,
            first_line: 0,
        }
    }

    pub fn fit_to_work_area(&mut self, dpi: u32, work: Rect) {
        let fit = (work.width as f32 * 96. / WIDTH).min(work.height as f32 * 96. / self.height);
        self.dpi = dpi.min(fit.max(1.) as u32);
    }

    pub fn has_install(&self) -> bool {
        self.install.is_some()
    }

    pub fn label(&self, action: PromptAction) -> &'static str {
        match action {
            PromptAction::Install => "下载并安装",
            PromptAction::Dismiss if self.has_install() => "稍后再说",
            PromptAction::Dismiss | PromptAction::Close => "关闭",
        }
    }

    /// A release starts on "稍后再说", so Enter alone never opts into an update.
    pub fn tab(&mut self) -> bool {
        if !self.has_install() {
            return false;
        }
        self.focus = if self.focus == PromptAction::Dismiss {
            PromptAction::Install
        } else {
            PromptAction::Dismiss
        };
        true
    }

    pub fn pixels(&self) -> (i32, i32) {
        let scale = self.dpi as f32 / 96.;
        (
            (WIDTH * scale).round() as i32,
            (self.height * scale).round() as i32,
        )
    }

    pub fn place(&self, work: Rect) -> Rect {
        let (width, height) = self.pixels();
        Rect {
            x: work.x + ((work.width - width) / 2).max(0),
            y: work.y + ((work.height - height) / 2).max(0),
            width,
            height,
        }
    }

    pub fn controls_pixels(&self) -> [(i32, i32, i32, i32); 3] {
        let scale = self.dpi as f32 / 96.;
        [Some(self.close), Some(self.dismiss), self.install].map(|area| {
            area.map_or((0, 0, 0, 0), |a| {
                (
                    (a.left * scale).ceil() as i32,
                    (a.top * scale).ceil() as i32,
                    (a.right * scale).ceil() as i32,
                    (a.bottom * scale).ceil() as i32,
                )
            })
        })
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<PromptAction> {
        self.controls_pixels()
            .into_iter()
            .zip([
                PromptAction::Close,
                PromptAction::Dismiss,
                PromptAction::Install,
            ])
            .find(|((l, t, r, b), _)| (*l..*r).contains(&x) && (*t..*b).contains(&y))
            .map(|(_, action)| action)
    }

    pub fn visible_lines(&self) -> impl Iterator<Item = (&str, Area)> {
        self.lines
            .iter()
            .skip(self.first_line)
            .take(MAX_LINES)
            .enumerate()
            .map(|(i, text)| {
                let top = DETAIL_TOP + i as f32 * LINE;
                (
                    text.as_str(),
                    Area::new(TEXT_LEFT, top, WIDTH - MARGIN - 8., top + LINE),
                )
            })
    }

    pub fn scrollbar(&self) -> Option<Area> {
        if self.lines.len() <= MAX_LINES {
            return None;
        }
        let track = LINE * MAX_LINES as f32;
        let thumb = (track * MAX_LINES as f32 / self.lines.len() as f32).max(12.);
        let top = DETAIL_TOP
            + (track - thumb) * self.first_line as f32 / (self.lines.len() - MAX_LINES) as f32;
        Some(Area::new(
            WIDTH - MARGIN - 3.,
            top,
            WIDTH - MARGIN,
            top + thumb,
        ))
    }

    pub fn scroll(&mut self, lines: i32) -> bool {
        let first = self
            .first_line
            .saturating_add_signed(lines as isize)
            .min(self.lines.len().saturating_sub(MAX_LINES));
        let changed = first != self.first_line;
        self.first_line = first;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(prompt: Prompt, dpi: u32) -> PromptLayout {
        PromptLayout::new(&prompt, dpi, |s, size, _| s.chars().count() as f32 * size)
    }

    #[test]
    fn controls_at_each_scale_match_the_window_hit_regions() {
        for dpi in [96, 120, 144, 192] {
            let layout = layout(Prompt::Available("1.2.3".into()), dpi);
            for ((l, t, r, b), action) in layout.controls_pixels().into_iter().zip([
                PromptAction::Close,
                PromptAction::Dismiss,
                PromptAction::Install,
            ]) {
                assert_eq!(layout.hit(l, t), Some(action));
                assert_eq!(layout.hit(r - 1, b - 1), Some(action));
                assert_eq!(layout.hit(r, b), None);
            }
            assert_eq!(layout.hit(0, 0), None);
        }
        assert!(!layout(Prompt::Current, 96).has_install());
    }

    #[test]
    fn keyboard_install_requires_deliberate_focus_on_a_release() {
        let mut release = layout(Prompt::Available("1.2.3".into()), 96);
        assert_eq!(release.focus, PromptAction::Dismiss);
        assert!(release.tab());
        assert_eq!(release.focus, PromptAction::Install);
        assert!(release.tab());
        assert_eq!(release.focus, PromptAction::Dismiss);
        for prompt in [
            Prompt::Current,
            Prompt::Failed("失败".into()),
            Prompt::Unsupported,
        ] {
            let mut status = layout(prompt, 144);
            assert!(!status.tab());
            assert_eq!(status.focus, PromptAction::Dismiss);
        }
    }

    #[test]
    fn long_errors_wrap_and_remain_readable_to_the_last_line_after_rescaling() {
        let diagnostic = "无法获取版本信息".repeat(80) + "\n错误码 1234";
        let mut layout = layout(Prompt::Failed(diagnostic), 96);
        assert!(layout.scrollbar().is_some());
        assert!(
            layout
                .visible_lines()
                .all(|(s, a)| s.chars().count() as f32 * BODY_SIZE <= a.right - a.left)
        );
        assert!(layout.scroll(i32::MAX));
        let work = Rect {
            x: -480,
            y: 20,
            width: 480,
            height: 320,
        };
        layout.fit_to_work_area(288, work);
        let placed = layout.place(work);
        assert!(placed.width <= work.width && placed.height <= work.height);
        assert!(placed.x >= work.x && placed.y >= work.y);
        assert_eq!(layout.visible_lines().last().unwrap().0, "错误码 1234");
        assert!(!layout.scroll(1));
        assert!(layout.scroll(i32::MIN));
        assert!(
            layout
                .visible_lines()
                .next()
                .unwrap()
                .0
                .starts_with("无法获取版本信息")
        );
    }
}
