use super::Bars;
use crate::platform::{self, Event, MenuKey, MenuWindow, audio, display};
use crate::render::is_device_lost;
use crate::ui::layout::{StatusHit, SystemItem};
use crate::ui::menu::{self, Anchor, Command, Hit, Menu, MenuLayout, SliderTarget, TrayEntry};
use crate::ui::system::volume_glyph;
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
                StatusHit::System(item) => self.flyout(item),
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
                StatusHit::Bar => self.bar_menu(),
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
        let anchor = match self.status.overflow {
            Some(r) => self.top_anchor(r.x + r.width / 2),
            None => self.cursor_anchor()?,
        };
        self.show_menu(menu, anchor)
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
        let mut layout = self.menu_layout(&mut menu);
        let window = MenuWindow::create(layout.place(anchor, self.monitor.bounds))?;
        if let Err(error) = self.paint_menu(&window, &menu, &layout, true) {
            drop(window);
            return self.recover(error).map(|()| None);
        }
        let opened_over = platform::foreground();
        let active = window.show();
        let mut hover: Option<usize> = None;
        let mut held = Vec::new();
        let mut failure = None;
        // The slider entry being dragged.
        let mut drag: Option<usize> = None;
        let choice = loop {
            let mut target = hover;
            let mut chosen = None;
            match platform::next_event()? {
                Event::MenuPress(x, y) => {
                    if let Hit::Slider(index, value) = layout.hit(&menu, x, y) {
                        window.capture();
                        drag = Some(index);
                        target = None;
                        if let Err(error) = self.slide(&window, &mut menu, index, value, false) {
                            failure = Some(error);
                            break None;
                        }
                    }
                }
                Event::MenuMove(x, y) => match drag {
                    Some(index) => {
                        let value = layout.slider_value(index, x);
                        if let Err(error) = self.slide(&window, &mut menu, index, value, false) {
                            failure = Some(error);
                            break None;
                        }
                    }
                    None => {
                        target = match layout.hit(&menu, x, y) {
                            Hit::Item(index) => Some(index),
                            _ => None,
                        }
                    }
                },
                Event::MenuLeave if drag.is_none() => target = None,
                // Releasing a slider, even outside the menu, only ends the drag.
                Event::MenuPick(x, _) if drag.is_some() => {
                    let index = drag.take().expect("dragging");
                    window.release();
                    let value = layout.slider_value(index, x);
                    if let Err(error) = self.slide(&window, &mut menu, index, value, true) {
                        failure = Some(error);
                        break None;
                    }
                }
                // The shadow margin belongs to the window but not to the menu.
                Event::MenuPick(x, y) if layout.hit(&menu, x, y) == Hit::Outside => break None,
                Event::MenuPick(x, y) => {
                    chosen = match layout.hit(&menu, x, y) {
                        Hit::Item(index) => menu.command(index).cloned(),
                        Hit::Part(index, part) => menu.part_command(index, part).cloned(),
                        _ => None,
                    }
                }
                Event::MenuKey(key) => match key {
                    MenuKey::Down => target = menu.step(hover, true),
                    MenuKey::Up => target = menu.step(hover, false),
                    MenuKey::Home => target = menu.step(None, true),
                    MenuKey::End => target = menu.step(None, false),
                    MenuKey::Select => {
                        chosen = hover.and_then(|index| menu.command(index).cloned())
                    }
                    MenuKey::Dismiss => break None,
                    _ => {}
                },
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
            match (chosen, flyout) {
                (Some(Command::System(command)), Some(item)) => {
                    if let Err(error) = self.system_command(command) {
                        platform::report_error(Some(window.0), &error);
                    }
                    menu = self.flyout_menu(item);
                    layout = self.menu_layout(&mut menu);
                    window.place(layout.place(anchor, self.monitor.bounds));
                    hover = None;
                    if let Err(error) = self.paint_menu(&window, &menu, &layout, false) {
                        failure = Some(error);
                        break None;
                    }
                    continue;
                }
                (Some(command), _) => break Some(command),
                (None, _) => {}
            }
            if target != hover {
                let danger = target.is_some_and(
                    |index| matches!(&menu.entries[index], menu::Entry::Item(item) if item.danger),
                );
                let renderer = self.renderer.as_mut().expect("renderer bound to bars");
                if let Err(error) = renderer.menu_hover(target, danger) {
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
    ) -> Result<()> {
        let Some(menu::Entry::Slider(slider)) = menu.entries.get_mut(index) else {
            return Ok(());
        };
        let changed = slider.value != Some(value);
        slider.value = Some(value);
        let applied = match slider.target {
            SliderTarget::Volume if changed => {
                slider.glyph = volume_glyph(value, false);
                audio::set_volume(value)
            }
            SliderTarget::Brightness if done => display::set_brightness(value),
            _ => Ok(()),
        };
        if changed {
            let renderer = self.renderer.as_mut().expect("renderer bound to bars");
            renderer.menu_slider(index, slider)?;
        }
        if let Err(error) = applied {
            platform::report_error(Some(window.0), &error);
        }
        Ok(())
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

fn px(dpi: u32, dip: i32) -> i32 {
    (dip * dpi as i32 + 48) / 96
}
