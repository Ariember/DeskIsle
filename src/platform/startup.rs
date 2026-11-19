//! Starting DeskIsle at sign-in through the current user's `Run` key.

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{Result, w};

const RUN_KEY: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const VALUE: windows::core::PCWSTR = w!("DeskIsle");

/// The command line Windows runs at sign-in for this executable.
fn command() -> Result<String> {
    Ok(format!("\"{}\"", std::env::current_exe()?.display()))
}

/// Whether sign-in starts this copy of DeskIsle. An entry left by a copy at another
/// path does not count; turning the setting on points it here.
pub fn enabled() -> Result<bool> {
    let mut buffer = vec![0u16; 4096];
    let mut bytes = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    status.ok()?;
    let units = (bytes as usize / 2).min(buffer.len());
    let end = buffer[..units]
        .iter()
        .position(|&u| u == 0)
        .unwrap_or(units);
    let saved = String::from_utf16_lossy(&buffer[..end]);
    Ok(saved.eq_ignore_ascii_case(&command()?))
}

pub fn set(enabled: bool) -> Result<()> {
    let status: WIN32_ERROR = if enabled {
        let data: Vec<u16> = command()?.encode_utf16().chain([0]).collect();
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE,
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            )
        }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE) } {
            ERROR_FILE_NOT_FOUND => return Ok(()),
            status => status,
        }
    };
    status.ok()
}
