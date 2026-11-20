//! The primary monitor's brightness and HDR state, and Windows' light or dark mode.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DestroyPhysicalMonitors, DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo,
    GetDisplayConfigBufferSizes, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    SetMonitorBrightness,
};
use windows::Win32::Foundation::{
    E_FAIL, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, LPARAM, POINT, WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFOEXW, MonitorFromPoint,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
use windows::core::{Error, PCWSTR, Result, w};

use crate::system::{Display, to_percent, to_range};

const PERSONALIZE: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");

fn primary() -> HMONITOR {
    unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) }
}

/// Whether Windows shows apps in dark mode. Windows treats a missing value as light.
pub fn system_dark() -> Result<bool> {
    let mut light = 1u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PERSONALIZE,
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut light).cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    status.ok()?;
    Ok(light == 0)
}

/// Whether "Transparency effects" is on in Personalization > Colors. Windows treats a
/// missing value as on.
pub fn transparency() -> Result<bool> {
    let mut enabled = 1u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PERSONALIZE,
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut enabled).cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(true);
    }
    status.ok()?;
    Ok(enabled != 0)
}

/// Switches Windows and its apps between light and dark mode, as the Personalization
/// page does, then tells running applications to repaint.
pub fn set_system_dark(dark: bool) -> Result<()> {
    let light = u32::from(!dark);
    for value in [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")] {
        let status: WIN32_ERROR = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PERSONALIZE,
                value,
                REG_DWORD.0,
                Some((&raw const light).cast()),
                size_of::<u32>() as u32,
            )
        };
        status.ok()?;
    }
    // Broadcasting waits on every top-level window; a busy one must not stall the UI
    // thread. DeskIsle's own bars receive it too and switch their palette.
    std::thread::spawn(|| {
        let area: Vec<u16> = "ImmersiveColorSet".encode_utf16().chain([0]).collect();
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                // `area` lives until the broadcast returns.
                LPARAM(area.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                500,
                None,
            )
        };
    });
    Ok(())
}

/// The physical monitors behind the primary display, released on drop.
struct Physical(Vec<PHYSICAL_MONITOR>);

impl Physical {
    fn primary() -> Result<Self> {
        let monitor = primary();
        let mut count = 0;
        unsafe {
            GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count)?;
            let mut monitors = vec![PHYSICAL_MONITOR::default(); count as usize];
            GetPhysicalMonitorsFromHMONITOR(monitor, &mut monitors)?;
            Ok(Self(monitors))
        }
    }

    /// The first monitor that answers a DDC/CI brightness query, with its range.
    fn brightness(&self) -> Option<(usize, u32, u32, u32)> {
        self.0.iter().enumerate().find_map(|(index, monitor)| {
            let (mut min, mut current, mut max) = (0, 0, 0);
            let ok = unsafe {
                GetMonitorBrightness(monitor.hPhysicalMonitor, &mut min, &mut current, &mut max)
            } != 0;
            (ok && max > min).then_some((index, min, current, max))
        })
    }
}

impl Drop for Physical {
    fn drop(&mut self) {
        let _ = unsafe { DestroyPhysicalMonitors(&self.0) };
    }
}

/// Brightness in percent. Monitors without DDC/CI, including most built-in laptop
/// panels, return `None`. Each query talks to the monitor and can take tens of ms.
pub fn brightness() -> Option<u8> {
    let monitors = Physical::primary().ok()?;
    let (_, min, current, max) = monitors.brightness()?;
    Some(to_percent(current, min, max))
}

pub fn set_brightness(percent: u8) -> Result<()> {
    let monitors = Physical::primary()?;
    let (index, min, _, max) = monitors
        .brightness()
        .ok_or_else(|| Error::new(E_FAIL, "主显示器不支持通过 DDC/CI 调节亮度"))?;
    let handle = monitors.0[index].hPhysicalMonitor;
    if unsafe { SetMonitorBrightness(handle, to_range(percent, min, max)) } == 0 {
        return Err(Error::from_thread());
    }
    Ok(())
}

/// The display path that drives the primary monitor.
fn primary_path() -> Result<Option<DISPLAYCONFIG_PATH_INFO>> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    unsafe { GetMonitorInfoW(primary(), &mut info.monitorInfo).ok()? };
    let (mut paths, mut modes) = (0, 0);
    unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes).ok()? };
    let mut path_list = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
    let mut mode_list = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
    let status = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths,
            path_list.as_mut_ptr(),
            &mut modes,
            mode_list.as_mut_ptr(),
            None,
        )
    };
    status.ok()?;
    path_list.truncate(paths as usize);
    Ok(path_list.into_iter().find(|path| {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };
        let status = unsafe { DisplayConfigGetDeviceInfo(&mut source.header) };
        status == ERROR_SUCCESS.0 as i32 && source.viewGdiDeviceName == info.szDevice
    }))
}

/// `Some(enabled)` when the primary monitor supports HDR.
pub fn hdr() -> Option<bool> {
    let path = primary_path().ok()??;
    let mut color = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            size: size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut color.header) } != ERROR_SUCCESS.0 as i32 {
        return None;
    }
    // Bit 0 is advancedColorSupported, bit 1 advancedColorEnabled.
    let bits = unsafe { color.Anonymous.value };
    (bits & 1 != 0).then_some(bits & 2 != 0)
}

pub fn set_hdr(enabled: bool) -> Result<()> {
    let path = primary_path()?.ok_or_else(|| Error::new(E_FAIL, "找不到主显示器的显示路径"))?;
    let mut state = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
            size: size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    state.Anonymous.value = u32::from(enabled);
    let status = unsafe { DisplayConfigSetDeviceInfo(&state.header) };
    WIN32_ERROR(status as u32).ok()
}

/// Everything the display flyout shows. Brightness talks to the monitor, so callers
/// query this when the flyout opens rather than on every status bar update.
pub fn state() -> Display {
    Display {
        brightness: brightness(),
        // An unreadable preference shows as light, which is what Windows assumes.
        dark: system_dark().unwrap_or(false),
        hdr: hdr(),
    }
}
