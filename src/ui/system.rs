//! The status bar's system controls: which ones appear, the icon each shows for the
//! current state, and the flyout each opens. The flyouts are menus whose commands
//! change a system feature and leave the flyout open, rebuilt from fresh state.

use crate::settings::{Key, Settings};
use crate::system::{
    Audio, Battery, Display, Media, MediaAction, Network, PlanKind, PowerPlan, RadioKind,
};
use crate::ui::icon::Symbol;
use crate::ui::layout::SystemItem;
use crate::ui::menu::{Command, DROPDOWN_WIDTH, Entry, Icon, Menu, Slider, SliderTarget, Tile};

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
    pub input_language: Option<&'static str>,
}

/// How a system control is drawn on the status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// An icon of DeskIsle's set, drawn faded when `dim`.
    Icon {
        symbol: Symbol,
        dim: bool,
    },
    Text(&'static str),
    /// The battery icon followed by the charge, when the battery reports it.
    Battery {
        symbol: Symbol,
        percent: Option<u8>,
    },
}

impl SystemStatus {
    /// The controls to show, in display order. Battery appears only on computers that
    /// have one.
    pub fn items(&self, settings: &Settings) -> Vec<SystemItem> {
        let mut items = Vec::new();
        if settings.enabled(Key::ShowInputMethodIcon) {
            items.push(SystemItem::InputMethod);
        }
        if settings.enabled(Key::ShowInputLanguageIcon) && self.input_language.is_some() {
            items.push(SystemItem::InputLanguage);
        }
        if settings.enabled(Key::ShowSearchIcon) {
            items.push(SystemItem::Search);
        }
        items.extend([SystemItem::Network, SystemItem::Display]);
        if self.battery.is_some() {
            items.push(SystemItem::Battery);
        }
        items.extend([SystemItem::Sound, SystemItem::Control]);
        items
    }

    pub fn mark(&self, item: SystemItem) -> Mark {
        let icon = |symbol| Mark::Icon { symbol, dim: false };
        match item {
            SystemItem::InputMethod => icon(Symbol::Keyboard),
            SystemItem::InputLanguage => Mark::Text(self.input_language.unwrap_or_default()),
            SystemItem::Search => icon(Symbol::Search),
            SystemItem::Network if self.network.ssid.is_some() => icon(Symbol::Wifi),
            SystemItem::Network if self.network.cellular => icon(Symbol::Cellular),
            SystemItem::Network if self.network.wired => icon(Symbol::Ethernet),
            SystemItem::Network => Mark::Icon {
                symbol: Symbol::Wifi,
                dim: true,
            },
            SystemItem::Display => icon(Symbol::Display),
            SystemItem::Battery => match self.battery {
                Some(battery) => Mark::Battery {
                    symbol: battery_symbol(battery),
                    percent: battery.percent.map(|p| p.min(100)),
                },
                None => Mark::Battery {
                    symbol: Symbol::Battery {
                        percent: None,
                        charging: false,
                    },
                    percent: None,
                },
            },
            SystemItem::Sound => match &self.audio {
                Some(audio) => icon(volume_symbol(audio.volume, audio.muted)),
                None => Mark::Icon {
                    symbol: Symbol::Muted,
                    dim: true,
                },
            },
            SystemItem::Control => icon(Symbol::Controls),
        }
    }
}

/// The Windows settings page a right click on `item` opens directly, skipping the
/// flyout a left click shows.
pub fn settings_page(item: SystemItem) -> &'static str {
    match item {
        SystemItem::InputMethod | SystemItem::InputLanguage => "ms-settings:typing",
        SystemItem::Search => "ms-settings:search-permissions",
        SystemItem::Network => "ms-settings:network",
        SystemItem::Display => "ms-settings:display",
        SystemItem::Battery => "ms-settings:powersleep",
        SystemItem::Sound => "ms-settings:sound",
        SystemItem::Control => "ms-settings:",
    }
}

/// Volume change per wheel notch over the sound icon, in percent.
pub const VOLUME_STEP: i32 = 2;

/// The volume to set after `notches` wheel notches over the sound icon (positive is
/// up), or `None` when nothing changes. Turning a muted output down leaves it muted;
/// turning it up unmutes it, as dragging the slider does.
pub fn wheel_volume(audio: &Audio, notches: i32) -> Option<u8> {
    if notches == 0 || (audio.muted && notches < 0) {
        return None;
    }
    let volume = (i32::from(audio.volume) + notches * VOLUME_STEP).clamp(0, 100) as u8;
    (volume != audio.volume || audio.muted).then_some(volume)
}

pub fn volume_symbol(volume: u8, muted: bool) -> Symbol {
    match volume {
        _ if muted => Symbol::Muted,
        0 => Symbol::Volume(0),
        1..=33 => Symbol::Volume(1),
        34..=66 => Symbol::Volume(2),
        _ => Symbol::Volume(3),
    }
}

/// The battery fills with the charge; on external power it carries a bolt.
pub fn battery_symbol(battery: Battery) -> Symbol {
    Symbol::Battery {
        percent: battery.percent.map(|p| p.min(100)),
        charging: battery.charging || battery.plugged,
    }
}

fn choice(
    menu: &mut Menu<Command>,
    label: impl Into<String>,
    symbol: Symbol,
    on: bool,
    command: Option<SystemCommand>,
) {
    menu.item(
        label,
        Icon::Choice { symbol, on },
        command.map(Command::System),
    );
}

pub fn volume_slider(state: Option<(u8, bool)>) -> Slider<Command> {
    Slider {
        target: SliderTarget::Volume,
        value: state.map(|(volume, muted)| if muted { 0 } else { volume }),
        symbol: state.map_or(Symbol::Muted, |(volume, muted)| {
            volume_symbol(volume, muted)
        }),
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
        symbol: Symbol::Sun,
        button: None,
    }));
}

/// Volume, output devices and the sound settings.
pub fn sound_flyout(audio: Option<&Audio>) -> Menu<Command> {
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    menu.label("声音");
    add_volume(&mut menu, audio);
    menu.separator();
    match audio {
        Some(audio) if !audio.outputs.is_empty() => {
            menu.label("输出设备");
            for output in &audio.outputs {
                // Choosing the current default again is harmless and keeps it clickable.
                let command = Some(SystemCommand::SetOutput(output.id.clone()));
                choice(
                    &mut menu,
                    output.name.clone(),
                    Symbol::Speaker,
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
        "声音设置",
        Icon::Symbol(Symbol::Settings),
        Some(Command::Open("ms-settings:sound")),
    );
    menu
}

/// Charge, power plans and the battery settings.
pub fn battery_flyout(battery: Option<Battery>, plans: &[PowerPlan]) -> Menu<Command> {
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    if let Some(battery) = battery {
        let source = match (battery.plugged, battery.charging) {
            (true, true) => "正在充电",
            (true, false) => "已接通电源",
            (false, _) => "正在使用电池",
        };
        // An unknown charge is left out rather than spelled out.
        menu.label(match battery.percent {
            Some(percent) => format!("电池 {percent}% · {source}"),
            None => format!("电池 · {source}"),
        });
        menu.separator();
    }
    if !plans.is_empty() {
        menu.label("电源计划");
        for plan in plans {
            choice(
                &mut menu,
                plan.name.clone(),
                plan_symbol(plan.kind()),
                plan.active,
                Some(SystemCommand::SetPlan(plan.id)),
            );
        }
        menu.separator();
    }
    if battery.is_some() {
        menu.item(
            "电池设置",
            Icon::Symbol(Symbol::Settings),
            Some(Command::Open("ms-settings:batterysaver")),
        );
    }
    menu.item(
        "电源和睡眠设置",
        Icon::Symbol(Symbol::Settings),
        Some(Command::Open("ms-settings:powersleep")),
    );
    menu
}

/// A leaf for saving energy, a gauge whose needle rises from quiet through balanced to
/// performance, a rocket for the top plan, and the bolt for a plan of unknown purpose.
fn plan_symbol(kind: PlanKind) -> Symbol {
    match kind {
        PlanKind::Saver => Symbol::Leaf,
        PlanKind::Quiet => Symbol::Gauge(20),
        PlanKind::Balanced => Symbol::Gauge(50),
        PlanKind::Performance => Symbol::Gauge(80),
        PlanKind::Ultimate => Symbol::Rocket,
        PlanKind::Custom => Symbol::Bolt,
    }
}

/// Brightness, dark mode, Symbol::Hdr and the display settings.
pub fn display_flyout(display: &Display) -> Menu<Command> {
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    menu.label("显示器");
    // Monitors without DDC/CI get no slider instead of a dead one.
    if display.brightness.is_some() {
        brightness_slider(&mut menu, display);
        menu.separator();
    }
    choice(
        &mut menu,
        "深色模式",
        Symbol::Contrast,
        display.dark,
        Some(SystemCommand::SetDark(!display.dark)),
    );
    if let Some(on) = display.hdr {
        choice(
            &mut menu,
            "Symbol::Hdr",
            Symbol::Hdr,
            on,
            Some(SystemCommand::SetHdr(!on)),
        );
    }
    menu.separator();
    menu.item(
        "显示设置",
        Icon::Symbol(Symbol::Settings),
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
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    menu.label("网络");
    for (kind, label, symbol, on, connected) in [
        (
            RadioKind::WiFi,
            "Wi-Fi",
            Symbol::Wifi,
            network.wifi,
            network.ssid.as_deref(),
        ),
        (
            RadioKind::Bluetooth,
            "蓝牙",
            Symbol::Bluetooth,
            network.bluetooth,
            None,
        ),
    ] {
        // A radio this computer lacks is not listed; its settings link stays below.
        let Some(on) = on else {
            continue;
        };
        menu.item(
            label,
            Icon::Choice { symbol, on },
            Some(Command::System(SystemCommand::SetRadio(kind, !on))),
        )
        .detail = radio_detail(Some(on), connected);
    }
    if network.wired {
        menu.item(
            "以太网",
            Icon::Choice {
                symbol: Symbol::Ethernet,
                on: true,
            },
            None,
        )
        .detail = "已连接".into();
    }
    menu.separator();
    menu.item(
        "网络和 Internet 设置",
        Icon::Symbol(Symbol::Settings),
        Some(Command::Open("ms-settings:network")),
    );
    menu.item(
        "蓝牙和其他设备",
        Icon::Symbol(Symbol::Settings),
        Some(Command::Open("ms-settings:bluetooth")),
    );
    menu
}

fn radio_tile(
    kind: RadioKind,
    title: &str,
    symbol: Symbol,
    on: Option<bool>,
    connected: Option<&str>,
) -> Tile<Command> {
    Tile {
        symbol,
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
    let mut menu = Menu::fixed(DROPDOWN_WIDTH);
    menu.entries.push(Entry::Tiles(vec![
        radio_tile(
            RadioKind::WiFi,
            "Wi-Fi",
            Symbol::Wifi,
            network.wifi,
            network.ssid.as_deref(),
        ),
        radio_tile(
            RadioKind::Bluetooth,
            "蓝牙",
            Symbol::Bluetooth,
            network.bluetooth,
            None,
        ),
    ]));
    menu.entries.push(Entry::Tiles(vec![
        Tile {
            symbol: Symbol::Contrast,
            title: "深色".into(),
            detail: if display.dark { "打开" } else { "关闭" }.into(),
            on: display.dark,
            command: Some(Command::System(SystemCommand::SetDark(!display.dark))),
        },
        Tile {
            symbol: Symbol::Moon,
            title: "专注".into(),
            detail: "设置".into(),
            on: false,
            command: Some(Command::Open("ms-settings:quiethours")),
        },
        Tile {
            symbol: Symbol::Cast,
            title: "投影".into(),
            detail: "选择模式".into(),
            on: false,
            command: Some(Command::Open("ms-settings-displays-topology:projection")),
        },
    ]));
    // Each slider carries its own icon, so the rows need no headings, and a monitor
    // without DDC/CI gets no brightness row at all.
    if display.brightness.is_some() {
        brightness_slider(&mut menu, display);
    }
    add_volume(&mut menu, status.audio.as_ref());
    // The player appears only while something plays; the flyout is rebuilt when the
    // media query answers.
    let control = |symbol, action| (symbol, Some(Command::System(SystemCommand::Media(action))));
    if let Some(media) = media {
        menu.entries.push(Entry::Player {
            title: if media.title.is_empty() {
                "正在播放".into()
            } else {
                media.title.clone()
            },
            detail: media.artist.clone(),
            buttons: vec![
                control(Symbol::Previous, MediaAction::Previous),
                control(
                    if media.playing {
                        Symbol::Pause
                    } else {
                        Symbol::Play
                    },
                    MediaAction::TogglePlay,
                ),
                control(Symbol::Next, MediaAction::Next),
            ],
        });
    }
    menu
}

/// The music note the player shows beside the title.
pub const PLAYER_SYMBOL: Symbol = Symbol::Music;

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

    fn count(menu: &Menu<Command>, kind: fn(&Entry<Command>) -> bool) -> usize {
        menu.entries.iter().filter(|e| kind(e)).count()
    }

    #[test]
    fn status_marks_follow_the_state() {
        let mut status = SystemStatus::default();
        let mut settings = Settings::default();
        assert!(!status.items(&settings).contains(&SystemItem::Battery));
        settings.set(Key::ShowInputMethodIcon, 0);
        settings.set(Key::ShowSearchIcon, 1);
        settings.set(Key::ShowInputLanguageIcon, 1);
        let items = status.items(&settings);
        assert!(items.contains(&SystemItem::Search));
        assert!(!items.contains(&SystemItem::InputMethod));
        assert!(!items.contains(&SystemItem::InputLanguage));
        status.input_language = Some("中");
        assert!(status.items(&settings).contains(&SystemItem::InputLanguage));
        assert_eq!(status.mark(SystemItem::InputLanguage), Mark::Text("中"));
        settings.set(Key::ShowInputLanguageIcon, 0);
        settings.set(Key::ShowSearchIcon, 0);
        settings.set(Key::ShowInputMethodIcon, 1);
        let items = status.items(&settings);
        assert!(items.contains(&SystemItem::InputMethod));
        assert!(!items.contains(&SystemItem::InputLanguage));
        assert!(!items.contains(&SystemItem::Search));
        assert_eq!(
            status.mark(SystemItem::Network),
            Mark::Icon {
                symbol: Symbol::Wifi,
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
            Mark::Icon {
                symbol: Symbol::Cellular,
                dim: false
            }
        );
        status.network.ssid = Some("home".into());
        let items = status.items(&settings);
        assert_eq!(items.last(), Some(&SystemItem::Control));
        assert!(items.contains(&SystemItem::Battery));
        assert_eq!(
            status.mark(SystemItem::Battery),
            Mark::Battery {
                symbol: Symbol::Battery {
                    percent: Some(64),
                    charging: false
                },
                percent: Some(64)
            }
        );
        assert_eq!(volume_symbol(80, true), Symbol::Muted);
        assert_eq!(volume_symbol(0, false), Symbol::Volume(0));
        assert_eq!(volume_symbol(100, false), Symbol::Volume(3));
        let full = Battery {
            percent: Some(100),
            plugged: true,
            charging: false,
        };
        assert_eq!(
            battery_symbol(full),
            Symbol::Battery {
                percent: Some(100),
                charging: true
            }
        );
    }

    #[test]
    fn wheel_steps_volume_and_only_unmutes_upward() {
        let audio = |volume, muted| Audio {
            volume,
            muted,
            outputs: Vec::new(),
        };
        assert_eq!(wheel_volume(&audio(40, false), 1), Some(42));
        assert_eq!(wheel_volume(&audio(40, false), -3), Some(34));
        assert_eq!(wheel_volume(&audio(99, false), 2), Some(100));
        assert_eq!(wheel_volume(&audio(100, false), 1), None);
        assert_eq!(wheel_volume(&audio(0, false), -1), None);
        assert_eq!(wheel_volume(&audio(40, false), 0), None);
        assert_eq!(wheel_volume(&audio(40, true), -1), None);
        assert_eq!(wheel_volume(&audio(100, true), 1), Some(100));
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
        let slider: fn(&Entry<Command>) -> bool = |e| matches!(e, Entry::Slider(_));
        let player: fn(&Entry<Command>) -> bool = |e| matches!(e, Entry::Player { .. });
        // A monitor without DDC/CI shows no brightness slider at all.
        assert_eq!(count(&display_flyout(&display), slider), 0);

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
        // Nothing playing and no brightness control: no placeholder rows either.
        let flyout = control_flyout(&status, &display, None);
        assert_eq!(count(&flyout, player), 0);
        assert_eq!(count(&flyout, slider), 1);

        // The network flyout lists only the radios this computer has.
        let network = commands(&network_flyout(&status.network));
        let bluetooth = Command::System(SystemCommand::SetRadio(RadioKind::Bluetooth, true));
        assert!(!network.contains(&bluetooth));
        assert!(network.contains(&Command::Open("ms-settings:bluetooth")));
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
                Focus::Slider(2),
                Focus::Part(3, 0),
                Focus::Slider(3),
                Focus::Part(4, 0),
                Focus::Part(4, 1),
                Focus::Part(4, 2),
            ]
        );
        assert_eq!(menu.step(Some(ordered[0]), false), ordered.last().copied());
    }
}
