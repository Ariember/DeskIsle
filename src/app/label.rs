use super::Bars;
use crate::geometry::Rect;
use crate::platform::{LabelWindow, backdrop::Backdrop};
use crate::render::LabelView;
use crate::settings::{AppearanceKey as A, Key};
use crate::ui::layout::DockHit;
use windows::core::Result;

pub(super) struct DockLabel {
    // Release the composition target before destroying its HWND.
    _view: LabelView,
    _window: LabelWindow,
    text: String,
    anchor: (i32, i32),
    dpi: u32,
}

impl Bars {
    pub(super) fn sync_dock_label(&mut self) -> Result<()> {
        let hover = self.hover.filter(|_| {
            self.dock_shown()
                && self.dock_press.is_none()
                && self.settings.enabled(Key::Appearance(A::LabelCustom))
        });
        let Some(slot) = hover.and_then(|hit| self.layout.slots.iter().find(|s| s.target == hit))
        else {
            self.label = None;
            return Ok(());
        };
        let text = match slot.target {
            DockHit::App(index) => self.apps.get(index).map(|a| a.label.as_str()),
            DockHit::Launchpad => Some("启动台"),
            DockHit::Separator(_) => None,
        };
        let Some(text) = text else {
            self.label = None;
            return Ok(());
        };
        let bottom = self.layout.bottom;
        let anchor = (
            bottom.x + slot.rect.x + slot.rect.width / 2,
            self.preview.as_ref().map_or(bottom.y, |p| p.bounds().y),
        );
        let dpi = self.monitor.dpi;
        if self
            .label
            .as_ref()
            .is_some_and(|l| l.anchor == anchor && l.dpi == dpi && l.text == text)
        {
            return Ok(());
        }
        self.label = None;
        let renderer = self.renderer.as_ref().expect("renderer bound to bars");
        let scale = dpi as f32 / 96.;
        let work = self.monitor.work;
        let width =
            ((renderer.measure(text, 13., false) + 20.).clamp(32., 360.) * scale).ceil() as i32;
        let width = width.clamp(1, work.width.max(1));
        let height = (28. * scale).ceil() as i32;
        let rect = Rect {
            x: (anchor.0 - width / 2).clamp(work.x, work.x + (work.width - width).max(0)),
            y: (anchor.1 - (5. * scale).ceil() as i32 - height).max(work.y),
            width,
            height,
        };
        let blur = self.settings.get(Key::Appearance(A::LabelBlur));
        let image = (!renderer.has_live_glass()
            && blur > 0
            && self.settings.get(Key::Appearance(A::LabelOpacity)) < 100)
            .then(|| Backdrop::capture(rect, blur, false).ok())
            .flatten();
        let window = LabelWindow::create(self.bottom.0, rect)?;
        let view = renderer.draw_label(window.0, rect, dpi, text, image.as_ref())?;
        window.show();
        self.label = Some(DockLabel {
            _view: view,
            _window: window,
            text: text.into(),
            dpi,
            anchor,
        });
        Ok(())
    }
}
