use super::Bars;
use crate::integrations::updates::Release;
use crate::platform::about::{AboutEvent, AboutKey, AboutWindow};
use crate::platform::{self, shell, window_dpi};
use crate::render::is_device_lost;
use crate::ui::update_prompt::{Prompt, PromptAction, PromptLayout};
use windows::core::Result;

pub(super) struct UpdatePrompt {
    window: AboutWindow,
    layout: PromptLayout,
    offer: Option<Release>,
    hover: Option<PromptAction>,
}

impl Bars {
    pub(super) fn bring_update_prompt_forward(&self, manual: bool) -> bool {
        let Some(card) = &self.update_prompt else {
            return false;
        };
        if manual {
            card.window.show();
        }
        true
    }

    pub(super) fn show_update_prompt(
        &mut self,
        prompt: Prompt,
        offer: Option<Release>,
    ) -> Result<()> {
        if let Err(error) = self.open_update_prompt(prompt, offer) {
            self.close_update_prompt();
            self.report_update_notice("无法显示更新提示", error)?;
        }
        Ok(())
    }

    fn report_update_notice(&mut self, message: &str, error: windows::core::Error) -> Result<()> {
        // If rebuilding the shared device failed, the normal shutdown path must run.
        if self.renderer.is_none() {
            return Err(error);
        }
        self.tray_notice = Some(format!("{message}（{}）", error.code()));
        platform::time_tray_notice(self.top.0);
        self.draw()
    }

    fn open_update_prompt(&mut self, prompt: Prompt, offer: Option<Release>) -> Result<()> {
        let owner = self.settings_owner();
        let work = platform::window_work_area(owner)?;
        let dpi = self.settings.ui_dpi(window_dpi(owner));
        let renderer = self.renderer.as_ref().expect("renderer bound to bars");
        let mut layout = PromptLayout::new(&prompt, dpi, |text, size, bold| {
            renderer.measure(text, size, bold)
        });
        layout.fit_to_work_area(dpi, work);
        let title = format!("DeskIsle 软件更新 · {}", prompt.headline());
        let window = AboutWindow::create_update(
            layout.place(work),
            layout.controls_pixels(),
            self.dark(),
            &title,
        )?;
        let card = UpdatePrompt {
            window,
            layout,
            offer,
            hover: None,
        };
        card.window
            .describe_update(&prompt.body(), card.layout.label(card.layout.focus));
        self.update_prompt = Some(card);
        self.draw_update_prompt()?;
        self.update_prompt
            .as_ref()
            .expect("prompt opened above")
            .window
            .show();
        Ok(())
    }

    fn close_update_prompt(&mut self) -> Option<Release> {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.close_update_prompt();
        }
        self.update_prompt.take().and_then(|card| card.offer)
    }

    pub(super) fn paint_update_prompt(&mut self) -> Result<()> {
        if let (Some(card), Some(renderer)) = (&self.update_prompt, &mut self.renderer) {
            renderer.draw_update_prompt(
                card.window.0,
                &card.layout,
                card.hover,
                card.layout.focus,
            )?;
        }
        Ok(())
    }

    fn draw_update_prompt(&mut self) -> Result<()> {
        match self.paint_update_prompt() {
            Err(error) if is_device_lost(&error) => self.rebuild_renderer(),
            result => result,
        }
    }

    pub(super) fn retheme_update_prompt(&mut self, dark: bool) -> Result<()> {
        if let Some(card) = &self.update_prompt {
            card.window.set_dark(dark);
            self.draw_update_prompt()?;
        }
        Ok(())
    }

    pub(super) fn refresh_update_prompt_scale(&mut self) -> Result<()> {
        if let Some(card) = &self.update_prompt {
            self.resize_update_prompt(window_dpi(card.window.0))?;
        }
        Ok(())
    }

    fn resize_update_prompt(&mut self, monitor_dpi: u32) -> Result<()> {
        if let Some(card) = &mut self.update_prompt {
            let work = platform::window_work_area(card.window.0)?;
            card.layout
                .fit_to_work_area(self.settings.ui_dpi(monitor_dpi), work);
            let (width, height) = card.layout.pixels();
            card.window
                .resize_update(width, height, card.layout.controls_pixels())?;
            card.hover = None;
            self.draw_update_prompt()?;
        }
        Ok(())
    }

    pub(super) fn update_prompt_event(&mut self, event: AboutEvent) -> Result<()> {
        let Some(card) = &mut self.update_prompt else {
            return Ok(());
        };
        let action = match event {
            AboutEvent::Move(x, y) => {
                let hover = card.layout.hit(x, y);
                if hover == card.hover {
                    return Ok(());
                }
                card.hover = hover;
                None
            }
            AboutEvent::Leave if card.hover.take().is_some() => None,
            AboutEvent::Key(AboutKey::Tab) if card.layout.tab() => {
                card.window
                    .focus_update(card.layout.label(card.layout.focus));
                None
            }
            AboutEvent::Scroll(steps) if card.layout.scroll(-steps * 3) => None,
            AboutEvent::Key(AboutKey::Up) if card.layout.scroll(-1) => None,
            AboutEvent::Key(AboutKey::Down) if card.layout.scroll(1) => None,
            AboutEvent::Key(AboutKey::Copy) => {
                let prompt = &card.layout.prompt;
                let text = format!(
                    "{}\r\n{}",
                    prompt.headline(),
                    prompt.body().replace('\n', "\r\n")
                );
                if let Err(error) = shell::copy_text(card.window.0, text.as_ref()) {
                    self.report_update_notice("无法复制更新提示", error)?;
                }
                return Ok(());
            }
            AboutEvent::Dpi(dpi) => return self.resize_update_prompt(dpi),
            AboutEvent::Click(x, y) => match card.layout.hit(x, y) {
                Some(action) => Some(action),
                None => return Ok(()),
            },
            AboutEvent::Key(AboutKey::Enter) => Some(card.layout.focus),
            AboutEvent::Key(AboutKey::Escape) | AboutEvent::Close => Some(PromptAction::Close),
            _ => return Ok(()),
        };
        if let Some(action) = action {
            let offer = self.close_update_prompt();
            if action == PromptAction::Install
                && let Some(release) = offer
            {
                self.start_update_download(release)?;
            }
            Ok(())
        } else {
            self.draw_update_prompt()
        }
    }
}
