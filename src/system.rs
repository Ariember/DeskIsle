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

/// What a power plan trades off, so the UI can show it at a glance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanKind {
    Saver,
    /// Keeps the fans quiet, such as a laptop maker's Silent mode.
    Quiet,
    Balanced,
    Performance,
    /// Windows' Ultimate Performance or a maker's Turbo mode.
    Ultimate,
    /// A plan whose purpose neither its GUID nor its name reveals.
    Custom,
}

/// The GUIDs of the plans Windows ships.
const POWER_SAVER: u128 = 0xa1841308_3541_4fab_bc81_f71556f20b4a;
const BALANCED: u128 = 0x381b4222_f694_41f0_9685_ff5bb260df2e;
const HIGH_PERFORMANCE: u128 = 0x8c5e7fda_e8bf_4a96_9a85_a6e23a8c635c;
const ULTIMATE_PERFORMANCE: u128 = 0xe9a42b02_d5df_448d_aa00_03f14749eb61;

impl PowerPlan {
    /// Windows' own plans are known by GUID. A plan copied from one gets a new GUID but
    /// usually keeps a telling name, as do most manufacturer plans, so the name decides
    /// next, matched without regard to case ("Silent", "TURBO"). Words for the top plan
    /// are checked before "性能" and "performance", which names such as "卓越性能" and
    /// "Turbo Performance" also contain.
    pub fn kind(&self) -> PlanKind {
        match self.id {
            POWER_SAVER => return PlanKind::Saver,
            BALANCED => return PlanKind::Balanced,
            HIGH_PERFORMANCE => return PlanKind::Performance,
            ULTIMATE_PERFORMANCE => return PlanKind::Ultimate,
            _ => {}
        }
        let name = self.name.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|word| name.contains(word));
        if has(&["卓越", "ultimate", "turbo", "野兽", "beast", "极速", "增强"]) {
            PlanKind::Ultimate
        } else if has(&["silent", "quiet", "静音", "安静"]) {
            PlanKind::Quiet
        } else if has(&["节能", "省电", "节电", "saver", "eco"]) {
            PlanKind::Saver
        } else if has(&["性能", "performance"]) {
            PlanKind::Performance
        } else if has(&["平衡", "均衡", "balanced"]) {
            PlanKind::Balanced
        } else {
            PlanKind::Custom
        }
    }
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
    /// The Internet profile belongs to an Ethernet adapter.
    pub wired: bool,
    pub cellular: bool,
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

    #[test]
    fn power_plans_are_known_by_guid_then_by_name() {
        let plan = |id, name: &str| {
            PowerPlan {
                id,
                name: name.into(),
                active: false,
            }
            .kind()
        };
        // A built-in plan keeps its kind under any localized or renamed label.
        assert_eq!(plan(POWER_SAVER, "My plan"), PlanKind::Saver);
        assert_eq!(plan(BALANCED, "Balanced"), PlanKind::Balanced);
        assert_eq!(plan(HIGH_PERFORMANCE, "高性能"), PlanKind::Performance);
        assert_eq!(plan(ULTIMATE_PERFORMANCE, "卓越性能"), PlanKind::Ultimate);
        assert_eq!(plan(1, "卓越性能 (副本)"), PlanKind::Ultimate);
        assert_eq!(plan(1, "Lenovo 节能"), PlanKind::Saver);
        assert_eq!(plan(1, "High Performance"), PlanKind::Performance);
        assert_eq!(plan(1, "平衡（推荐）"), PlanKind::Balanced);
        // Laptop makers' modes, often in English on a Chinese system.
        assert_eq!(plan(1, "Silent"), PlanKind::Quiet);
        assert_eq!(plan(1, "静音模式"), PlanKind::Quiet);
        assert_eq!(plan(1, "Turbo"), PlanKind::Ultimate);
        assert_eq!(plan(1, "TURBO Performance"), PlanKind::Ultimate);
        assert_eq!(plan(1, "野兽模式"), PlanKind::Ultimate);
        assert_eq!(plan(1, "均衡"), PlanKind::Balanced);
        assert_eq!(plan(1, "游戏"), PlanKind::Custom);
    }
}
