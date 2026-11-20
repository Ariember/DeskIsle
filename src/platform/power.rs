//! Battery charge and the power plans (schemes) the user can switch between.

use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, HLOCAL, LocalFree};
use windows::Win32::System::Power::{
    ACCESS_SCHEME, GetSystemPowerStatus, PowerEnumerate, PowerGetActiveScheme,
    PowerReadFriendlyName, PowerSetActiveScheme, SYSTEM_POWER_STATUS,
};
use windows::core::{GUID, Result};

use crate::system::{Battery, PowerPlan};

const NO_BATTERY: u8 = 128;
const CHARGING: u8 = 8;
const UNKNOWN: u8 = 255;

/// The battery, or `None` on a computer without one.
pub fn battery() -> Result<Option<Battery>> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status)? };
    if status.BatteryFlag & NO_BATTERY != 0 && status.BatteryFlag != UNKNOWN {
        return Ok(None);
    }
    Ok(Some(Battery {
        percent: (status.BatteryLifePercent <= 100).then_some(status.BatteryLifePercent),
        plugged: status.ACLineStatus == 1,
        charging: status.BatteryFlag != UNKNOWN && status.BatteryFlag & CHARGING != 0,
    }))
}

fn active() -> Result<GUID> {
    let mut scheme: *mut GUID = std::ptr::null_mut();
    unsafe {
        PowerGetActiveScheme(None, &mut scheme).ok()?;
        // The system allocates the GUID with LocalAlloc.
        let id = *scheme;
        LocalFree(Some(HLOCAL(scheme.cast())));
        Ok(id)
    }
}

fn friendly_name(scheme: &GUID) -> Result<String> {
    let mut size = 0u32;
    unsafe {
        PowerReadFriendlyName(None, Some(scheme), None, None, None, &mut size).ok()?;
        let mut buffer = vec![0u16; (size as usize).div_ceil(2).max(1)];
        PowerReadFriendlyName(
            None,
            Some(scheme),
            None,
            None,
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
        )
        .ok()?;
        let end = buffer.iter().position(|&u| u == 0).unwrap_or(buffer.len());
        Ok(String::from_utf16_lossy(&buffer[..end]))
    }
}

/// Every power plan in the order Windows lists them, with the active one marked.
pub fn plans() -> Result<Vec<PowerPlan>> {
    let current = active()?;
    let mut plans = Vec::new();
    for index in 0.. {
        let mut scheme = GUID::zeroed();
        let mut size = size_of::<GUID>() as u32;
        let status = unsafe {
            PowerEnumerate(
                None,
                None,
                None,
                ACCESS_SCHEME,
                index,
                Some((&raw mut scheme).cast()),
                &mut size,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        status.ok()?;
        plans.push(PowerPlan {
            id: scheme.to_u128(),
            // A plan without a readable name is still selectable.
            name: friendly_name(&scheme).unwrap_or_else(|_| format!("{scheme:?}")),
            active: scheme == current,
        });
    }
    Ok(plans)
}

pub fn set_plan(id: u128) -> Result<()> {
    let scheme = GUID::from_u128(id);
    unsafe { PowerSetActiveScheme(None, Some(&scheme)).ok() }
}
