use super::Bars;
use crate::platform::{self, Event, MenuKey, MenuWindow, audio, display};
use crate::render::is_device_lost;
use crate::ui::layout::{StatusHit, SystemItem};
use crate::ui::menu::{
    self, Anchor, Command, Focus, Hit, Menu, MenuLayout, Slider, SliderTarget, TrayEntry,
};
use crate::ui::system::{settings_page, volume_slider};
use windows::Win32::Foundation::HWND;
use windows::core::{Error, Result};

impl Bars {
    /// Opens the menu for whatever is under a right click. Returns whether to quit.
    pub(super) fn context_menu(&mut self, owner: HWND, x: i32, y: i32) -> Result<bool> {
        if owner == self.top.0 {
            return match self.status.hit(x, y) {
                StatusHit::Tray(slot) => {
                    // The application owns its tray icon's menu.
                    self.tray_click(slot, true)?;
                    Ok(false)
                }
                StatusHit::Overflow => self.tray_menu(),
                // A left click opens the flyout; a right click goes straight to Settings.
                StatusHit::System(item) => self.run_command(Command::Open(settings_page(item))),
                StatusHit::Clock => self.clock_menu(),
                StatusHit::Title => match self.apps.iter().position(|app| app.active) {
                    Some(index) => {
                        let menu = menu::app_menu(&self.apps[index], self.foreground);
                        let anchor =
                            self.top_anchor(self.status.title.x + px(self.monitor.dpi, 60));
                        self.show_menu(menu, anchor)
                    }
                    None => self.bar_menu(),
                },
                // A left click on the mark opens the system menu; a right click keeps
                // DeskIsle's own menu with Settings and Quit.
                StatusHit::Mark | StatusHit::Bar => self.bar_menu(),
            };
        }
        if owner != self.bottom.0 {
            return Ok(false);
        }
        match self.layout.hit(x, y) {
            Some(index) => {
                let menu = menu::app_menu(&self.apps[index], self.foreground);
                let anchor = self.icon_anchor(index);
                self.show_menu(menu, anchor)
            }
            None => self.bar_menu(),
        }
    }

    pub(super) fn bar_menu(&mut self) -> Result<bool> {
        let menu = menu::bar_menu(&self.apps);
        let anchor = self.cursor_anchor()?;
        self.show_menu(menu, anchor)
    }

    /// The system menu, below the status bar mark.
    pub(super) fn mark_menu(&mut self) -> Result<bool> {
        let mark = self.status.mark;
        let anchor = self.top_anchor(mark.x + mark.width / 2);
        self.show_menu(menu::mark_menu(), anchor)
    }

    pub(super) fn clock_menu(&mut self) -> Result<bool> {
        let (date, year) = platform::date();
        let clock = self.status.clock;
        let anchor = self.top_anchor(clock.x + clock.width / 2);
        self.show_menu(menu::clock_menu(&date, &year), anchor)
    }

    pub(super) fn tray_menu(&mut self) -> Result<bool> {
        let on_bar = &self.shown[..self.status.tray.len()];
        let entries = self
            .tray
            .icons()
            .iter()
            .enumerate()
            .filter(|(_, icon)| icon.visible())
            .map(|(index, icon)| TrayEntry {
                index,
                label: &icon.label,
                on_bar: on_bar.contains(&index),
                kept: !self.hidden.contains(&icon.key),
            })
            .collect::<Vec<_>>();
        let menu = menu::tray_menu(&entries);
        self.show_menu(menu, self.tray_overflow_anchor()?)
    }

    pub(super) fn tray_overflow_anchor(&self) -> Result<Anchor> {
        match self.status.overflow {
            Some(r) => Ok(self.top_anchor(r.x + r.width / 2)),
            None => self.cursor_anchor(),
        }
    }

    /// Below the status bar at client `x`.
    pub(super) fn top_anchor(&self, x: i32) -> Anchor {
        Anchor {
            x: self.top_rect.x + x,
            y: self.top_rect.y + self.top_rect.height,
            up: false,
        }
    }

    /// Above the Dock icon of application `index`, or at the pointer when it scrolled out.
    pub(super) fn icon_anchor(&self, index: usize) -> Anchor {
        let bottom = self.layout.bottom;
        match self.layout.slots.iter().find(|slot| slot.index == index) {
            Some(slot) => Anchor {
                x: bottom.x + slot.rect.x + slot.rect.width / 2,
                y: bottom.y,
                up: true,
            },
            None => self.cursor_anchor().unwrap_or(Anchor {
                x: bottom.x + bottom.width / 2,
                y: bottom.y,
                up: true,
            }),
        }
    }

    /// At the pointer, clear of the bar it is over.
    pub(super) fn cursor_anchor(&self) -> Result<Anchor> {
        let point = platform::cursor()?;
        let top = self.top_rect;
        let bottom = self.layout.bottom;
        Ok(if point.y < top.y + top.height {
            Anchor {
                x: point.x,
                y: top.y + top.height,
                up: false,
            }
        } else if point.y >= bottom.y {
            Anchor {
                x: point.x,
                y: bottom.y,
                up: true,
            }
        } else {
            let middle = self.monitor.bounds.y + self.monitor.bounds.height / 2;
            Anchor {
                x: point.x,
                y: point.y,
                up: point.y > middle,
            }
        })
    }

    /// Shows `menu`, then carries out the chosen command. Returns whether to quit.
    fn show_menu(&mut self, menu: Menu<Command>, anchor: Anchor) -> Result<bool> {
        match self.popup(menu, anchor, None)? {
            Some(command) => self.run_command(command),
            None => Ok(false),
        }
    }

    fn menu_layout(&mut self, menu: &mut Menu<Command>) -> MenuLayout {
        let renderer = self.renderer.as_mut().expect("renderer bound to bars");
        MenuLayout::new(menu, self.monitor.dpi, |text, size, bold| {
            renderer.measure(text, size, bold)
        })
    }

    /// Binds and paints `menu` into its window; a failed paint releases the target.
    fn paint_menu(
        &mut self,
        window: &MenuWindow,
        menu: &Menu<Command>,
        layout: &MenuLayout,
        animate: bool,
    ) -> Result<()> {
        let renderer = self.renderer.as_mut().expect("renderer bound to bars");
        let tray = &self.tray;
        let resolve = |index: usize| tray.icons().get(index).and_then(|icon| icon.icon());
        let result =
            renderer.open_menu(window.0, menu, layout, self.monitor.dpi, &resolve, animate);
        if result.is_err() {
            renderer.close_menu();
        }
        result
    }

    /// Runs the menu until the user chooses, dismisses or clicks elsewhere. Bar events
    /// that arrive meanwhile are held and handled afterwards in their original order.
    ///
    /// A `flyout` stays open for its system commands: each runs here, and the flyout is
    /// rebuilt in place from fresh state. Its sliders apply while they are dragged.
    pub(super) fn popup(
        &mut self,
        mut menu: Menu<Command>,
        anchor: Anchor,
        flyout: Option<SystemItem>,
    ) -> Result<Option<Command>> {
        self.tray.cancel_click();
        let mut layout = self.menu_layout(&mut menu);
        let window = MenuWindow::create(layout.place(anchor, self.monitor.bounds))?;
        if let Err(error) = self.paint_menu(&window, &menu, &layout, true) {
            drop(window);
            return self.recover(error).map(|()| None);
        }
        let opened_over = platform::foreground();
        let active = window.show();
        let mut hover: Option<Focus> = None;
        let mut held = Vec::new();
        let mut failure = None;
        // The slider entry being dragged.
        let mut drag: Option<usize> = None;
        let mut refresh_flyout = false;
        let choice = loop {
            let mut target = hover;
            let mut chosen = None;
            let mut slide = None;
            match platform::next_event()? {
                Event::MenuPress(x, y) => {
                    if let Hit::Slider(index, value) = layout.hit(&menu, x, y) {
                        window.capture();
                        drag = Some(index);
                        target = None;
                        slide = Some((index, value, false));
                    }
                }
                Event::MenuMove(x, y) => match drag {
                    Some(index) => {
                        slide = Some((index, layout.slider_value(index, x), false));
                    }
                    None => {
                        target = layout.hit(&menu, x, y).focus();
                    }
                },
                Event::MenuLeave if drag.is_none() => target = None,
                // Releasing a slider, even outside the menu, only ends the drag.
                Event::MenuPick(x, _) if drag.is_some() => {
                    let index = drag.take().expect("dragging");
                    window.release();
                    slide = Some((index, layout.slider_value(index, x), true));
                }
                // The shadow margin belongs to the window but not to the menu.
                Event::MenuPick(x, y) if layout.hit(&menu, x, y) == Hit::Outside => break None,
                Event::MenuPick(x, y) => {
                    chosen = layout
                        .hit(&menu, x, y)
                        .focus()
                        .and_then(|focus| menu.focused_command(focus))
                        .cloned();
                }
                Event::MenuKey(key) => match key {
                    MenuKey::Down => target = menu.step(hover, true),
                    MenuKey::Up => target = menu.step(hover, false),
                    MenuKey::Home => target = menu.step(None, true),
                    MenuKey::End => target = menu.step(None, false),
                    MenuKey::Left | MenuKey::Right => {
                        let forward = key == MenuKey::Right;
                        if let Some(Focus::Slider(index)) = hover {
                            if let menu::Entry::Slider(slider) = &menu.entries[index]
                                && let Some(current) = slider.value
                            {
                                let value = if forward {
                                    current.saturating_add(5).min(100)
                                } else {
                                    current.saturating_sub(5)
                                };
                                slide = Some((index, value, true));
                            }
                        } else {
                            target = menu.step(hover, forward);
                        }
                    }
                    MenuKey::Select => {
                        chosen = hover.and_then(|focus| menu.focused_command(focus).cloned())
                    }
                    MenuKey::Dismiss => break None,
                    _ => {}
                },
                Event::SystemUpdate if flyout.is_some() => {
                    let updated = self.system_updated(window.0)?;
                    refresh_flyout |= updated.is_some()
                        && (flyout == updated || flyout == Some(SystemItem::Control));
                }
                Event::MenuDismiss => break None,
                Event::Press(_, right) => {
                    // A right press goes on to open the menu of what it hit.
                    self.swallow_click = !right;
                    break None;
                }
                Event::Quit => {
                    held.push(Event::Quit);
                    break None;
                }
                Event::Refresh => {
                    held.push(Event::Refresh);
                    // Without activation the menu cannot see clicks elsewhere; another
                    // window coming to the front stands in for them.
                    if !active && !window.is_foreground() && platform::foreground() != opened_over {
                        break None;
                    }
                }
                // A dragged volume slider sends one per step; the latest state is enough.
                event @ (Event::Audio | Event::Power) if held.contains(&event) => {}
                event => held.push(event),
            }
            if let Some((index, value, done)) = slide {
                match self.slide(&window, &mut menu, index, value, done) {
                    Ok(true) => {}
                    Ok(false) => drag = None,
                    Err(error) => {
                        failure = Some(error);
                        break None;
                    }
                }
            }
            match (chosen, flyout) {
                (Some(Command::System(command)), Some(_)) => {
                    if let Err(error) = self.system_command(command) {
                        platform::report_error(Some(window.0), &error);
                    }
                    refresh_flyout = true;
                }
                (Some(command), _) => break Some(command),
                (None, _) => {}
            }
            if refresh_flyout
                && drag.is_none()
                && let Some(item) = flyout
            {
                menu = self.flyout_menu(item);
                layout = self.menu_layout(&mut menu);
                window.place(layout.place(anchor, self.monitor.bounds));
                target = target.filter(|&focus| match focus {
                    Focus::Slider(i) => matches!(menu.entries.get(i), Some(menu::Entry::Slider(s)) if s.value.is_some()),
                    _ => menu.focused_command(focus).is_some(),
                });
                hover = None;
                if let Err(error) = self.paint_menu(&window, &menu, &layout, false) {
                    failure = Some(error);
                    break None;
                }
                refresh_flyout = false;
            }
            if target != hover {
                let danger = matches!(target, Some(Focus::Item(i)) if matches!(&menu.entries[i], menu::Entry::Item(item) if item.danger));
                let bounds = target.map(|focus| layout.focus_bounds(&menu, focus));
                let renderer = self.renderer.as_mut().expect("renderer bound to bars");
                if let Err(error) = renderer.menu_hover(bounds, danger) {
                    failure = Some(error);
                    break None;
                }
                hover = target;
            }
        };
        self.renderer
            .as_mut()
            .expect("renderer bound to bars")
            .close_menu();
        drop(window);
        platform::requeue(held);
        if let Some(error) = failure {
            self.recover(error)?;
        }
        Ok(choice)
    }

    /// Moves slider `index` of `menu` to `value` and applies it: the volume follows the
    /// pointer, while brightness, which talks to the monitor, applies on release. Returns
    /// only drawing errors; a refused change is reported over the menu.
    fn slide(
        &mut self,
        window: &MenuWindow,
        menu: &mut Menu<Command>,
        index: usize,
        value: u8,
        done: bool,
    ) -> Result<bool> {
        let Some(menu::Entry::Slider(slider)) = menu.entries.get_mut(index) else {
            return Ok(false);
        };
        let result = update_slider(
            slider,
            value,
            done,
            |target, value| match target {
                SliderTarget::Volume => audio::set_volume(value),
                SliderTarget::Brightness => display::set_brightness(value),
            },
            |slider| match slider.target {
                SliderTarget::Volume => {
                    *slider = volume_slider(audio::state().ok().map(|a| (a.volume, a.muted)));
                }
                SliderTarget::Brightness => slider.value = display::brightness(),
            },
        );
        if !matches!(result, Ok(false)) {
            self.renderer
                .as_mut()
                .expect("renderer bound to bars")
                .menu_slider(index, slider)?;
        }
        match result {
            Ok(_) => Ok(true),
            Err(error) => {
                if !done {
                    window.release();
                }
                platform::report_error(Some(window.0), &error);
                Ok(false)
            }
        }
    }

    /// Rebuilds the graphics device after it was lost, or reports another menu failure.
    fn recover(&mut self, error: Error) -> Result<()> {
        if is_device_lost(&error) {
            self.draw()
        } else {
            platform::report_error(Some(self.bottom.0), &error);
            Ok(())
        }
    }
}

/// Returns whether the slider needs painting. A failed write can have partially
/// changed the device, so its current state is read before the error is reported.
fn update_slider<E>(
    slider: &mut Slider<Command>,
    value: u8,
    done: bool,
    write: impl FnOnce(SliderTarget, u8) -> std::result::Result<(), E>,
    refresh: impl FnOnce(&mut Slider<Command>),
) -> std::result::Result<bool, E> {
    let changed = slider.value != Some(value);
    let apply = match slider.target {
        SliderTarget::Volume => changed,
        SliderTarget::Brightness => done,
    };
    if apply && let Err(error) = write(slider.target, value) {
        refresh(slider);
        return Err(error);
    }
    if changed {
        match slider.target {
            SliderTarget::Volume => *slider = volume_slider(Some((value, false))),
            SliderTarget::Brightness => slider.value = Some(value),
        }
    }
    Ok(changed)
}

fn px(dpi: u32, dip: i32) -> i32 {
    (dip * dpi as i32 + 48) / 96
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::system::SystemCommand;

    #[test]
    fn raising_muted_volume_makes_the_button_mute_again() {
        let mut slider = volume_slider(Some((30, true)));
        let changed = update_slider(
            &mut slider,
            50,
            false,
            |target, value| {
                assert_eq!((target, value), (SliderTarget::Volume, 50));
                Ok::<_, ()>(())
            },
            |_| panic!("unexpected refresh"),
        );
        assert_eq!(changed, Ok(true));
        assert_eq!(slider.value, Some(50));
        assert_eq!(
            slider.button,
            Some(Command::System(SystemCommand::SetMuted(true)))
        );
    }

    #[test]
    fn failed_slider_writes_show_the_device_value_and_allow_retry() {
        for target in [SliderTarget::Volume, SliderTarget::Brightness] {
            let mut slider = volume_slider(Some((20, false)));
            slider.target = target;
            if target == SliderTarget::Brightness {
                let preview = update_slider(
                    &mut slider,
                    70,
                    false,
                    |_, _| -> Result<()> { panic!("preview wrote to monitor") },
                    |_| unreachable!(),
                );
                assert_eq!(preview, Ok(true));
            }
            let refused = update_slider(
                &mut slider,
                70,
                true,
                |_, _| Err(()),
                |s| s.value = Some(25),
            );
            assert_eq!(refused, Err(()));
            assert_eq!(slider.value, Some(25));
            let mut written = None;
            let retry = update_slider(
                &mut slider,
                70,
                true,
                |t, v| {
                    written = Some((t, v));
                    Ok::<_, ()>(())
                },
                |_| unreachable!(),
            );
            assert_eq!(retry, Ok(true));
            assert_eq!(written, Some((target, 70)));
        }
    }
}
