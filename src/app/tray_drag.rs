use super::Bars;
use crate::platform::{self, tray::TrayInput};
use crate::storage;
use crate::ui::layout::StatusHit;
use windows::core::Result;

pub(super) struct TrayPress {
    registration: u64,
    origin: (i32, i32),
    threshold: (i32, i32),
    double: bool,
    dragged: bool,
}

impl TrayPress {
    pub(super) fn track(&mut self, x: i32, y: i32) {
        self.dragged |= self.origin.0.abs_diff(x) >= self.threshold.0 as u32
            || self.origin.1.abs_diff(y) >= self.threshold.1 as u32;
    }
}

/// Keep positions for hidden or temporarily absent applications. New registrations
/// follow saved ones in their original display order until the user moves them.
fn move_icon(order: &mut Vec<String>, shown: &[&str], source: &str, target: &str) {
    for &key in shown {
        if !order.iter().any(|saved| saved == key) {
            order.push(key.to_owned());
        }
    }
    let from = order.iter().position(|key| key == source).unwrap();
    let to = order.iter().position(|key| key == target).unwrap();
    let key = order.remove(from);
    order.insert(to, key);
}

impl Bars {
    pub(super) fn press_tray(&mut self, slot: usize, x: i32, y: i32, double: bool) {
        self.top.capture();
        self.tray_press = Some(TrayPress {
            registration: self.tray.icons()[self.shown[slot]].registration,
            origin: (x, y),
            threshold: platform::drag_threshold(self.monitor.dpi),
            double,
            dragged: false,
        });
    }

    pub(super) fn cancel_tray_drag(&mut self) {
        if self.tray_press.take().is_some() {
            self.top.release();
            self.tray.cancel_click();
            self.swallow_click = true;
        }
    }

    pub(super) fn release_tray(&mut self, x: i32, y: i32) -> Result<()> {
        let Some(mut press) = self.tray_press.take() else {
            return Ok(());
        };
        self.top.release();
        press.track(x, y);
        let on_bar = &self.shown[..self.status.tray.len()];
        let StatusHit::Tray(slot) = self.status.hit(x, y) else {
            self.tray.cancel_click();
            return Ok(());
        };
        if !press.dragged {
            if self.tray.icons()[on_bar[slot]].registration == press.registration {
                // Delay native button-down until the gesture is known to be a click.
                self.tray_pointer(
                    slot,
                    TrayInput::Press {
                        right: false,
                        double: press.double,
                    },
                    press.origin.0,
                    press.origin.1,
                )?;
                return self.tray_pointer(slot, TrayInput::Release { right: false }, x, y);
            }
            self.tray.cancel_click();
            return Ok(());
        }
        self.tray.cancel_click();
        let icons = self.tray.icons();
        let target = &icons[on_bar[slot]];
        let Some(source) = on_bar
            .iter()
            .map(|&i| &icons[i])
            .find(|icon| icon.registration == press.registration)
        else {
            return Ok(());
        };
        if source.key == target.key {
            return Ok(());
        }
        let shown: Vec<_> = self.shown.iter().map(|&i| icons[i].key.as_str()).collect();
        move_icon(&mut self.tray_order, &shown, &source.key, &target.key);
        if let Err(error) = storage::save_tray_order(&self.tray_order) {
            platform::report_error(Some(self.top.0), &error);
        }
        self.restatus();
        self.draw()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_to_another_slot_keeps_absent_icon_in_saved_order() {
        let mut saved = vec!["one".into(), "absent".into(), "two".into()];
        let shown = ["one", "two", "new"];
        move_icon(&mut saved, &shown, "new", "two");
        assert_eq!(saved, ["one", "absent", "new", "two"]);
        move_icon(&mut saved, &shown, "one", "two");
        assert_eq!(saved, ["absent", "new", "two", "one"]);
    }
}
