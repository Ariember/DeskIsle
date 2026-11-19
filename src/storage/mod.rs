use std::collections::BTreeSet;
use windows::Win32::Foundation::E_FAIL;
use windows::core::{Error, HRESULT, Result};

use crate::dock::{PinnedApp, pinned};

const PINS_FILE: &str = "pins.txt";

/// The pinned applications the user saved, or `None` before the first change.
pub fn load_pins() -> Result<Option<Vec<PinnedApp>>> {
    Ok(read_setting(PINS_FILE)?
        .map(|lines| lines.into_iter().map(|line| pinned(line.into())).collect()))
}

pub fn save_pins(pins: &[PinnedApp]) -> Result<()> {
    let lines = pins
        .iter()
        .map(|pin| pin.launch.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    write_setting(PINS_FILE, lines.iter().map(String::as_str))
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
    let path = settings_path(name)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(Into::into)
                .collect(),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(&path, e)),
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
