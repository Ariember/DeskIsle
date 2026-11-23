use super::{Bar, Renderer, Text, paint};
use crate::geometry::Rect;
use crate::platform::backdrop::Backdrop;
use crate::settings::{AppearanceKey as A, Key};
use crate::ui::panel::Area;
use windows::Win32::{Foundation::HWND, Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_CENTER};
use windows::core::Result;

pub(crate) struct LabelView {
    _bar: Bar,
}

impl Renderer {
    pub(crate) fn draw_label(
        &self,
        hwnd: HWND,
        rect: Rect,
        dpi: u32,
        text: &str,
        image: Option<&Backdrop>,
    ) -> Result<LabelView> {
        let bar = Bar::new(&self.device, hwnd)?;
        let opacity = self.appearance.get(Key::Appearance(A::LabelOpacity)) as f32 / 100.;
        let border = self.appearance.get(Key::Appearance(A::LabelBorder)) as f32 / 100.;
        let surface = paint(
            &self.device,
            self.palette,
            rect.width,
            rect.height,
            dpi,
            |c| {
                let w = rect.width as f32 / c.scale;
                let h = rect.height as f32 / c.scale;
                if let Some(image) = image {
                    c.backdrop(image, Area::new(0., 0., w, h), 7.)?;
                }
                c.fill(0., 0., w, h, 7., c.p.panel, opacity)?;
                c.stroke(0., 0., w, h, 7., c.p.edge, border)?;
                c.draw_text(
                    &self.write,
                    text,
                    c.rect(6., 0., w - 6., h),
                    Text {
                        size: 13.,
                        align: DWRITE_TEXT_ALIGNMENT_CENTER,
                        ..Text::default()
                    },
                    c.p.ink,
                    1.,
                )
            },
        )?;
        unsafe {
            bar.root.SetContent(&surface)?;
            self.device.Commit()?;
        }
        Ok(LabelView { _bar: bar })
    }
}
