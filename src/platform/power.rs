//! Battery charge, the power plans (schemes) the user can switch between, and
//! sleeping, restarting, shutting down, signing out and locking.

use windows::Win32::Foundation::{
    ERROR_NO_MORE_ITEMS, HANDLE, HLOCAL, HWND, LPARAM, LUID, LocalFree, WPARAM,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
};
use windows::Win32::System::Power::{
    ACCESS_SCHEME, GetPwrCapabilities, GetSystemPowerStatus, PowerEnumerate, PowerGetActiveScheme,
    PowerReadFriendlyName, PowerSetActiveScheme, SYSTEM_POWER_CAPABILITIES, SYSTEM_POWER_STATUS,
    SetSuspendState,
};
use windows::Win32::System::Shutdown::{
    EWX_LOGOFF, EWX_POWEROFF, EWX_REBOOT, EXIT_WINDOWS_FLAGS, ExitWindowsEx, LockWorkStation,
    SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER, SHUTDOWN_REASON,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SC_MONITORPOWER, WM_SYSCOMMAND};
use windows::core::{Error, GUID, Owned, PCWSTR, Result};

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

/// Enables the shutdown privilege in this process's token. Standard users hold it
/// disabled, and Windows checks it before sleeping, restarting or shutting down.
fn enable_shutdown_privilege() -> Result<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token)?;
        // The process handle is a pseudo handle; only the token is ours to close.
        let token = Owned::new(token);
        let mut luid = LUID::default();
        LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid)?;
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        // This also succeeds when the account lacks the privilege; the request that
        // needs it then fails, and that error is the one reported.
        AdjustTokenPrivileges(*token, false, Some(&raw const privileges), 0, None, None)
    }
}

/// Asks Windows to end the session. Without `EWX_FORCE`, applications may still keep
/// it open to save their work.
fn exit_windows(flags: EXIT_WINDOWS_FLAGS) -> Result<()> {
    let reason = SHUTDOWN_REASON(SHTDN_REASON_MAJOR_OTHER.0 | SHTDN_REASON_FLAG_PLANNED.0);
    unsafe { ExitWindowsEx(flags, reason) }
}

/// On S0 low-power idle systems Windows starts the standby transition at screen-off.
fn standby_display_command(caps: &SYSTEM_POWER_CAPABILITIES) -> Option<(WPARAM, LPARAM)> {
    caps.AoAc
        .then_some((WPARAM(SC_MONITORPOWER as usize), LPARAM(2)))
}

pub fn sleep(owner: HWND) -> Result<()> {
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    if !unsafe { GetPwrCapabilities(&mut caps) } {
        return Err(Error::from_thread());
    }
    if let Some((command, state)) = standby_display_command(&caps) {
        // The bar forwards this message to DefWindowProcW on its UI thread.
        return unsafe { PostMessageW(Some(owner), WM_SYSCOMMAND, command, state) };
    }
    enable_shutdown_privilege()?;
    if unsafe { SetSuspendState(false, false, false) } {
        Ok(())
    } else {
        Err(Error::from_thread())
    }
}

pub fn restart() -> Result<()> {
    enable_shutdown_privilege()?;
    exit_windows(EWX_REBOOT)
}

pub fn shut_down() -> Result<()> {
    enable_shutdown_privilege()?;
    exit_windows(EWX_POWEROFF)
}

/// Signing out needs no privilege.
pub fn sign_out() -> Result<()> {
    exit_windows(EWX_LOGOFF)
}

pub fn lock() -> Result<()> {
    unsafe { LockWorkStation() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_modern_standby_requests_display_power_off() {
        let disconnected_standby = SYSTEM_POWER_CAPABILITIES {
            AoAc: true,
            ..Default::default()
        };
        assert_eq!(
            standby_display_command(&disconnected_standby),
            Some((WPARAM(SC_MONITORPOWER as usize), LPARAM(2)))
        );
        let traditional_standby = SYSTEM_POWER_CAPABILITIES {
            SystemS3: true,
            ..Default::default()
        };
        assert_eq!(standby_display_command(&traditional_standby), None);
    }
}
