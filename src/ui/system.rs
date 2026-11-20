//! The status bar's system controls: which ones appear, the glyph each shows for the
//! current state, and the flyout each opens. The flyouts are menus whose commands
//! change a system feature and leave the flyout open, rebuilt from fresh state.

use crate::system::{Audio, Battery, Display, Media, MediaAction, Network, PowerPlan, RadioKind};
use crate::ui::layout::SystemItem;
use crate::ui::menu::{Command, Entry, Icon, Menu, Slider, SliderTarget, Tile};

/// A change to a system feature, chosen in a flyout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemCommand {
    SetMuted(bool),
    /// Makes the output with this endpoint ID the default.
    SetOutput(String),
    SetPlan(u128),
    SetDark(bool),
    SetHdr(bool),
    SetRadio(RadioKind, bool),
    Media(MediaAction),
}

/// What the status bar icons show. Cheap to query, so it is refreshed on every clock
/// tick and after system notifications.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemStatus {
    pub audio: Option<Audio>,
    pub battery: Option<Battery>,
    pub network: Network,
}

/// How a system control is drawn on the status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A Segoe Fluent Icons glyph, drawn faded when `dim`.
    Glyph { glyph: &'static str, dim: bool },
    /// Two stacked switches, the control center's own mark.
    Switches,
}

impl SystemStatus {
    /// The controls to show, in display order. Battery appears only on computers that
    /// have one.
    pub fn items(&self) -> Vec<SystemItem> {
        let mut items = vec![SystemItem::Network, SystemItem::Display];
        if self.battery.is_some() {
            items.push(SystemItem::Battery);
        }
        items.extend([SystemItem::Sound, SystemItem::Control]);
        items
    }

    pub fn mark(&self, item: SystemItem) -> Mark {
        let glyph = |glyph| Mark::Glyph { glyph, dim: false };
        match item {
            SystemItem::Network if self.network.ssid.is_some() => glyph(WIFI),
            SystemItem::Network if self.network.cellular => glyph("\u{EC05}"),
            SystemItem::Network if self.network.wired => glyph("\u{E839}"),
            SystemItem::Network => Mark::Glyph {
                glyph: WIFI,
                dim: true,
            },
            SystemItem::Display => glyph(DISPLAY),
            SystemItem::Battery => match self.battery {
                Some(battery) => glyph(battery_glyph(battery)),
                None => glyph(BATTERY_UNKNOWN),
            },
            SystemItem::Sound => match &self.audio {
                Some(audio) => glyph(volume_glyph(audio.volume, audio.muted)),
                None => Mark::Glyph {
                    glyph: MUTE,
                    dim: true,
                },
            },
            SystemItem::Control => Mark::Switches,
        }
    }
}

const WIFI: &str = "\u{E701}";
const BLUETOOTH: &str = "\u{E702}";
const DISPLAY: &str = "\u{E7F4}";
const BRIGHTNESS: &str = "\u{E706}";
const MOON: &str = "\u{E708}";
const CONTRAST: &str = "\u{E793}";
const HDR: &str = "\u{E790}";
const SPEAKER: &str = "\u{E7F5}";
const MUTE: &str = "\u{E74F}";
const POWER: &str = "\u{E945}";
const BATTERY_UNKNOWN: &str = "\u{E996}";
const PROJECT: &str = "\u{EBC6}";
const MUSIC: &str = "\u{E8D6}";

pub fn volume_glyph(volume: u8, muted: bool) -> &'static str {
    match volume {
        _ if muted => MUTE,
        0 => "\u{E992}",
        1..=33 => "\u{E993}",
        34..=66 => "\u{E994}",
        _ => "\u{E995}",
    }
}

/// Battery glyphs fill in tenths; charging ones carry a bolt.
pub fn battery_glyph(battery: Battery) -> &'static str {
    const LEVELS: [&str; 11] = [
        "\u{E850}", "\u{E851}", "\u{E852}", "\u{E853}", "\u{E854}", "\u{E855}", "\u{E856}",
        "\u{E857}", "\u{E858}", "\u{E859}", "\u{E83F}",
    ];
    const CHARGING: [&str; 11] = [
        "\u{E85A}", "\u{E85B}", "\u{E85C}", "\u{E85D}", "\u{E85E}", "\u{E85F}", "\u{E860}",
        "\u{E861}", "\u{E862}", "\u{E83E}", "\u{EA93}",
    ];
    let Some(percent) = battery.percent else {
        return BATTERY_UNKNOWN;
    };
    let level = usize::from(percent.min(100) + 5) / 10;
    if battery.charging || battery.plugged {
        CHARGING[level]
    } else {
        LEVELS[level]
    }
}

fn choice(
    menu: &mut Menu<Command>,
    label: impl Into<String>,
    glyph: &'static str,
    on: bool,
    command: Option<SystemCommand>,
) {
    menu.item(
        label,
        Icon::Choice { glyph, on },
        command.map(Command::System),
    );
}

pub fn volume_slider(state: Option<(u8, bool)>) -> Slider<Command> {
    Slider {
        target: SliderTarget::Volume,
        value: state.map(|(volume, muted)| if muted { 0 } else { volume }),
        glyph: state.map_or(MUTE, |(volume, muted)| volume_glyph(volume, muted)),
        button: state.map(|(_, muted)| Command::System(SystemCommand::SetMuted(!muted))),
    }
}

fn add_volume(menu: &mut Menu<Command>, audio: Option<&Audio>) {
    menu.entries.push(Entry::Slider(volume_slider(
        audio.map(|a| (a.volume, a.muted)),
    )));
}

fn brightness_slider(menu: &mut Menu<Command>, display: &Display) {
    menu.entries.push(Entry::Slider(Slider {
        target: SliderTarget::Brightness,
        value: display.brightness,
        glyph: BRIGHTNESS,
        button: None,
    }));
}

const FLYOUT_WIDTH: f32 = 300.;

/// Volume, output devices and the sound settings.
pub fn sound_flyout(audio: Option<&Audio>) -> Menu<Command> {
    let mut menu = Menu::new();
    menu.min_width = FLYOUT_WIDTH;
    menu.label("声音");
    add_volume(&mut menu, audio);
    menu.separator();
    match audio {
        Some(audio) if !audio.outputs.is_empty() => {
            menu.label("输出");
            for output in &audio.outputs {
                // Choosing the current default again is harmless and keeps it clickable.
                let command = Some(SystemCommand::SetOutput(output.id.clone()));
                choice(
                    &mut menu,
                    output.name.clone(),
                    SPEAKER,
                    output.default,
                    command,
                );
            }
        }
        _ => {
            menu.item("没有可用的输出设备", Icon::None, None);
        }
    }
    menu.separator();
    menu.item(
        "声音偏好设置",
        Icon::None,
        Some(Command::Open("ms-settings:sound")),
    );
    menu
}

/// Charge, power plans and the battery settings.
pub fn battery_flyout(battery: Option<Battery>, plans: &[PowerPlan]) -> Menu<Command> {
    let mut menu = Menu::new();
    menu.min_width = FLYOUT_WIDTH;
    if let Some(battery) = battery {
        let percent = battery
            .percent
            .map_or("电量未知".into(), |p| format!("{p}%"));
        let source = match (battery.plugged, battery.charging) {
            (true, true) => "已接通电源 · 正在充电",
            (true, false) => "已接通电源",
            (false, _) => "正在使用电池",
        };
        menu.label(format!("{percent}  {source}"));
        menu.separator();
    }
    if !plans.is_empty() {
        menu.label("电源计划");
        for plan in plans {
            choice(
                &mut menu,
                plan.name.clone(),
                POWER,
                plan.active,
                Some(SystemCommand::SetPlan(plan.id)),
            );
        }
        menu.separator();
    }
    menu.item(
        "电池偏好设置",
        Icon::None,
        Some(Command::Open("ms-settings:batterysaver")),
    );
    menu.item(
        "电源和睡眠设置",
        Icon::None,
        Some(Command::Open("ms-settings:powersleep")),
    );
    menu
}

/// Brightness, dark mode, HDR and the display settings.
pub fn display_flyout(display: &Display) -> Menu<Command> {
    let mut menu = Menu::new();
    menu.min_width = FLYOUT_WIDTH;
    menu.label(if display.brightness.is_some() {
        "显示器"
    } else {
        "显示器 · 不支持调节亮度"
    });
    brightness_slider(&mut menu, display);
    menu.separator();
    choice(
        &mut menu,
        "系统暗色模式",
        CONTRAST,
        display.dark,
        Some(SystemCommand::SetDark(!display.dark)),
    );
    if let Some(on) = display.hdr {
        choice(&mut menu, "HDR", HDR, on, Some(SystemCommand::SetHdr(!on)));
    }
    menu.separator();
    menu.item(
        "显示器偏好设置",
        Icon::None,
        Some(Command::Open("ms-settings:display")),
    );
    menu
}

fn radio_detail(on: Option<bool>, connected: Option<&str>) -> String {
    match (on, connected) {
        (None, _) => "不可用".into(),
        (Some(false), _) => "关闭".into(),
        (Some(true), Some(name)) => name.into(),
        (Some(true), None) => "打开".into(),
    }
}

/// Wi-Fi and Bluetooth switches and the network settings.
pub fn network_flyout(network: &Network) -> Menu<Command> {
    let mut menu = Menu::new();
    menu.min_width = FLYOUT_WIDTH;
    menu.label("网络");
    for (kind, label, glyph, on, connected) in [
        (
            RadioKind::WiFi,
            "Wi-Fi",
            WIFI,
            network.wifi,
            network.ssid.as_deref(),
        ),
        (
            RadioKind::Bluetooth,
            "蓝牙",
            BLUETOOTH,
            network.bluetooth,
            None,
        ),
    ] {
        menu.item(
            label,
            Icon::Choice {
                glyph,
                on: on == Some(true),
            },
            on.map(|on| Command::System(SystemCommand::SetRadio(kind, !on))),
        )
        .detail = radio_detail(on, connected);
    }
    menu.separator();
    menu.item(
        "网络和 Internet 设置",
        Icon::None,
        Some(Command::Open("ms-settings:network")),
    );
    menu.item(
        "蓝牙和其他设备",
        Icon::None,
        Some(Command::Open("ms-settings:bluetooth")),
    );
    menu
}

fn radio_tile(
    kind: RadioKind,
    title: &str,
    glyph: &'static str,
    on: Option<bool>,
    connected: Option<&str>,
) -> Tile<Command> {
    Tile {
        glyph,
        title: title.into(),
        detail: radio_detail(on, connected),
        on: on == Some(true),
        command: Some(match on {
            Some(on) => Command::System(SystemCommand::SetRadio(kind, !on)),
            None => Command::Open(match kind {
                RadioKind::WiFi => "ms-settings:network-wifi",
                RadioKind::Bluetooth => "ms-settings:bluetooth",
            }),
        }),
    }
}

/// Everything at once: radio and mode tiles, brightness, volume and media playback.
pub fn control_flyout(
    status: &SystemStatus,
    display: &Display,
    media: Option<&Media>,
) -> Menu<Command> {
    let network = &status.network;
    let mut menu = Menu::new();
    menu.min_width = 330.;
    menu.entries.push(Entry::Tiles(vec![
        radio_tile(
            RadioKind::WiFi,
            "Wi-Fi",
            WIFI,
            network.wifi,
            network.ssid.as_deref(),
        ),
        radio_tile(
            RadioKind::Bluetooth,
            "蓝牙",
            BLUETOOTH,
            network.bluetooth,
            None,
        ),
    ]));
    menu.entries.push(Entry::Tiles(vec![
        Tile {
            glyph: CONTRAST,
            title: "暗色模式".into(),
            detail: if display.dark { "打开" } else { "关闭" }.into(),
            on: display.dark,
            command: Some(Command::System(SystemCommand::SetDark(!display.dark))),
        },
        Tile {
            glyph: MOON,
            title: "专注".into(),
            detail: "设置".into(),
            on: false,
            command: Some(Command::Open("ms-settings:quiethours")),
        },
        Tile {
            glyph: PROJECT,
            title: "投影".into(),
            detail: "选择模式".into(),
            on: false,
            command: Some(Command::Open("ms-settings-displays-topology:projection")),
        },
    ]));
    menu.label("显示器");
    brightness_slider(&mut menu, display);
    menu.label("声音");
    add_volume(&mut menu, status.audio.as_ref());
    let control = |glyph, action| (glyph, Some(Command::System(SystemCommand::Media(action))));
    menu.entries.push(match media {
        Some(media) => Entry::Player {
            title: if media.title.is_empty() {
                "正在播放".into()
            } else {
                media.title.clone()
            },
            detail: media.artist.clone(),
            buttons: vec![
                control("\u{E892}", MediaAction::Previous),
                control(
                    if media.playing {
                        "\u{E769}"
                    } else {
                        "\u{E768}"
                    },
                    MediaAction::TogglePlay,
                ),
                control("\u{E893}", MediaAction::Next),
            ],
        },
        None => Entry::Player {
            title: "未在播放".into(),
            detail: String::new(),
            buttons: Vec::new(),
        },
    });
    menu
}

/// The music note the player shows beside the title.
pub const PLAYER_GLYPH: &str = MUSIC;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::AudioOutput;

    fn commands(menu: &Menu<Command>) -> Vec<Command> {
        let mut found = Vec::new();
        for index in 0..menu.entries.len() {
            found.extend(menu.command(index).cloned());
            for part in 0..4 {
                found.extend(menu.part_command(index, part).cloned());
            }
        }
        found
    }

    #[test]
    fn status_marks_follow_the_state() {
        let mut status = SystemStatus::default();
        assert!(!status.items().contains(&SystemItem::Battery));
        assert_eq!(
            status.mark(SystemItem::Network),
            Mark::Glyph {
                glyph: WIFI,
                dim: true
            }
        );
        status.battery = Some(Battery {
            percent: Some(64),
            plugged: false,
            charging: false,
        });
        status.network.cellular = true;
        assert_eq!(
            status.mark(SystemItem::Network),
            Mark::Glyph {
                glyph: "\u{EC05}",
                dim: false
            }
        );
        status.network.ssid = Some("home".into());
        let items = status.items();
        assert_eq!(items.last(), Some(&SystemItem::Control));
        assert!(items.contains(&SystemItem::Battery));
        assert_eq!(
            status.mark(SystemItem::Battery),
            Mark::Glyph {
                glyph: "\u{E856}",
                dim: false
            }
        );
        assert_eq!(volume_glyph(80, true), MUTE);
        assert_eq!(volume_glyph(0, false), "\u{E992}");
        assert_eq!(volume_glyph(100, false), "\u{E995}");
        let full = Battery {
            percent: Some(100),
            plugged: true,
            charging: false,
        };
        assert_eq!(battery_glyph(full), "\u{EA93}");
    }

    #[test]
    fn flyouts_offer_the_opposite_of_the_current_state() {
        let audio = Audio {
            volume: 40,
            muted: false,
            outputs: vec![
                AudioOutput {
                    id: "a".into(),
                    name: "Speakers".into(),
                    default: true,
                },
                AudioOutput {
                    id: "b".into(),
                    name: "Headset".into(),
                    default: false,
                },
            ],
        };
        let sound = commands(&sound_flyout(Some(&audio)));
        assert!(sound.contains(&Command::System(SystemCommand::SetMuted(true))));
        assert!(sound.contains(&Command::System(SystemCommand::SetOutput("b".into()))));

        let display = Display {
            brightness: None,
            dark: true,
            hdr: Some(false),
        };
        let shown = commands(&display_flyout(&display));
        assert!(shown.contains(&Command::System(SystemCommand::SetDark(false))));
        assert!(shown.contains(&Command::System(SystemCommand::SetHdr(true))));

        // Missing radios fall back to their settings pages.
        let status = SystemStatus {
            network: Network {
                wifi: Some(true),
                bluetooth: None,
                ..Network::default()
            },
            ..SystemStatus::default()
        };
        let control = commands(&control_flyout(&status, &display, None));
        assert!(control.contains(&Command::System(SystemCommand::SetRadio(
            RadioKind::WiFi,
            false
        ))));
        assert!(control.contains(&Command::Open("ms-settings:bluetooth")));
        assert!(
            !control
                .iter()
                .any(|c| matches!(c, Command::System(SystemCommand::Media(_))))
        );
    }

    #[test]
    fn control_center_keyboard_reaches_tiles_sliders_and_player_buttons() {
        use crate::ui::menu::Focus;
        let status = SystemStatus {
            audio: Some(Audio::default()),
            ..SystemStatus::default()
        };
        let display = Display {
            brightness: Some(80),
            ..Display::default()
        };
        let menu = control_flyout(&status, &display, Some(&Media::default()));
        let mut focus = None;
        let mut ordered = Vec::new();
        while let Some(next) = menu.step(focus, true) {
            if ordered.contains(&next) {
                break;
            }
            if !matches!(next, Focus::Slider(_)) {
                assert!(menu.focused_command(next).is_some());
            }
            ordered.push(next);
            focus = Some(next);
        }
        assert_eq!(
            ordered,
            vec![
                Focus::Part(0, 0),
                Focus::Part(0, 1),
                Focus::Part(1, 0),
                Focus::Part(1, 1),
                Focus::Part(1, 2),
                Focus::Slider(3),
                Focus::Part(5, 0),
                Focus::Slider(5),
                Focus::Part(6, 0),
                Focus::Part(6, 1),
                Focus::Part(6, 2),
            ]
        );
        assert_eq!(menu.step(Some(ordered[0]), false), ordered.last().copied());
    }
}
