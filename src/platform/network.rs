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

fn radio_switch(states: impl Iterator<Item = RadioState>) -> Option<bool> {
    states
        .filter_map(|s| match s {
            RadioState::On => Some(true),
            RadioState::Off => Some(false),
            _ => None,
        })
        .reduce(|a, b| a || b)
}

/// The radios and the current connection. Each part that cannot be read shows as
/// missing rather than failing the whole snapshot.
pub fn state() -> Network {
    let radios = Radio::GetRadiosAsync().and_then(|op| op.join()).ok();
    let state = |wanted| {
        let states = radios.as_ref()?.into_iter().filter_map(|r| {
            (r.Kind().ok()? == kind(wanted))
                .then(|| r.State().ok())
                .flatten()
        });
        radio_switch(states)
    };
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
        wifi: state(Kind::WiFi),
        ssid,
        cellular: profile
            .as_ref()
            .is_some_and(|p| p.IsWwanConnectionProfile().unwrap_or(false)),
        // IANA ifType ethernetCsmacd (6). Virtual and mobile links also have profiles.
        wired: profile.as_ref().is_some_and(|p| {
            p.NetworkAdapter().and_then(|a| a.IanaInterfaceType()).ok() == Some(6)
        }),
        bluetooth: state(Kind::Bluetooth),
    }
}

/// Turns a radio on or off, as the quick settings toggle does.
pub fn request_change(wanted: Kind, enabled: bool) -> Result<impl FnOnce() -> Result<()> + Send> {
    // Initiate consent on the UI thread. Only the worker waits and changes the devices.
    let consent = Radio::RequestAccessAsync()?;
    Ok(move || set_radio(wanted, enabled, consent.join()?))
}

fn set_radio(wanted: Kind, enabled: bool, access: RadioAccessStatus) -> Result<()> {
    let label = match wanted {
        Kind::WiFi => "Wi-Fi",
        Kind::Bluetooth => "蓝牙",
    };
    if access != RadioAccessStatus::Allowed {
        return Err(Error::new(
            E_ACCESSDENIED,
            format!("Windows 未允许 DeskIsle 切换{label}，请在“隐私和安全性 > 无线电”中允许"),
        ));
    }
    let state = if enabled {
        RadioState::On
    } else {
        RadioState::Off
    };
    let mut found = false;
    for radio in Radio::GetRadiosAsync()?.join()? {
        if radio.Kind()? != kind(wanted) {
            continue;
        }
        let current = radio.State()?;
        if !matches!(current, RadioState::On | RadioState::Off) {
            continue;
        }
        found = true;
        if current != state && radio.SetStateAsync(state)?.join()? != RadioAccessStatus::Allowed {
            return Err(Error::new(
                E_ACCESSDENIED,
                format!("Windows 拒绝切换{label}"),
            ));
        }
    }
    if !found {
        return Err(Error::new(E_FAIL, format!("找不到可切换的{label}设备")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_reflects_every_controllable_radio() {
        assert_eq!(radio_switch([RadioState::Disabled].into_iter()), None);
        assert_eq!(
            radio_switch([RadioState::Off, RadioState::Disabled].into_iter()),
            Some(false)
        );
        assert_eq!(
            radio_switch([RadioState::Off, RadioState::On].into_iter()),
            Some(true)
        );
    }
}
