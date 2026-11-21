//! The About card: opening it from the status bar mark's "关于本机", routing its input
//! and keeping it painted through theme, DPI and device changes.

use super::Bars;
use crate::machine::Machine;
use crate::platform::about::{AboutEvent, AboutKey, AboutWindow};
use crate::platform::{self, display, machine, shell};
use crate::render::is_device_lost;
use crate::ui::about::{AboutHit, AboutLayout};
use windows::core::Result;

/// The Windows Settings page with the full device and Windows specifications.
const MORE_INFO: &str = "ms-settings:about";

pub(super) struct About {
    window: AboutWindow,
    machine: Machine,
    layout: AboutLayout,
    dpi: u32,
    /// Whether the card is painted translucent over the acrylic backdrop.
    glass: bool,
}

/// The backdrop shows only where DWM draws acrylic: the build supports it and
/// transparency effects are on. An unreadable preference counts as off.
fn glass(window: &AboutWindow) -> bool {
    window.backdrop() && display::transparency().unwrap_or(false)
}

impl Bars {
    fn about_layout(&self, machine: &Machine, dpi: u32, hover: Option<AboutHit>) -> AboutLayout {
        let renderer = self.renderer.as_ref().expect("renderer bound to bars");
        AboutLayout::new(machine, dpi, hover, |text, size, bold| {
            renderer.measure(text, size, bold)
        })
    }

    /// Opens the card centered on the primary monitor, or brings the open one forward.
    /// The facts are read again on each opening, so the card shows the current release.
    pub(super) fn open_about(&mut self) -> Result<()> {
        if let Some(about) = &self.about {
            about.window.show();
            return Ok(());
        }
        let machine = machine::query();
        let dpi = self.monitor.dpi;
        let layout = self.about_layout(&machine, dpi, None);
        let window = AboutWindow::create(
            layout.place(self.monitor.work),
            layout.controls_pixels(),
            self.dark(),
        )?;
        self.about = Some(About {
            glass: glass(&window),
            window,
            machine,
            layout,
            dpi,
        });
        // Paint before showing so the window never appears empty.
        if let Err(error) = self.draw_about() {
            self.close_about();
            return Err(error);
        }
        if let Some(about) = &self.about {
            about.window.show();
        }
        Ok(())
    }

    fn close_about(&mut self) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.close_about();
        }
        self.about = None;
    }

    /// Paints the open card without recovering from a lost device.
    pub(super) fn paint_about(&mut self) -> Result<()> {
        let (Some(about), Some(renderer)) = (&self.about, self.renderer.as_mut()) else {
            return Ok(());
        };
        renderer.draw_about(about.window.0, &about.layout, about.dpi, about.glass)
    }

    fn draw_about(&mut self) -> Result<()> {
        match self.paint_about() {
            // Rebuilding the device for the bars repaints the card too.
            Err(error) if is_device_lost(&error) => self.rebuild_renderer(),
            result => result,
        }
    }

    /// Follows a palette or transparency change, which Windows announces as a setting
    /// change; `repalette` says whether the renderer switched palettes.
    pub(super) fn retheme_about(&mut self, dark: bool, repalette: bool) -> Result<()> {
        let Some(about) = self.about.as_mut() else {
            return Ok(());
        };
        let glass = glass(&about.window);
        if !repalette && glass == about.glass {
            return Ok(());
        }
        about.glass = glass;
        if repalette {
            about.window.set_dark(dark);
        }
        self.draw_about()
    }

    /// Lays the card out again for its DPI or hover and repaints only when it changed.
    fn relayout_about(&mut self, hover: Option<AboutHit>) -> Result<()> {
        let Some(about) = self.about.as_ref() else {
            return Ok(());
        };
        let layout = self.about_layout(&about.machine, about.dpi, hover);
        let about = self.about.as_mut().expect("checked above");
        if layout == about.layout {
            return Ok(());
        }
        if layout.pixels() != about.layout.pixels() {
            let (width, height) = layout.pixels();
            about
                .window
                .resize(width, height, layout.controls_pixels())?;
        }
        about.layout = layout;
        self.draw_about()
    }

    pub(super) fn about_event(&mut self, event: AboutEvent) -> Result<()> {
        let Some(about) = self.about.as_mut() else {
            return Ok(());
        };
        let (hover, owner) = (about.layout.hover, about.window.0);
        let action = match event {
            AboutEvent::Move(x, y) => {
                let hover = about.layout.hit(x, y);
                return self.relayout_about(hover);
            }
            AboutEvent::Leave => return self.relayout_about(None),
            AboutEvent::Dpi(dpi) => {
                about.dpi = dpi.max(96);
                return self.relayout_about(hover);
            }
            AboutEvent::Click(x, y) => about.layout.hit(x, y),
            AboutEvent::Key(AboutKey::Enter) => Some(AboutHit::More),
            AboutEvent::Key(AboutKey::Escape) | AboutEvent::Close => Some(AboutHit::Close),
        };
        match action {
            Some(AboutHit::Close) => self.close_about(),
            Some(AboutHit::More) => {
                // Like "More Info…" on macOS: the full page replaces the card.
                match shell::execute(owner, MORE_INFO) {
                    Ok(()) => self.close_about(),
                    Err(error) => platform::report_error(Some(owner), &error),
                }
            }
            None => {}
        }
        Ok(())
    }
}
