use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
use windows::core::GUID;

// Identifiers used by Windows Shell status indicators, independent of display language.
const SHELL_INDICATORS: &[GUID] = &[
    GUID::from_u128(0x7820ae73_23e3_4229_82c1_e41cb67d5b9c), // Volume
    GUID::from_u128(0x7820ae74_23e3_4229_82c1_e41cb67d5b9c), // Network
    GUID::from_u128(0x7820ae75_23e3_4229_82c1_e41cb67d5b9c), // Power
    GUID::from_u128(0x7820ae76_23e3_4229_82c1_e41cb67d5b9c), // Action Center
    GUID::from_u128(0x7820ae77_23e3_4229_82c1_e41cb67d5b9c), // Location
    GUID::from_u128(0x7820ae78_23e3_4229_82c1_e41cb67d5b9c), // Safely Remove Hardware
    // Additional built-in status registrations in the Windows 11 taskbar.
    GUID::from_u128(0x7820ae79_23e3_4229_82c1_e41cb67d5b9c),
    GUID::from_u128(0x7820ae80_23e3_4229_82c1_e41cb67d5b9c),
    GUID::from_u128(0x7820ae81_23e3_4229_82c1_e41cb67d5b9c), // Windows Update
    GUID::from_u128(0x7820ae82_23e3_4229_82c1_e41cb67d5b9c), // Microphone
    GUID::from_u128(0x7820ae83_23e3_4229_82c1_e41cb67d5b9c), // Meet Now
    GUID::from_u128(0xba82e2dc_f405_47ad_b032_cf0faa0e3933), // Windows Security
    GUID::from_u128(0x6da68f06_00f1_4e6e_8158_7f1ffd4f9db9), // Bluetooth
];

pub(super) fn is_system_indicator(guid: Option<GUID>, owner: HWND) -> bool {
    if guid.is_some_and(|id| SHELL_INDICATORS.contains(&id)) {
        return true;
    }
    if owner.is_invalid() {
        return false;
    }
    // Some components register by HWND and numeric ID. These classes belong to
    // their dedicated notification windows, even when hosted in Explorer or rundll32.
    let mut class = [0; 256];
    let len = unsafe { GetClassNameW(owner, &mut class) } as usize;
    is_system_class(&String::from_utf16_lossy(&class[..len]))
}

fn is_system_class(class: &str) -> bool {
    matches!(
        class,
        "SystemTray_Main"
            | "Windows Volume App Window"
            | "BluetoothNotificationAreaIconWindowClass"
            | "SecHealth Window Class"
            | "PrintTray_Notify_WndClass"
            | "CiceroUIWndFrame"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_shell_registrations_and_keeps_unrecognized_apps() {
        for id in [
            0x7820ae78_23e3_4229_82c1_e41cb67d5b9c,
            0x7820ae73_23e3_4229_82c1_e41cb67d5b9c,
            0xba82e2dc_f405_47ad_b032_cf0faa0e3933,
            0x6da68f06_00f1_4e6e_8158_7f1ffd4f9db9,
        ] {
            assert!(is_system_indicator(
                Some(GUID::from_u128(id)),
                HWND::default()
            ));
        }
        assert!(is_system_class("SystemTray_Main"));
        assert!(is_system_class("Windows Volume App Window"));
        assert!(is_system_class("PrintTray_Notify_WndClass"));
        assert!(is_system_class("CiceroUIWndFrame"));
        assert!(!is_system_class("Shell_TrayWnd"));
        assert!(!is_system_class("FxSound"));
        assert!(!is_system_indicator(None, HWND::default()));
        assert!(!is_system_indicator(
            Some(GUID::from_u128(0x7820ae73_23e3_4229_82c1_e41cb67d5b9d)),
            HWND::default(),
        ));
    }
}
