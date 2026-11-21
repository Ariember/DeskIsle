use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::E_FAIL;
use windows::core::{Error, HRESULT, Result};

use crate::dock::PinnedApp;
use crate::settings::{Key, Settings, spec};

const PINS_FILE: &str = "pins.txt";

const LAUNCHPAD_FILE: &str = "launchpad.txt";

pub fn load_launchpad() -> Result<Vec<PathBuf>> {
    read_launchpad(&settings_path(LAUNCHPAD_FILE)?)
}

fn read_launchpad(path: &Path) -> Result<Vec<PathBuf>> {
    let lines = read_lines(path)?.unwrap_or_default();
    let mut entries = Vec::new();
    let mut known = BTreeSet::new();
    for line in lines {
        let path = PathBuf::from(line);
        if !crate::launchpad::supported(&path) {
            return Err(Error::new(
                E_FAIL,
                format!("启动台列表包含无效路径：{}", path.display()),
            ));
        }
        if known.insert(crate::dock::path_id(&path)) {
            entries.push(path);
        }
    }
    Ok(entries)
}

pub fn save_launchpad(entries: &[PathBuf]) -> Result<()> {
    write_paths(
        &settings_path(LAUNCHPAD_FILE)?,
        entries.iter().map(PathBuf::as_path),
    )
}

fn write_paths<'a>(path: &Path, entries: impl IntoIterator<Item = &'a Path>) -> Result<()> {
    let dir = path
        .parent()
        .expect("the list file is in the settings directory");
    std::fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    let mut text = String::new();
    for entry in entries {
        text.push_str(&entry.to_string_lossy());
        text.push('\n');
    }
    // DeskIsle has a single writer per session. Replace the list only after the full
    // new contents are on disk, so a failed write leaves the previous list intact.
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| io_error(path, e))
}

/// The pinned applications the user saved, or `None` before the first change.
pub fn load_pins(make_pin: impl Fn(PathBuf) -> PinnedApp) -> Result<Option<Vec<PinnedApp>>> {
    Ok(read_setting(PINS_FILE)?.map(|lines| {
        let mut known = BTreeSet::new();
        lines
            .into_iter()
            .map(|line| make_pin(line.into()))
            .filter(|pin| known.insert(pin.app_id.clone()))
            .collect()
    }))
}

pub fn save_pins(pins: &[PinnedApp]) -> Result<()> {
    write_paths(
        &settings_path(PINS_FILE)?,
        pins.iter().map(|pin| pin.launch.as_path()),
    )
}

const HIDDEN_FILE: &str = "tray-hidden.txt";

/// Keys of tray icons the user moved off the bar. A missing file means none.
pub fn load_hidden() -> Result<BTreeSet<String>> {
    Ok(read_setting(HIDDEN_FILE)?
        .unwrap_or_default()
        .into_iter()
        .collect())
}

pub fn save_hidden(hidden: &BTreeSet<String>) -> Result<()> {
    write_setting(HIDDEN_FILE, hidden.iter().map(String::as_str))
}

const TRAY_ORDER_FILE: &str = "tray-order.txt";

pub fn load_tray_order() -> Result<Vec<String>> {
    Ok(read_setting(TRAY_ORDER_FILE)?.unwrap_or_default())
}

pub fn save_tray_order(keys: &[String]) -> Result<()> {
    write_setting(TRAY_ORDER_FILE, keys.iter().map(String::as_str))
}

const SETTINGS_FILE: &str = "settings.ini";

/// Settings the user saved, or `None` before the first change. Lines read
/// `id = value`; `#` starts a comment. Invalid lines keep their defaults and are
/// returned as messages for the caller to show.
pub fn load_settings() -> Result<Option<(Settings, Vec<String>)>> {
    Ok(read_setting(SETTINGS_FILE)?.map(|lines| parse_settings(&lines)))
}

fn parse_settings(lines: &[String]) -> (Settings, Vec<String>) {
    let mut settings = Settings::default();
    let mut problems = Vec::new();
    let mut legacy_immersive = None;
    let mut has_display_mode = false;
    let mode_id = spec(Key::DockDisplayMode).id;
    for line in lines.iter().filter(|line| !line.starts_with('#')) {
        let result = match line.split_once('=') {
            Some((id, value)) if id.trim() == "dock.immersive" => {
                legacy_immersive = Some(value.trim());
                continue;
            }
            Some((id, value)) => {
                has_display_mode |= id.trim() == mode_id;
                settings.set_text(id.trim(), value.trim())
            }
            None => Err(format!("无法识别的设置行“{line}”，已忽略")),
        };
        if let Err(problem) = result {
            problems.push(problem);
        }
    }
    if !has_display_mode && let Some(value) = legacy_immersive {
        match value {
            "true" => settings.set_text(mode_id, "auto_hide").expect("valid mode"),
            "false" => {}
            _ => problems.push(format!("dock.immersive 的值“{value}”无效，已使用默认值")),
        }
    }
    (settings, problems)
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    let lines = settings
        .file_entries()
        .map(|(id, value)| format!("{id} = {value}"))
        .collect::<Vec<_>>();
    write_setting(
        SETTINGS_FILE,
        std::iter::once("# DeskIsle 设置，可在设置面板中修改")
            .chain(lines.iter().map(String::as_str)),
    )
}

fn settings_path(name: &str) -> Result<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Error::new(E_FAIL, "未找到 APPDATA 目录，无法读写 DeskIsle 设置"))?;
    Ok(std::path::PathBuf::from(base).join("DeskIsle").join(name))
}

fn io_error(path: &std::path::Path, error: std::io::Error) -> Error {
    let code = error
        .raw_os_error()
        .map_or(E_FAIL, |c| HRESULT::from_win32(c as u32));
    Error::new(code, format!("{}：{error}", path.display()))
}

/// Nonempty trimmed lines of a settings file under `%APPDATA%\DeskIsle`, or `None`
/// when the user never saved one.
fn read_setting(name: &str) -> Result<Option<Vec<String>>> {
    read_lines(&settings_path(name)?)
}

fn read_lines(path: &Path) -> Result<Option<Vec<String>>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(Into::into)
                .collect(),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(path, e)),
    }
}

fn write_setting<'a>(name: &str, lines: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let path = settings_path(name)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    }
    let text: String = lines.into_iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(&path, text).map_err(|e| io_error(&path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_immersive_setting_yields_to_explicit_display_mode() {
        use crate::visibility::DockDisplayMode::*;
        for (lines, expected) in [
            (vec![], Default),
            (vec!["dock.immersive=false"], Default),
            (vec!["dock.immersive=true"], AutoHide),
            (
                vec!["dock.immersive=true", "dock.display_mode=desktop"],
                Desktop,
            ),
            (
                vec!["dock.display_mode=desktop", "dock.immersive=true"],
                Desktop,
            ),
        ] {
            let (settings, problems) =
                parse_settings(&lines.into_iter().map(String::from).collect::<Vec<_>>());
            assert!(problems.is_empty());
            assert_eq!(settings.dock_display_mode(), expected);
            assert!(
                settings
                    .file_entries()
                    .all(|(id, _)| id != "dock.immersive")
            );
        }
    }

    #[test]
    fn independent_launchpad_round_trips_shortcuts_and_an_empty_list() {
        let dir = std::env::temp_dir().join(format!("deskisle-list-test-{}", std::process::id()));
        let file = dir.join(LAUNCHPAD_FILE);
        let entries = vec![
            PathBuf::from(r"C:\应用\游戏.lnk"),
            PathBuf::from(r"C:\Apps\Edit.EXE"),
        ];
        assert!(read_launchpad(&file).unwrap().is_empty());
        write_paths(&file, entries.iter().map(PathBuf::as_path)).unwrap();
        assert_eq!(read_launchpad(&file).unwrap(), entries);
        write_paths(&file, []).unwrap();
        assert!(read_launchpad(&file).unwrap().is_empty());
        assert!(!dir.join(PINS_FILE).exists());
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
