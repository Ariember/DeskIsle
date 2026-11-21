//! User settings: one declarative [`SCHEMA`] row per setting drives validation, the
//! settings file, the settings panel and resetting to defaults. Adding a setting means
//! adding a [`Key`] variant and its row, then reacting to the change in the app layer.

use crate::metrics::{Metric, Sources, Style};
use crate::visibility::DockDisplayMode;

/// Identifies one setting. Callers read values through the typed accessors on
/// [`Settings`]; the panel and the settings file go through [`SCHEMA`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    InterfaceScale,
    Backdrop,
    Theme,
    SyncSystemTheme,
    Accent,
    TaskbarShortcut,
    RecentFiles,
    Language,
    TemperatureUnit,
    FontFamily,
    FontScale,
    BoldFont,
    SmoothText,
    IconSpacing,
    DockDisplayMode,
    DockRevealDelay,
    IconSize,
    DockGap,
    HoverZoom,
    HideTaskbar,
    Autostart,
    TopBarBackground,
    TopBarHeight,
    ConfirmPowerActions,
    TopBarImmersive,
    TopBarCloseMaximized,
    LockTrayIcons,
    ColorTrayIcons,
    ShowInputMethodIcon,
    ShowInputLanguageIcon,
    ShowSearchIcon,
    Clock(ClockKey),
    Monitor(MonitorKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockKey {
    Format,
    Date,
    Weekday,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorKey {
    Highlight,
    Cpu,
    CpuStyle,
    Temperature,
    GpuTemperature,
    Memory,
    MemoryStyle,
    Disk,
    DiskStyle,
}

/// A page of the settings panel, in navigation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Dock,
    TopBar,
    Clock,
    Monitor,
}

pub const SECTIONS: &[Section] = &[
    Section::General,
    Section::Dock,
    Section::TopBar,
    Section::Clock,
    Section::Monitor,
];

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::General => "常规",
            Section::Dock => "Dock",
            Section::TopBar => "顶部栏",
            Section::Clock => "时间",
            Section::Monitor => "监控",
        }
    }
}

/// How the user changes a value and which values are valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// `0` is off and `1` is on.
    Toggle,
    /// Whole numbers from `min` to `max` in increments of `step`.
    Slider {
        min: i32,
        max: i32,
        step: i32,
        unit: &'static str,
    },
    /// The value is an index into the options, stored by their `id`.
    Choice(&'static [Choice]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: &'static str,
    pub label: &'static str,
}

/// Where the value lives between runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    /// DeskIsle's settings file.
    File,
    /// The system keeps the value; the app layer reads it at startup and writes it on change.
    System,
    /// UI preview for this run of DeskIsle.
    Session,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub key: Key,
    /// The stable name in the settings file.
    pub id: &'static str,
    pub section: Section,
    pub label: &'static str,
    pub help: &'static str,
    pub control: Control,
    pub default: i32,
    pub store: Store,
}

pub const HOVER_ZOOM: &[Choice] = &[
    Choice {
        id: "off",
        label: "关闭",
    },
    Choice {
        id: "subtle",
        label: "柔和",
    },
    Choice {
        id: "standard",
        label: "标准",
    },
];

pub const DOCK_DISPLAY_MODES: &[Choice] = &[
    Choice {
        id: "default",
        label: "默认显示模式",
    },
    Choice {
        id: "overlay",
        label: "叠层显示模式",
    },
    Choice {
        id: "auto_hide",
        label: "自动隐藏模式",
    },
    Choice {
        id: "smart_hide",
        label: "智能隐藏模式",
    },
    Choice {
        id: "always",
        label: "始终显示模式",
    },
    Choice {
        id: "hotkey",
        label: "热键显示模式",
    },
    Choice {
        id: "always_including_fullscreen",
        label: "始终显示包括全屏",
    },
    Choice {
        id: "desktop",
        label: "桌面显示模式",
    },
];

/// The colors DeskIsle draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    /// Follows Windows' app mode and switches when it changes.
    System,
    Light,
    Dark,
    Automatic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopBarBackground {
    Tinted,
    Frosted,
    Transparent,
}

impl TopBarBackground {
    pub fn from_index(index: i32) -> Self {
        [Self::Tinted, Self::Frosted, Self::Transparent][index as usize]
    }
}

pub const THEMES: &[Choice] = &[
    Choice {
        id: "system",
        label: "跟随系统",
    },
    Choice {
        id: "light",
        label: "浅色",
    },
    Choice {
        id: "dark",
        label: "深色",
    },
    Choice {
        id: "auto",
        label: "自动",
    },
];

const CLOCK_FORMATS: &[Choice] = &[
    Choice {
        id: "24h",
        label: "24 小时",
    },
    Choice {
        id: "12h",
        label: "12 小时",
    },
];

const MONITOR_STYLES: &[Choice] = &[
    Choice {
        id: "bar",
        label: "条形",
    },
    Choice {
        id: "ring",
        label: "环形",
    },
    Choice {
        id: "graph",
        label: "曲线",
    },
    Choice {
        id: "number",
        label: "数值",
    },
];

const fn clock_preview(
    key: ClockKey,
    id: &'static str,
    label: &'static str,
    help: &'static str,
    control: Control,
    default: i32,
) -> Spec {
    Spec {
        key: Key::Clock(key),
        id,
        section: Section::Clock,
        label,
        help,
        control,
        default,
        store: Store::Session,
    }
}

const fn monitor(
    key: MonitorKey,
    id: &'static str,
    label: &'static str,
    help: &'static str,
    control: Control,
    default: i32,
) -> Spec {
    Spec {
        key: Key::Monitor(key),
        id,
        section: Section::Monitor,
        label,
        help,
        control,
        default,
        store: Store::File,
    }
}

const fn top_bar_toggle(
    key: Key,
    id: &'static str,
    label: &'static str,
    help: &'static str,
    default: i32,
) -> Spec {
    Spec {
        key,
        id,
        section: Section::TopBar,
        label,
        help,
        control: Control::Toggle,
        default,
        store: Store::File,
    }
}

pub const SCHEMA: &[Spec] = &[
    Spec {
        key: Key::InterfaceScale,
        id: "general.interface_scale",
        section: Section::General,
        label: "界面显示大小",
        help: "在显示器缩放基础上调整 DeskIsle 界面",
        control: Control::Slider {
            min: 80,
            max: 150,
            step: 5,
            unit: "%",
        },
        default: 100,
        store: Store::File,
    },
    Spec {
        key: Key::Backdrop,
        id: "general.backdrop",
        section: Section::General,
        label: "毛玻璃效果强度",
        help: "调整设置侧栏和关于本机的背景透视效果",
        control: Control::Slider {
            min: 0,
            max: 100,
            step: 5,
            unit: "%",
        },
        default: 100,
        store: Store::File,
    },
    Spec {
        key: Key::Theme,
        id: "appearance.theme",
        section: Section::General,
        label: "界面颜色模式",
        help: "自动模式在 18:00 至次日 06:00 使用深色",
        control: Control::Choice(THEMES),
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::SyncSystemTheme,
        id: "general.sync_system_theme",
        section: Section::General,
        label: "同步 Windows 颜色模式",
        help: "选择浅色、深色或自动时同步 Windows 和应用颜色",
        control: Control::Toggle,
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::Accent,
        id: "general.accent",
        section: Section::General,
        label: "界面强调色",
        help: "用于选中项、开关和滑块",
        control: Control::Choice(&[
            Choice {
                id: "blue",
                label: "蓝",
            },
            Choice {
                id: "purple",
                label: "紫",
            },
            Choice {
                id: "pink",
                label: "粉",
            },
            Choice {
                id: "red",
                label: "红",
            },
            Choice {
                id: "orange",
                label: "橙",
            },
            Choice {
                id: "green",
                label: "绿",
            },
            Choice {
                id: "gray",
                label: "灰",
            },
        ]),
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::Autostart,
        id: "system.autostart",
        section: Section::General,
        label: "开机自动启动",
        help: "登录 Windows 后启动 DeskIsle",
        control: Control::Toggle,
        default: 0,
        store: Store::System,
    },
    Spec {
        key: Key::HideTaskbar,
        id: "system.hide_taskbar",
        section: Section::General,
        label: "隐藏系统任务栏",
        help: "运行时隐藏主屏任务栏，退出后恢复",
        control: Control::Toggle,
        default: 1,
        store: Store::File,
    },
    Spec {
        key: Key::TaskbarShortcut,
        id: "system.taskbar_shortcut",
        section: Section::General,
        label: "显示或隐藏任务栏快捷键",
        help: "在任何应用中切换主屏任务栏",
        control: Control::Choice(&[
            Choice {
                id: "none",
                label: "关闭",
            },
            Choice {
                id: "ctrl_alt_t",
                label: "Ctrl+Alt+T",
            },
            Choice {
                id: "ctrl_shift_f11",
                label: "Ctrl+Shift+F11",
            },
        ]),
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::RecentFiles,
        id: "general.recent_files",
        section: Section::General,
        label: "显示程序最近打开文件个数",
        help: "在支持 Windows 最近文档列表的应用菜单中显示",
        control: Control::Slider {
            min: 0,
            max: 5,
            step: 1,
            unit: "个",
        },
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::TemperatureUnit,
        id: "general.temperature_unit",
        section: Section::General,
        label: "温度单位",
        help: "顶部栏的 CPU 和 GPU 温度",
        control: Control::Choice(&[
            Choice {
                id: "celsius",
                label: "℃",
            },
            Choice {
                id: "fahrenheit",
                label: "℉",
            },
        ]),
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::Language,
        id: "general.panel_language",
        section: Section::General,
        label: "控制面板语言",
        help: "设置面板的显示语言",
        control: Control::Choice(&[
            Choice {
                id: "zh-CN",
                label: "简体中文",
            },
            Choice {
                id: "zh-TW",
                label: "繁體中文",
            },
        ]),
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::FontFamily,
        id: "general.font_family",
        section: Section::General,
        label: "界面字体",
        help: "用于 DeskIsle 的文字",
        control: Control::Choice(&[
            Choice {
                id: "segoe",
                label: "Segoe UI",
            },
            Choice {
                id: "yahei",
                label: "Microsoft YaHei UI",
            },
            Choice {
                id: "dengxian",
                label: "等线",
            },
        ]),
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::BoldFont,
        id: "general.bold_font",
        section: Section::General,
        label: "使用加粗字体",
        help: "加粗界面正文",
        control: Control::Toggle,
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::SmoothText,
        id: "general.smooth_text",
        section: Section::General,
        label: "高质量渲染字体",
        help: "平滑文字边缘",
        control: Control::Toggle,
        default: 1,
        store: Store::File,
    },
    Spec {
        key: Key::FontScale,
        id: "general.font_scale",
        section: Section::General,
        label: "字体缩放",
        help: "调整界面中的文字大小",
        control: Control::Slider {
            min: 80,
            max: 120,
            step: 5,
            unit: "%",
        },
        default: 100,
        store: Store::File,
    },
    Spec {
        key: Key::IconSize,
        id: "dock.icon_size",
        section: Section::Dock,
        label: "图标大小",
        help: "图标边长，随显示器缩放",
        control: Control::Slider {
            min: 24,
            max: 56,
            step: 2,
            unit: "DIP",
        },
        default: 34,
        store: Store::File,
    },
    Spec {
        key: Key::IconSpacing,
        id: "dock.icon_spacing",
        section: Section::Dock,
        label: "图标之间的距离",
        help: "相邻 Dock 图标之间的留白",
        control: Control::Slider {
            min: 0,
            max: 40,
            step: 1,
            unit: "DIP",
        },
        default: 18,
        store: Store::File,
    },
    Spec {
        key: Key::DockGap,
        id: "dock.bottom_gap",
        section: Section::Dock,
        label: "Dock 距离屏幕边缘",
        help: "Dock 与工作区底边的距离",
        control: Control::Slider {
            min: 0,
            max: 32,
            step: 2,
            unit: "DIP",
        },
        default: 8,
        store: Store::File,
    },
    Spec {
        key: Key::HoverZoom,
        id: "dock.hover_zoom",
        section: Section::Dock,
        label: "悬停放大",
        help: "指针经过时图标放大的幅度",
        control: Control::Choice(HOVER_ZOOM),
        default: 2,
        store: Store::File,
    },
    Spec {
        key: Key::DockDisplayMode,
        id: "dock.display_mode",
        section: Section::Dock,
        label: "Dock 显示隐藏模式",
        help: "全屏应用在前台时隐藏",
        control: Control::Choice(DOCK_DISPLAY_MODES),
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::DockRevealDelay,
        id: "dock.reveal_delay",
        section: Section::Dock,
        label: "底边唤出延迟",
        help: "指针停留在底部唤出区域后的等待时间",
        control: Control::Slider {
            min: 0,
            max: 1000,
            step: 50,
            unit: "ms",
        },
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::TopBarBackground,
        id: "topbar.background",
        section: Section::TopBar,
        label: "顶部栏背景样式",
        help: "选择顶部栏在桌面上的背景效果",
        control: Control::Choice(&[
            Choice {
                id: "tinted",
                label: "半透明",
            },
            Choice {
                id: "frosted",
                label: "毛玻璃",
            },
            Choice {
                id: "transparent",
                label: "透明",
            },
        ]),
        default: 0,
        store: Store::File,
    },
    Spec {
        key: Key::TopBarHeight,
        id: "topbar.height",
        section: Section::TopBar,
        label: "顶部栏高度",
        help: "调整顶部栏及其在主屏预留的空间",
        control: Control::Slider {
            min: 24,
            max: 48,
            step: 1,
            unit: "DIP",
        },
        default: 30,
        store: Store::File,
    },
    top_bar_toggle(
        Key::ConfirmPowerActions,
        "topbar.confirm_power",
        "关机或重启前确认",
        "从顶部栏选择关机或重启时显示确认对话框",
        0,
    ),
    top_bar_toggle(
        Key::TopBarImmersive,
        "topbar.immersive",
        "窗口最大化时使用沉浸模式",
        "顶部栏背景跟随主屏前台最大化窗口",
        1,
    ),
    top_bar_toggle(
        Key::TopBarCloseMaximized,
        "topbar.close_maximized",
        "点击右上角关闭最大化窗口",
        "在顶部栏右端显示当前最大化窗口的关闭按钮",
        0,
    ),
    top_bar_toggle(
        Key::LockTrayIcons,
        "topbar.lock_icons",
        "锁定托盘图标位置",
        "关闭时可左右拖动顶部栏的托盘图标调整顺序",
        0,
    ),
    top_bar_toggle(
        Key::ColorTrayIcons,
        "topbar.color_tray_icons",
        "显示彩色托盘图标",
        "保留应用托盘图标的原色",
        0,
    ),
    top_bar_toggle(
        Key::ShowInputMethodIcon,
        "topbar.show_input_method",
        "显示输入法图标",
        "在顶部栏显示输入设置入口",
        1,
    ),
    top_bar_toggle(
        Key::ShowInputLanguageIcon,
        "topbar.show_input_language",
        "显示输入法中英文图标",
        "在顶部栏显示当前输入模式",
        0,
    ),
    top_bar_toggle(
        Key::ShowSearchIcon,
        "topbar.show_search",
        "显示搜索图标",
        "点击搜索图标，在启动台中查找应用",
        1,
    ),
    clock_preview(
        ClockKey::Format,
        "clock.format",
        "时间格式",
        "顶部栏时钟的小时制",
        Control::Choice(CLOCK_FORMATS),
        0,
    ),
    clock_preview(
        ClockKey::Date,
        "clock.date",
        "显示日期",
        "在时间旁显示月份和日期",
        Control::Toggle,
        1,
    ),
    clock_preview(
        ClockKey::Weekday,
        "clock.weekday",
        "显示星期",
        "在日期旁显示星期几",
        Control::Toggle,
        0,
    ),
    monitor(
        MonitorKey::Highlight,
        "monitor.highlight",
        "高占用提醒",
        "CPU、内存或磁盘占用达到 90% 时高亮显示",
        Control::Toggle,
        0,
    ),
    monitor(
        MonitorKey::Cpu,
        "monitor.cpu",
        "CPU",
        "",
        Control::Toggle,
        1,
    ),
    monitor(
        MonitorKey::CpuStyle,
        "monitor.cpu_style",
        "显示样式",
        "",
        Control::Choice(MONITOR_STYLES),
        3,
    ),
    monitor(
        MonitorKey::Temperature,
        "monitor.temperature",
        "核心温度",
        "",
        Control::Toggle,
        0,
    ),
    monitor(
        MonitorKey::GpuTemperature,
        "monitor.gpu_temperature",
        "GPU",
        "",
        Control::Toggle,
        0,
    ),
    monitor(
        MonitorKey::Memory,
        "monitor.memory",
        "内存",
        "",
        Control::Toggle,
        1,
    ),
    monitor(
        MonitorKey::MemoryStyle,
        "monitor.memory_style",
        "显示样式",
        "",
        Control::Choice(MONITOR_STYLES),
        0,
    ),
    monitor(
        MonitorKey::Disk,
        "monitor.disk",
        "磁盘",
        "",
        Control::Toggle,
        0,
    ),
    monitor(
        MonitorKey::DiskStyle,
        "monitor.disk_style",
        "显示样式",
        "",
        Control::Choice(MONITOR_STYLES),
        2,
    ),
];

pub fn spec(key: Key) -> &'static Spec {
    &SCHEMA[position(key)]
}

fn position(key: Key) -> usize {
    SCHEMA
        .iter()
        .position(|spec| spec.key == key)
        .expect("every key has a schema row")
}

/// A user's change to one value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjust {
    /// Flips a toggle or moves a choice to the next option, wrapping around.
    Toggle,
    /// Moves by `n` increments without wrapping; a toggle turns on for a positive `n`.
    Step(i32),
    /// Selects option `n` of a choice.
    Pick(usize),
    /// Commits a slider position.
    Set(i32),
}

impl Spec {
    /// The nearest valid value, or `None` when `value` cannot belong to this setting.
    fn valid(&self, value: i32) -> Option<i32> {
        match self.control {
            Control::Toggle => (0..=1).contains(&value).then_some(value),
            Control::Slider { min, max, step, .. } => {
                let value = value.clamp(min, max);
                Some(min + (value - min) / step * step)
            }
            Control::Choice(options) => (0..options.len() as i32).contains(&value).then_some(value),
        }
    }

    pub fn parse(&self, text: &str) -> Option<i32> {
        let value = match self.control {
            Control::Toggle => match text {
                "true" => 1,
                "false" => 0,
                _ => return None,
            },
            Control::Slider { min, max, .. } => {
                let value: i32 = text.parse().ok()?;
                if !(min..=max).contains(&value) {
                    return None;
                }
                value
            }
            Control::Choice(options) => options.iter().position(|o| o.id == text)? as i32,
        };
        self.valid(value)
    }

    pub fn format(&self, value: i32) -> String {
        match self.control {
            Control::Toggle => (value != 0).to_string(),
            Control::Slider { .. } => value.to_string(),
            Control::Choice(options) => options[value as usize].id.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Aligned with [`SCHEMA`].
    values: Vec<i32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            values: SCHEMA.iter().map(|spec| spec.default).collect(),
        }
    }
}

impl Settings {
    pub fn get(&self, key: Key) -> i32 {
        self.values[position(key)]
    }

    /// Stores the nearest valid value. Returns whether anything changed.
    pub fn set(&mut self, key: Key, value: i32) -> bool {
        let index = position(key);
        match SCHEMA[index].valid(value) {
            Some(value) if value != self.values[index] => {
                self.values[index] = value;
                true
            }
            _ => false,
        }
    }

    /// Applies a change from the panel. Returns whether the value changed.
    pub fn adjust(&mut self, key: Key, adjust: Adjust) -> bool {
        let value = self.get(key);
        let next = match (spec(key).control, adjust) {
            (Control::Toggle, Adjust::Toggle) => 1 - value,
            (Control::Toggle, Adjust::Step(n)) => i32::from(n > 0),
            (Control::Slider { step, .. }, Adjust::Step(n)) => {
                value.saturating_add(step.saturating_mul(n))
            }
            (Control::Slider { .. }, Adjust::Set(n)) => n,
            (Control::Choice(options), Adjust::Toggle) => (value + 1) % options.len() as i32,
            (Control::Choice(options), Adjust::Step(n)) => {
                value.saturating_add(n).clamp(0, options.len() as i32 - 1)
            }
            (Control::Choice(_), Adjust::Pick(n)) => n as i32,
            _ => return false,
        };
        self.set(key, next)
    }

    /// Reads a value saved as text, keeping the current one when the text is invalid.
    pub fn set_text(&mut self, id: &str, text: &str) -> Result<(), String> {
        let Some(spec) = SCHEMA.iter().find(|spec| spec.id == id) else {
            // A newer DeskIsle may have written it.
            return Ok(());
        };
        if spec.store != Store::File {
            return Ok(());
        }
        let value = spec
            .parse(text)
            .ok_or_else(|| format!("{}（{id}）的值“{text}”无效，已使用默认值", spec.label))?;
        self.set(spec.key, value);
        Ok(())
    }

    /// Values kept in the settings file, as `(id, text)` pairs in schema order.
    pub fn file_entries(&self) -> impl Iterator<Item = (&'static str, String)> + '_ {
        SCHEMA
            .iter()
            .zip(&self.values)
            .filter(|(spec, _)| spec.store == Store::File)
            .map(|(spec, &value)| (spec.id, spec.format(value)))
    }

    /// Settings whose values differ between `self` and `other`.
    pub fn changed<'a>(&'a self, other: &'a Settings) -> impl Iterator<Item = &'static Spec> + 'a {
        SCHEMA
            .iter()
            .zip(self.values.iter().zip(&other.values))
            .filter(|(_, (a, b))| a != b)
            .map(|(spec, _)| spec)
    }

    pub fn enabled(&self, key: Key) -> bool {
        self.get(key) != 0
    }

    pub fn sources(&self) -> Sources {
        let temperature = self.enabled(Key::Monitor(MonitorKey::Temperature));
        Sources {
            cpu: self.enabled(Key::Monitor(MonitorKey::Cpu)),
            memory: self.enabled(Key::Monitor(MonitorKey::Memory)),
            disk: self.enabled(Key::Monitor(MonitorKey::Disk)),
            cpu_temperature: temperature,
            gpu_temperature: temperature && self.enabled(Key::Monitor(MonitorKey::GpuTemperature)),
        }
    }

    pub fn monitor_style(&self, metric: Metric) -> Style {
        let key = match metric {
            Metric::Cpu => MonitorKey::CpuStyle,
            Metric::Memory => MonitorKey::MemoryStyle,
            Metric::Disk => MonitorKey::DiskStyle,
            Metric::CpuTemperature | Metric::GpuTemperature => return Style::Value,
        };
        [Style::Bars, Style::Ring, Style::Graph, Style::Value][self.get(Key::Monitor(key)) as usize]
    }

    pub fn monitor_highlight(&self) -> bool {
        self.enabled(Key::Monitor(MonitorKey::Highlight))
    }

    /// Interface pixels per 96 DIPs after applying the user's scale to Windows' DPI.
    pub fn ui_dpi(&self, monitor_dpi: u32) -> u32 {
        (monitor_dpi * self.get(Key::InterfaceScale) as u32 + 50) / 100
    }

    /// The Dock icon edge in DIPs.
    pub fn icon_size(&self) -> i32 {
        self.get(Key::IconSize)
    }

    /// Distance between the Dock and the bottom of the work area in DIPs.
    pub fn dock_gap(&self) -> i32 {
        self.get(Key::DockGap)
    }

    pub fn dock_display_mode(&self) -> DockDisplayMode {
        use DockDisplayMode::*;
        [
            Default,
            Overlay,
            AutoHide,
            SmartHide,
            Always,
            Hotkey,
            AlwaysIncludingFullscreen,
            Desktop,
        ][self.get(Key::DockDisplayMode) as usize]
    }

    pub fn theme(&self) -> Theme {
        [Theme::System, Theme::Light, Theme::Dark, Theme::Automatic][self.get(Key::Theme) as usize]
    }

    pub fn top_bar_background(&self) -> TopBarBackground {
        TopBarBackground::from_index(self.get(Key::TopBarBackground))
    }

    /// The scale of a hovered Dock icon.
    pub fn hover_scale(&self) -> f32 {
        [1.0, 1.12, 1.24][self.get(Key::HoverZoom) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_ids_are_unique_and_defaults_valid() {
        for (index, spec) in SCHEMA.iter().enumerate() {
            assert!(
                SCHEMA[..index]
                    .iter()
                    .all(|s| s.id != spec.id && s.key != spec.key)
            );
            assert_eq!(spec.valid(spec.default), Some(spec.default), "{}", spec.id);
            assert!(SECTIONS.contains(&spec.section));
            assert_eq!(spec.parse(&spec.format(spec.default)), Some(spec.default));
        }
    }

    #[test]
    fn adjustments_stay_within_each_control() {
        let mut settings = Settings::default();
        assert!(settings.adjust(Key::IconSize, Adjust::Step(1)));
        assert_eq!(settings.icon_size(), 36);
        assert!(settings.adjust(Key::IconSize, Adjust::Step(100)));
        assert_eq!(settings.icon_size(), 56);
        assert!(!settings.adjust(Key::IconSize, Adjust::Step(1)));
        assert!(!settings.adjust(Key::IconSize, Adjust::Toggle));

        assert!(settings.adjust(Key::HideTaskbar, Adjust::Toggle));
        assert!(!settings.enabled(Key::HideTaskbar));
        assert!(!settings.adjust(Key::HideTaskbar, Adjust::Step(-1)));
        assert!(settings.adjust(Key::HideTaskbar, Adjust::Step(1)));

        assert!(settings.adjust(Key::HoverZoom, Adjust::Toggle));
        assert_eq!(settings.hover_scale(), 1.0);
        assert!(!settings.adjust(Key::HoverZoom, Adjust::Step(-1)));
        assert!(settings.adjust(Key::HoverZoom, Adjust::Pick(1)));
        assert!(!settings.adjust(Key::HoverZoom, Adjust::Pick(9)));
        assert_eq!(settings.hover_scale(), 1.12);

        let changed: Vec<_> = settings
            .changed(&Settings::default())
            .map(|s| s.key)
            .collect();
        assert_eq!(changed, [Key::IconSize, Key::HoverZoom]);
    }

    #[test]
    fn saved_text_is_validated_and_system_values_stay_out_of_the_file() {
        let mut settings = Settings::default();
        assert_eq!(settings.set_text("dock.icon_size", "40"), Ok(()));
        assert_eq!(settings.set_text("dock.hover_zoom", "subtle"), Ok(()));
        assert_eq!(settings.set_text("appearance.theme", "dark"), Ok(()));
        assert_eq!(settings.theme(), Theme::Dark);
        assert_eq!(settings.set_text("future.option", "x"), Ok(()));
        assert!(settings.set_text("dock.bottom_gap", "999").is_err());
        assert!(settings.set_text("system.hide_taskbar", "yes").is_err());
        assert_eq!(settings.set_text("system.autostart", "true"), Ok(()));
        assert_eq!(settings.icon_size(), 40);
        assert_eq!(settings.dock_gap(), 8);
        assert!(!settings.enabled(Key::Autostart));

        // Odd values snap down to the slider's grid.
        settings.set(Key::IconSize, 41);
        assert_eq!(settings.icon_size(), 40);

        let entries: Vec<_> = settings.file_entries().collect();
        assert!(entries.iter().all(|(id, _)| *id != "system.autostart"));
        let mut read = Settings::default();
        for (id, text) in &entries {
            read.set_text(id, text).unwrap();
        }
        assert_eq!(read, settings);
    }

    #[test]
    fn slider_edits_round_trip_without_changing_legacy_settings() {
        let mut settings = Settings::default();
        settings.set_text("appearance.theme", "dark").unwrap();
        settings.set_text("dock.bottom_gap", "14").unwrap();
        assert!(settings.adjust(Key::InterfaceScale, Adjust::Set(125)));
        assert_eq!(settings.ui_dpi(144), 180);
        assert!(settings.adjust(Key::DockRevealDelay, Adjust::Set(300)));
        assert!(settings.adjust(Key::IconSpacing, Adjust::Set(6)));
        assert!(settings.adjust(Key::DockDisplayMode, Adjust::Pick(2)));
        let mut restored = Settings::default();
        for (key, value) in settings.file_entries() {
            restored.set_text(key, &value).unwrap();
        }
        assert_eq!(restored, settings);
        assert_eq!(restored.theme(), Theme::Dark);
        assert_eq!(restored.dock_gap(), 14);
        assert_eq!(restored.dock_display_mode(), DockDisplayMode::AutoHide);
    }

    #[test]
    fn top_bar_settings_load_over_legacy_defaults_and_round_trip() {
        let mut settings = Settings::default();
        settings.set_text("dock.icon_size", "42").unwrap();
        assert_eq!(settings.get(Key::TopBarHeight), 30);
        assert!(settings.enabled(Key::TopBarImmersive));
        assert!(settings.enabled(Key::ShowInputMethodIcon));
        assert!(settings.enabled(Key::ShowSearchIcon));
        for spec in SCHEMA.iter().filter(|s| s.section == Section::TopBar) {
            let adjustment = match spec.control {
                Control::Choice(_) => Adjust::Pick(2),
                Control::Slider { .. } => Adjust::Set(37),
                Control::Toggle => Adjust::Toggle,
            };
            assert!(settings.adjust(spec.key, adjustment));
        }
        let mut restored = Settings::default();
        for (id, value) in settings.file_entries() {
            restored.set_text(id, &value).unwrap();
        }
        assert_eq!(restored, settings);
        assert_eq!(restored.icon_size(), 42);
    }

    #[test]
    fn monitor_settings_round_trip_with_hidden_styles_and_gpu_selection() {
        let mut settings = Settings::default();
        assert!(settings.sources().cpu && settings.sources().memory);
        assert_eq!(settings.monitor_style(Metric::Cpu), Style::Value);
        assert_eq!(settings.monitor_style(Metric::Memory), Style::Bars);
        assert_eq!(settings.monitor_style(Metric::Disk), Style::Graph);
        assert!(!settings.sources().disk);
        settings.set_text("monitor.cpu_style", "ring").unwrap();
        settings.adjust(Key::Monitor(MonitorKey::Cpu), Adjust::Toggle);
        settings.adjust(Key::Monitor(MonitorKey::GpuTemperature), Adjust::Toggle);
        settings.adjust(Key::Monitor(MonitorKey::Highlight), Adjust::Toggle);

        let mut read = Settings::default();
        for (id, text) in settings.file_entries() {
            read.set_text(id, &text).unwrap();
        }
        assert!(!read.sources().cpu);
        assert_eq!(read.monitor_style(Metric::Cpu), Style::Ring);
        assert!(read.monitor_highlight());
        assert!(!read.sources().gpu_temperature);
        read.adjust(Key::Monitor(MonitorKey::Temperature), Adjust::Toggle);
        assert!(read.sources().cpu_temperature && read.sources().gpu_temperature);
        assert_eq!(read.monitor_style(Metric::GpuTemperature), Style::Value);
    }
}
