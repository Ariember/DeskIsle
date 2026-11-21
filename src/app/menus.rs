use super::Bars;
use crate::platform::tray::TrayInput;
use crate::platform::{self, Event, MenuKey, MenuWindow, audio, display};
use crate::render::is_device_lost;
use crate::ui::layout::{DockHit, StatusHit, SystemItem};
use crate::ui::menu::{
    self, Anchor, Command, Focus, Hit, Menu, MenuLayout, Slider, SliderTarget, TrayEntry,
};
use crate::ui::system::{settings_page, volume_slider};
use windows::Win32::Foundation::{HWND, POINT};
use windows::core::{Error, Result};

impl Bars {
    /// Opens the menu for whatever is under a right click. Returns whether to quit.
    pub(super) fn context_menu(&mut self, owner: HWND, x: i32, y: i32) -> Result<bool> {
        if owner == self.top.0 {
            return match self.status.hit(x, y) {
                StatusHit::Tray(slot) => {
                    // The application owns its tray icon's menu.
                    self.tray_pointer(slot, TrayInput::Release { right: true }, x, y)?;
                    Ok(false)
                }
                StatusHit::Overflow => self.tray_menu(),
                // A left click opens the flyout; a right click goes straight to Settings.
                StatusHit::System(item) => self.run_command(Command::Open(settings_page(item))),
                StatusHit::Clock => self.clock_menu(),
                StatusHit::Title => match self.apps.iter().position(|app| app.active) {
                    Some(index) => {
                        let menu = self.app_menu(index);
                        let anchor =
                            self.top_anchor(self.status.title.x + px(self.monitor.dpi, 60));
                        self.show_menu(menu, anchor)
                    }
                    None => self.bar_menu(),
                },
                // A left click on the mark opens the system menu; a right click keeps
                // DeskIsle's own menu with Settings and Quit.
                StatusHit::Mark | StatusHit::Bar | StatusHit::CloseMaximized => self.bar_menu(),
            };
        }
        if owner != self.bottom.0 {
            return Ok(false);
        }
        match self.layout.hit(x, y) {
            Some(DockHit::App(index)) => {
                let menu = self.app_menu(index);
                let anchor = self.icon_anchor(index);
                self.show_menu(menu, anchor)
            }
            Some(DockHit::Launchpad) | None => self.bar_menu(),
        }
    }

    fn app_menu(&mut self, index: usize) -> Menu<Command> {
        let app = &self.apps[index];
        let window = app.current_window(self.foreground).map(|w| w.id);
        let tray_menus = self.tray.app_menus(&app.app_id, window);
        let mut menu = menu::app_menu(app, self.foreground, &tray_menus);
        let limit = self.settings.get(crate::settings::Key::RecentFiles) as u32;
        if limit > 0 {
            let available = self.monitor.work.height;
            let capacity = self.menu_layout(&mut menu).recent_file_capacity(available);
            if let Some(paths) = platform::shell::recent_documents(
                &self.apps[index],
                self.foreground,
                limit.min(capacity),
            ) {
                menu::append_recent_files(&mut menu, paths);
            }
        }
        menu
    }

    pub(super) fn bar_menu(&mut self) -> Result<bool> {
        let menu = menu::bar_menu(&self.apps);
        let anchor = self.cursor_anchor()?;
        self.show_menu(menu, anchor)
    }

    /// The system menu, below the status bar mark.
    pub(super) fn mark_menu(&mut self) -> Result<bool> {
        // The bounce is only feedback: if it fails, the menu still opens.
        if let Some(renderer) = &self.renderer {
            let _ = renderer.bounce_mark();
        }
        let mark = self.status.mark;
        let anchor = self.top_anchor(mark.x + mark.width / 2);
        self.show_menu(menu::mark_menu(), anchor)
    }

    pub(super) fn clock_menu(&mut self) -> Result<bool> {
        let today = platform::today();
        let clock = self.status.clock;
        let anchor = self.top_anchor(clock.x + clock.width / 2);
        self.show_menu(menu::clock_menu(today, (today.0, today.1)), anchor)
    }

    pub(super) fn tray_menu(&mut self) -> Result<bool> {
        // Registrations can arrive between the last paint and the overflow click.
        self.tray.recover_icons();
        if self.tray.update(true) {
            self.restatus();
            self.draw()?;
        }
        if self.status.overflow.is_none() {
            return Ok(false);
        }
        let menu = self.tray_list(&mut 0, None);
        let anchor = self.tray_overflow_anchor()?;
        // The button flips to a collapse arrow while the menu is open.
        self.overflow_open = true;
        self.draw()?;
        let choice = self.popup(menu, anchor, None);
        self.overflow_open = false;
        self.draw()?;
        match choice? {
            Some(command) => self.run_command(command),
            None => Ok(false),
        }
    }

    /// Every visible application tray icon, on the bar or not.
    fn tray_list(&self, page: &mut usize, focused_icon: Option<usize>) -> Menu<Command> {
        let entries = self
            .tray
            .icons()
            .iter()
            .enumerate()
            .filter(|(_, icon)| icon.visible())
            .map(|(index, icon)| TrayEntry {
                index,
                stamp: icon.stamp,
                label: &icon.name,
                status: &icon.label,
                can_interact: icon.can_interact(),
                kept: !self.hidden.contains(&icon.key),
            })
            .collect::<Vec<_>>();
        let available = self.monitor.bounds.y + self.monitor.bounds.height
            - self.top_rect.y
            - self.top_rect.height;
        menu::tray_menu(&entries, page, focused_icon, available, self.monitor.dpi)
    }

    /// Where a click on tray icon `index` happens: its slot on the status bar, or the
    /// overflow button while the icon is not on the bar.
    pub(super) fn tray_anchor(&self, index: usize) -> Result<Anchor> {
        let on_bar = &self.shown[..self.status.tray.len()];
        match on_bar.iter().position(|&shown| shown == index) {
            Some(slot) => {
                let rect = self.status.tray[slot].rect;
                Ok(self.top_anchor(rect.x + rect.width / 2))
            }
            None => self.tray_overflow_anchor(),
        }
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
        match self
            .layout
            .slots
            .iter()
            .find(|slot| slot.target == DockHit::App(index))
        {
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

    /// Returns whether replacing the menu layers also reset the hover highlight.
    fn refresh_menu(
        &mut self,
        window: &MenuWindow,
        anchor: Anchor,
        current: &Menu<Command>,
        next: &mut Menu<Command>,
        layout: &mut MenuLayout,
    ) -> Result<bool> {
        if self.overflow_open
            && let Some(rows) = current.changed_tray_rows(next)
        {
            let renderer = self.renderer.as_ref().expect("renderer bound to bars");
            let resolve = |index: usize| self.tray.icons().get(index).and_then(|icon| icon.icon());
            if renderer.update_tray_rows(next, layout, self.monitor.dpi, &resolve, &rows)? {
                return Ok(false);
            }
        }
        *layout = self.menu_layout(next);
        window.place(layout.place(anchor, self.monitor.bounds));
        self.paint_menu(window, next, layout, false)?;
        Ok(true)
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
        self.set_hover(None)?;
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
        let mut tray_page = 0usize;
        let mut tray_buttons = [false; 2];
        let mut refresh_tray = false;
        let mut windows_changed = false;
        // Accumulated wheel travel, 120 per notch.
        let mut wheel = 0;
        let mut tray_context = None;
        let choice = loop {
            let mut target = hover;
            let mut chosen = None;
            let mut slide = None;
            // The menu's entries, rebuilt in place after a change it keeps open for.
            let mut rebuilt = None;
            let event = platform::next_event()?;
            if tray_buttons.contains(&true)
                && matches!(event, Event::MenuPick(..) | Event::MenuContext(..))
            {
                tray_buttons[usize::from(matches!(event, Event::MenuContext(..)))] = false;
                if !tray_buttons.contains(&true) {
                    window.release();
                }
            }
            match event {
                Event::MenuPress {
                    x,
                    y,
                    right,
                    double,
                } => {
                    if self.overflow_open {
                        // Keep the target under the pointer stable through release, even
                        // when it is dragged off the popup or a registration arrives.
                        window.capture();
                        tray_buttons[usize::from(right)] = true;
                    }
                    let hit = layout.hit(&menu, x, y);
                    let command = hit.focus().and_then(|focus| {
                        if right {
                            menu.context_command(focus.index())
                        } else {
                            menu.focused_command(focus)
                        }
                    });
                    let press = TrayInput::Press { right, double };
                    match command {
                        Some(Command::TrayActivate(index)) => {
                            let rect = layout.place(anchor, self.monitor.bounds);
                            self.tray_input(
                                *index,
                                press,
                                POINT {
                                    x: rect.x + x,
                                    y: rect.y + y,
                                },
                            )?;
                        }
                        Some(Command::TrayMenu(index)) if self.overflow_open && right => {
                            self.tray_anchored_input(*index, press)?;
                        }
                        _ => self.tray.cancel_click(),
                    }
                    if !right && let Hit::Slider(index, value) = hit {
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
                Event::MenuScroll(delta) => {
                    wheel += i32::from(delta);
                    let notches = wheel / 120;
                    wheel -= notches * 120;
                    if notches != 0 {
                        chosen = menu.page_step(notches < 0).cloned();
                    }
                }
                // Releasing a slider, even outside the menu, only ends the drag.
                Event::MenuPick(x, _) | Event::MenuContext(x, _) if drag.is_some() => {
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
                    if let Some(Command::TrayActivate(index)) = chosen {
                        let rect = layout.place(anchor, self.monitor.bounds);
                        self.tray_input(
                            index,
                            TrayInput::Release { right: false },
                            POINT {
                                x: rect.x + x,
                                y: rect.y + y,
                            },
                        )?;
                        // Double-click-only owners leave the list active for the next
                        // click. Owners that open a window dismiss it by taking focus.
                        chosen = None;
                    }
                }
                // A row with a menu of its own opens that menu, as the same click on the
                // status bar does; anywhere else a right click picks like a left one.
                Event::MenuContext(x, y) => match layout.hit(&menu, x, y) {
                    Hit::Outside => break None,
                    hit => {
                        chosen = hit
                            .focus()
                            .and_then(|focus| {
                                menu.context_command(focus.index()).or_else(|| {
                                    menu.focused_command(focus)
                                        .filter(|cmd| !matches!(cmd, Command::TrayToggle(_)))
                                })
                            })
                            .cloned();
                        if self.overflow_open
                            && let Some(Command::TrayMenu(index)) = chosen
                        {
                            // Dismiss our popup before the owner opens its context menu.
                            tray_context = Some((index, TrayInput::Release { right: true }));
                            break None;
                        }
                    }
                },
                Event::MenuKey(key) => match key {
                    MenuKey::Down => target = menu.step(hover, true),
                    MenuKey::Up => target = menu.step(hover, false),
                    MenuKey::Home => target = menu.step(None, true),
                    MenuKey::End => target = menu.step(None, false),
                    MenuKey::PageUp | MenuKey::PageDown => {
                        chosen = menu.page_step(key == MenuKey::PageDown).cloned();
                    }
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
                    MenuKey::Context => {
                        chosen =
                            hover.and_then(|focus| menu.context_command(focus.index()).cloned());
                        if let Some(Command::TrayMenu(index)) = chosen {
                            tray_context = Some((index, TrayInput::ContextMenu));
                            break None;
                        }
                    }
                    MenuKey::Dismiss => break None,
                    _ => {}
                },
                Event::TrayNoticeExpired => self.clear_tray_notice()?,
                Event::MetricsUpdate => {
                    if let Err(error) = self.draw_metrics() {
                        failure = Some(error);
                        break None;
                    }
                }
                Event::SystemUpdate if flyout.is_some() => {
                    let updated = self.system_updated(window.0)?;
                    refresh_flyout |= updated.is_some()
                        && (flyout == updated || flyout == Some(SystemItem::Control));
                }
                Event::MenuDismiss => break None,
                press @ Event::Press { right, .. } => {
                    // Deliver the press as well as the release to the next tray target.
                    if right {
                        held.push(press);
                    }
                    self.swallow_click = !right;
                    break None;
                }
                Event::Quit => {
                    held.push(Event::Quit);
                    break None;
                }
                Event::TrayRecovery if self.overflow_open => self.tray.retry_incomplete_icons(),
                Event::Tray if self.overflow_open => refresh_tray = true,
                Event::Refresh => {
                    if self.overflow_open {
                        self.events.acknowledge();
                        refresh_tray = true;
                        windows_changed = true;
                    }
                    if !held.contains(&Event::Refresh) {
                        held.push(Event::Refresh);
                    }
                    // Without activation the menu cannot see clicks elsewhere; another
                    // window coming to the front stands in for them.
                    if !active && !window.is_foreground() && platform::foreground() != opened_over {
                        break None;
                    }
                }
                // A dragged volume slider sends one per step; the latest state is enough.
                event @ (Event::Audio | Event::Power | Event::InputLanguage)
                    if held.contains(&event) => {}
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
                // The overflow menu stays open so several icons can be toggled in a row.
                (Some(Command::TrayToggle(index)), _) => {
                    if let Err(error) = self.tray_toggle(index) {
                        failure = Some(error);
                        break None;
                    }
                    rebuilt = Some(self.tray_list(&mut tray_page, None));
                }
                (Some(Command::TrayPage(_)), _) if tray_buttons.contains(&true) => {}
                (Some(Command::TrayPage(forward)), _) => {
                    if forward {
                        tray_page += 1;
                    } else {
                        tray_page -= 1;
                    }
                    rebuilt = Some(self.tray_list(&mut tray_page, None));
                    target = None;
                }
                // The calendar turns in place.
                (Some(Command::Calendar(year, month)), _) => {
                    rebuilt = Some(menu::clock_menu(platform::today(), (year, month)));
                }
                (Some(command), _) => break Some(command),
                (None, _) => {}
            }
            if refresh_tray && !tray_buttons.contains(&true) {
                let mut focused = target
                    .and_then(|focus| menu.focused_command(focus))
                    .cloned();
                let focused_icon = match focused {
                    Some(Command::TrayActivate(i) | Command::TrayToggle(i)) => {
                        Some(self.tray.icons()[i].registration)
                    }
                    _ => None,
                };
                if self.tray.update(windows_changed) {
                    self.restatus();
                    self.draw()?;
                    if self.status.overflow.is_none() {
                        break None;
                    }
                    let new_index = focused_icon.and_then(|id| {
                        self.tray
                            .icons()
                            .iter()
                            .position(|icon| icon.registration == id)
                    });
                    if let Some(Command::TrayActivate(index) | Command::TrayToggle(index)) =
                        &mut focused
                    {
                        match new_index {
                            Some(i) => *index = i,
                            None => focused = None,
                        }
                    }
                    let next = self.tray_list(&mut tray_page, new_index);
                    target = focused
                        .as_ref()
                        .and_then(|command| next.tray_focus(command));
                    rebuilt = Some(next);
                }
                refresh_tray = false;
                windows_changed = false;
            }
            if refresh_flyout
                && drag.is_none()
                && let Some(item) = flyout
            {
                rebuilt = self.flyout_menu(item);
                refresh_flyout = false;
            }
            if let Some(mut next) = rebuilt {
                match self.refresh_menu(&window, anchor, &menu, &mut next, &mut layout) {
                    Ok(true) => hover = None,
                    Ok(false) => {}
                    Err(error) => {
                        failure = Some(error);
                        break None;
                    }
                }
                menu = next;
                target = target.filter(|&focus| match focus {
                    Focus::Slider(i) => matches!(menu.entries.get(i), Some(menu::Entry::Slider(s)) if s.value.is_some()),
                    _ => menu.focused_command(focus).is_some(),
                });
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
        if refresh_tray {
            held.push(Event::Tray);
        }
        platform::requeue(held);
        if let Some((index, input)) = tray_context {
            self.tray_anchored_input(index, input)?;
        }
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
