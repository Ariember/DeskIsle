//! Snapshots of the system features the status bar controls: sound, power, display,
//! wireless radios and media playback. Platform adapters fill them in; the status bar
//! and its flyouts read them. `None` marks a feature this computer does not offer or
//! whose query failed, so the UI can show it as unavailable instead of guessing.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioOutput {
    /// The endpoint ID Windows uses to select the device.
    pub id: String,
    pub name: String,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Audio {
    /// Master volume of the default output in percent.
    pub volume: u8,
    pub muted: bool,
    pub outputs: Vec<AudioOutput>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Battery {
    /// Remaining charge in percent, when the battery reports it.
    pub percent: Option<u8>,
    pub plugged: bool,
    pub charging: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerPlan {
    /// The scheme GUID as a number.
    pub id: u128,
    pub name: String,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Display {
    /// Brightness of the primary monitor in percent, for monitors that support DDC/CI.
    pub brightness: Option<u8>,
    /// Windows uses its dark mode for apps.
    pub dark: bool,
    /// `Some(on)` when the primary monitor supports HDR.
    pub hdr: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioKind {
    WiFi,
    Bluetooth,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Network {
    /// `Some(on)` when the computer has a Wi-Fi radio.
    pub wifi: Option<bool>,
    /// The network Wi-Fi is connected to.
    pub ssid: Option<String>,
    /// Connected to the internet through a network other than Wi-Fi.
    pub wired: bool,
    pub bluetooth: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Media {
    pub title: String,
    pub artist: String,
    pub playing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaAction {
    Previous,
    TogglePlay,
    Next,
}

/// Maps `percent` onto a device range, rounding to the nearest step.
pub fn to_range(percent: u8, min: u32, max: u32) -> u32 {
    let span = max.saturating_sub(min);
    min + (u32::from(percent.min(100)) * span + 50) / 100
}

/// Maps a device value in `min..=max` to a percentage.
pub fn to_percent(value: u32, min: u32, max: u32) -> u8 {
    let span = max.saturating_sub(min);
    if span == 0 {
        return 100;
    }
    (((value.clamp(min, max) - min) * 100 + span / 2) / span) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ranges_round_trip_through_percent() {
        assert_eq!(to_range(0, 0, 100), 0);
        assert_eq!(to_range(100, 20, 80), 80);
        assert_eq!(to_range(50, 20, 80), 50);
        assert_eq!(to_range(200, 0, 10), 10);
        for value in 0..=10 {
            assert_eq!(to_range(to_percent(value, 0, 10), 0, 10), value);
        }
        assert_eq!(to_percent(5, 5, 5), 100);
        assert_eq!(to_percent(999, 0, 100), 100);
    }
}
