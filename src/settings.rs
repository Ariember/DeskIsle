//! User settings: one declarative [`SCHEMA`] row per setting drives validation, the
//! settings file, the settings panel and resetting to defaults. Adding a setting means
//! adding a [`Key`] variant and its row, then reacting to the change in the app layer.

/// Identifies one setting. Callers read values through the typed accessors on
/// [`Settings`]; the panel and the settings file go through [`SCHEMA`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    IconSize,
    DockGap,
    HoverZoom,
    HideTaskbar,
    Autostart,
}

/// A page of the settings panel, in navigation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Dock,
    System,
}

pub const SECTIONS: &[Section] = &[Section::Dock, Section::System];

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Dock => "Dock 外观",
            Section::System => "系统行为",
        }
    }

    /// A Segoe Fluent Icons code point for the navigation list.
    pub fn glyph(self) -> &'static str {
        match self {
            Section::Dock => "\u{E771}",
            Section::System => "\u{E770}",
        }
    }
}

/// How the user changes a value and which values are valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// `0` is off and `1` is on.
    Toggle,
    /// Whole numbers from `min` to `max` in increments of `step`.
    Stepper {
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

pub const SCHEMA: &[Spec] = &[
    Spec {
        key: Key::IconSize,
        id: "dock.icon_size",
        section: Section::Dock,
        label: "图标大小",
        help: "Dock 图标的边长，按显示器缩放换算",
        control: Control::Stepper {
            min: 24,
            max: 56,
            step: 2,
            unit: "px",
        },
        default: 34,
        store: Store::File,
    },
    Spec {
        key: Key::DockGap,
        id: "dock.bottom_gap",
        section: Section::Dock,
        label: "底部间距",
        help: "Dock 与工作区底边的距离",
        control: Control::Stepper {
            min: 0,
            max: 32,
            step: 2,
            unit: "px",
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
        key: Key::HideTaskbar,
        id: "system.hide_taskbar",
        section: Section::System,
        label: "隐藏系统任务栏",
        help: "DeskIsle 运行时隐藏主屏的 Windows 任务栏，退出后恢复",
        control: Control::Toggle,
        default: 1,
        store: Store::File,
    },
    Spec {
        key: Key::Autostart,
        id: "system.autostart",
        section: Section::System,
        label: "开机自动启动",
        help: "登录 Windows 后启动 DeskIsle",
        control: Control::Toggle,
        default: 0,
        store: Store::System,
    },
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
}

impl Spec {
    /// The nearest valid value, or `None` when `value` cannot belong to this setting.
    fn valid(&self, value: i32) -> Option<i32> {
        match self.control {
            Control::Toggle => (0..=1).contains(&value).then_some(value),
            Control::Stepper { min, max, step, .. } => {
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
            Control::Stepper { min, max, .. } => {
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
            Control::Stepper { .. } => value.to_string(),
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
            (Control::Stepper { step, .. }, Adjust::Step(n)) => {
                value.saturating_add(step.saturating_mul(n))
            }
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

    /// The Dock icon edge in DIPs.
    pub fn icon_size(&self) -> i32 {
        self.get(Key::IconSize)
    }

    /// Distance between the Dock and the bottom of the work area in DIPs.
    pub fn dock_gap(&self) -> i32 {
        self.get(Key::DockGap)
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
        assert_eq!(settings.set_text("future.option", "x"), Ok(()));
        assert!(settings.set_text("dock.bottom_gap", "999").is_err());
        assert!(settings.set_text("system.hide_taskbar", "yes").is_err());
        assert_eq!(settings.set_text("system.autostart", "true"), Ok(()));
        assert_eq!(settings.icon_size(), 40);
        assert_eq!(settings.dock_gap(), 8);
        assert!(!settings.enabled(Key::Autostart));

        // Odd values snap down to the stepper's grid.
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
}
