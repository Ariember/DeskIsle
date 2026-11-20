//! Wi-Fi and Bluetooth radios and the network the computer is connected to.

use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Networking::Connectivity::NetworkInformation;
use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL};
use windows::core::{Error, Result};

use crate::system::{Network, RadioKind as Kind};

fn kind(kind: Kind) -> RadioKind {
    match kind {
        Kind::WiFi => RadioKind::WiFi,
        Kind::Bluetooth => RadioKind::Bluetooth,
    }
}

fn radio(wanted: Kind) -> Result<Option<Radio>> {
    let radios = Radio::GetRadiosAsync()?.join()?;
    for radio in radios {
        if radio.Kind()? == kind(wanted) {
            return Ok(Some(radio));
        }
    }
    Ok(None)
}

fn on(radio: &Radio) -> Result<bool> {
    Ok(radio.State()? == RadioState::On)
}

/// The radios and the current connection. Each part that cannot be read shows as
/// missing rather than failing the whole snapshot.
pub fn state() -> Network {
    let state = |wanted| radio(wanted).ok().flatten().and_then(|r| on(&r).ok());
    let wifi = state(Kind::WiFi);
    // The profile name of a Wi-Fi connection is its SSID; unlike the WLAN API it needs
    // no location permission.
    let profile = NetworkInformation::GetInternetConnectionProfile().ok();
    let wlan = profile
        .as_ref()
        .is_some_and(|profile| profile.IsWlanConnectionProfile().unwrap_or(false));
    let ssid = profile
        .as_ref()
        .filter(|_| wlan)
        .and_then(|profile| profile.ProfileName().ok())
        .map(|name| name.to_string());
    Network {
        wifi,
        ssid,
        wired: profile.is_some() && !wlan,
        bluetooth: state(Kind::Bluetooth),
    }
}

/// Turns a radio on or off, as the quick settings toggle does.
pub fn set_radio(wanted: Kind, enabled: bool) -> Result<()> {
    let label = match wanted {
        Kind::WiFi => "Wi-Fi",
        Kind::Bluetooth => "蓝牙",
    };
    if Radio::RequestAccessAsync()?.join()? != RadioAccessStatus::Allowed {
        return Err(Error::new(
            E_ACCESSDENIED,
            format!("Windows 未允许 DeskIsle 切换{label}，请在“隐私和安全性 > 无线电”中允许"),
        ));
    }
    let radio = radio(wanted)?.ok_or_else(|| Error::new(E_FAIL, format!("找不到{label}设备")))?;
    let state = if enabled {
        RadioState::On
    } else {
        RadioState::Off
    };
    match radio.SetStateAsync(state)?.join()? {
        RadioAccessStatus::Allowed => Ok(()),
        _ => Err(Error::new(
            E_ACCESSDENIED,
            format!("Windows 拒绝切换{label}"),
        )),
    }
}
