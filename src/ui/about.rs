//! The About card: a macOS-style "About This Mac" window with an illustration of the
//! computer, its name and model, the hardware and Windows rows and a button that opens
//! the full Windows page. Layout and hits are in DIPs; drawing lives in the renderer
//! and the window in the platform layer.

use crate::geometry::Rect;
use crate::machine::{Form, Machine};
use crate::ui::panel::Area;

pub const WIDTH: f32 = 280.;
/// The close light's center, at the window's top left as in macOS.
const LIGHT_X: f32 = 20.;
const LIGHT_Y: f32 = 20.;
/// Half the close light's hit square, a little larger than the 12 DIP light.
const LIGHT_HIT: f32 = 8.;
const FIGURE_TOP: f32 = 36.;
const FIGURE_HEIGHT: f32 = 104.;
const MARGIN: f32 = 24.;
pub const NAME_SIZE: f32 = 16.;
pub const MODEL_SIZE: f32 = 11.;
pub const ROW_SIZE: f32 = 13.;
const NAME_HEIGHT: f32 = 24.;
const MODEL_HEIGHT: f32 = 18.;
const LINE: f32 = 18.;
const ROW_GAP: f32 = 6.;
/// Labels are right-aligned against this edge; values start after the gutter.
const LABEL_RIGHT: f32 = 94.;
const GUTTER: f32 = 10.;
const BUTTON_WIDTH: f32 = 104.;
const BUTTON_HEIGHT: f32 = 24.;

/// What a click or key on the card asks the coordinator to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutHit {
    Close,
    /// Opens the About page of Windows Settings.
    More,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub area: Area,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AboutRow {
    pub label: &'static str,
    pub label_area: Area,
    /// The value wrapped at word boundaries to the value column.
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AboutLayout {
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    pub close: Area,
    pub figure: Area,
    pub form: Form,
    pub name: Line,
    pub model: Option<Line>,
    pub rows: Vec<AboutRow>,
    pub more: Area,
    pub hover: Option<AboutHit>,
}

/// Breaks `text` at spaces into lines no wider than `width`. A single word wider than
/// the column keeps a line of its own and ends in an ellipsis when drawn.
fn wrap(text: &str, width: f32, measure: &impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if line.is_empty() || measure(&candidate) <= width {
            line = candidate;
        } else {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

impl AboutLayout {
    /// Lays out `machine` at `dpi`. `measure(text, size, bold)` gives a line's width in DIPs.
    pub fn new(
        machine: &Machine,
        dpi: u32,
        hover: Option<AboutHit>,
        measure: impl Fn(&str, f32, bool) -> f32,
    ) -> Self {
        let scale = dpi as f32 / 96.;
        let width = WIDTH;
        let close = Area::new(
            LIGHT_X - LIGHT_HIT,
            LIGHT_Y - LIGHT_HIT,
            LIGHT_X + LIGHT_HIT,
            LIGHT_Y + LIGHT_HIT,
        );
        let figure = Area::new(
            MARGIN,
            FIGURE_TOP,
            width - MARGIN,
            FIGURE_TOP + FIGURE_HEIGHT,
        );
        let mut y = figure.bottom + 10.;
        let name = Line {
            text: machine.name.clone(),
            area: Area::new(MARGIN, y, width - MARGIN, y + NAME_HEIGHT),
        };
        y += NAME_HEIGHT;
        let model = machine.model.as_ref().map(|text| {
            let line = Line {
                text: text.clone(),
                area: Area::new(MARGIN, y, width - MARGIN, y + MODEL_HEIGHT),
            };
            y += MODEL_HEIGHT;
            line
        });
        y += 14.;
        let value_left = LABEL_RIGHT + GUTTER;
        let value_right = width - MARGIN;
        let value = |text: &str| measure(text, ROW_SIZE, false);
        let rows = machine
            .rows()
            .into_iter()
            .map(|(label, text)| {
                let lines: Vec<Line> = wrap(&text, value_right - value_left, &value)
                    .into_iter()
                    .enumerate()
                    .map(|(i, text)| {
                        let top = y + i as f32 * LINE;
                        Line {
                            text,
                            area: Area::new(value_left, top, value_right, top + LINE),
                        }
                    })
                    .collect();
                let row = AboutRow {
                    label,
                    label_area: Area::new(MARGIN, y, LABEL_RIGHT, y + LINE),
                    lines,
                };
                y += row.lines.len().max(1) as f32 * LINE + ROW_GAP;
                row
            })
            .collect();
        y += 12.;
        let more = Area::new(
            (width - BUTTON_WIDTH) / 2.,
            y,
            (width + BUTTON_WIDTH) / 2.,
            y + BUTTON_HEIGHT,
        );
        Self {
            scale,
            width,
            height: (more.bottom + 20.).ceil(),
            close,
            figure,
            form: machine.form,
            name,
            model,
            rows,
            more,
            hover,
        }
    }

    /// The window size in physical pixels.
    pub fn pixels(&self) -> (i32, i32) {
        (
            (self.width * self.scale).round() as i32,
            (self.height * self.scale).round() as i32,
        )
    }

    /// Centers the window in `work`, in screen pixels.
    pub fn place(&self, work: Rect) -> Rect {
        let (width, height) = self.pixels();
        Rect {
            x: work.x + (work.width - width) / 2,
            y: work.y + ((work.height - height) / 2).max(0),
            width,
            height,
        }
    }

    /// The close light and the button in client pixels, as left, top, right, bottom.
    /// The rest of the card drags the window.
    pub fn controls_pixels(&self) -> [(i32, i32, i32, i32); 2] {
        let px = |a: Area| {
            (
                (a.left * self.scale).floor() as i32,
                (a.top * self.scale).floor() as i32,
                (a.right * self.scale).ceil() as i32,
                (a.bottom * self.scale).ceil() as i32,
            )
        };
        [px(self.close), px(self.more)]
    }

    /// The control under a point in client pixels.
    pub fn hit(&self, x: i32, y: i32) -> Option<AboutHit> {
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        if self.close.contains(x, y) {
            Some(AboutHit::Close)
        } else if self.more.contains(x, y) {
            Some(AboutHit::More)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{Cpu, Gpu, Os};

    /// Every character is half the font size wide.
    fn measure(text: &str, size: f32, _: bool) -> f32 {
        text.chars().count() as f32 * size * 0.5
    }

    fn machine() -> Machine {
        Machine {
            name: "DESKTOP-CAT6HGQ".into(),
            model: Some("ROG Strix SCAR 18".into()),
            form: Form::Laptop,
            cpu: Some(Cpu {
                name: "Intel(R) Core(TM) i9-14900HX".into(),
                mhz: Some(2419),
            }),
            gpu: Some(Gpu {
                name: "GPU".into(),
                memory: 0,
            }),
            memory: None,
            os: Some(Os {
                product: "Windows 10 Pro".into(),
                edition: "Professional".into(),
                build: 26200,
                revision: Some(9711),
            }),
        }
    }

    #[test]
    fn values_wrap_at_spaces_to_the_value_column() {
        let width = |t: &str| measure(t, ROW_SIZE, false);
        assert_eq!(wrap("a bb  ccc", 100., &width), ["a bb ccc"]);
        assert_eq!(wrap("aaaa bbbb cccc", 60., &width), ["aaaa bbbb", "cccc"]);
        // A word wider than the column still gets its line.
        assert_eq!(
            wrap("x aaaaaaaaaaaa y", 30., &width),
            ["x", "aaaaaaaaaaaa", "y"]
        );
        assert!(wrap("   ", 30., &width).is_empty());
    }

    #[test]
    fn rows_stack_below_the_name_and_grow_with_their_lines() {
        let layout = AboutLayout::new(&machine(), 96, None, measure);
        let labels: Vec<_> = layout.rows.iter().map(|row| row.label).collect();
        assert_eq!(labels, ["处理器", "图形卡", "版本"]);
        let cpu = &layout.rows[0];
        assert!(cpu.lines.len() > 1, "{:?}", cpu.lines);
        assert!(layout.model.as_ref().unwrap().area.bottom <= cpu.label_area.top);
        for pair in layout.rows.windows(2) {
            let last = pair[0].lines.last().unwrap().area.bottom;
            assert!(last < pair[1].label_area.top);
        }
        for line in layout.rows.iter().flat_map(|row| &row.lines) {
            assert!(line.area.left > cpu.label_area.right);
            assert!(line.area.right <= layout.width);
        }
        let last = layout
            .rows
            .last()
            .unwrap()
            .lines
            .last()
            .unwrap()
            .area
            .bottom;
        assert!(last < layout.more.top && layout.more.bottom < layout.height);
    }

    #[test]
    fn close_light_and_button_hit_at_each_scale() {
        for dpi in [96, 120, 144, 192] {
            let layout = AboutLayout::new(&machine(), dpi, None, measure);
            let s = layout.scale;
            let at = |a: Area| {
                (
                    ((a.left + a.right) / 2. * s) as i32,
                    (a.middle() * s) as i32,
                )
            };
            let (x, y) = at(layout.close);
            assert_eq!(layout.hit(x, y), Some(AboutHit::Close));
            let (x, y) = at(layout.more);
            assert_eq!(layout.hit(x, y), Some(AboutHit::More));
            let (x, y) = at(layout.name.area);
            assert_eq!(layout.hit(x, y), None);
            let (width, height) = layout.pixels();
            assert_eq!(width, (WIDTH * s).round() as i32);
            let [close, more] = layout.controls_pixels();
            assert!(close.0 >= 0 && close.1 >= 0 && more.2 <= width && more.3 <= height);
        }
    }

    #[test]
    fn card_centers_in_the_work_area() {
        let layout = AboutLayout::new(&machine(), 144, None, measure);
        let work = Rect {
            x: 100,
            y: 40,
            width: 1920,
            height: 1000,
        };
        let r = layout.place(work);
        let (width, height) = layout.pixels();
        assert_eq!((r.width, r.height), (width, height));
        assert_eq!(
            r.x + width / 2,
            work.x + (work.width - width) / 2 + width / 2
        );
        assert!(r.y >= work.y);
    }
}
