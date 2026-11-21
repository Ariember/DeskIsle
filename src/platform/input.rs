//! The foreground thread owns the keyboard layout and the IME mode shown in the bar.

use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::SystemServices::{LANG_CHINESE, LANG_ENGLISH},
    UI::{
        Input::{
            Ime::{IME_CMODE_NATIVE, ImmGetDefaultIMEWnd},
            KeyboardAndMouse::GetKeyboardLayout,
        },
        WindowsAndMessaging::*,
    },
};
use windows::core::{Error, Result};

pub(super) const TIMER: usize = 4;

/// IME mode can change without changing either the foreground window or its layout.
/// Windows destroys this window-owned timer with the bar.
pub fn set_timer(hwnd: HWND, enabled: bool) -> Result<()> {
    if enabled {
        if unsafe { SetTimer(Some(hwnd), TIMER, 1000, None) } == 0 {
            return Err(Error::from_thread());
        }
    } else {
        unsafe { KillTimer(Some(hwnd), TIMER)? };
    }
    Ok(())
}

/// Return no label when the layout is neither English nor Chinese, or the foreground
/// input method cannot be queried. Neither case establishes an English input mode.
pub fn language() -> Option<&'static str> {
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(0, &mut info).ok()?;
        let window = if info.hwndFocus.is_invalid() {
            info.hwndActive
        } else {
            info.hwndFocus
        };
        let thread = GetWindowThreadProcessId(window, None);
        if thread == 0 {
            return None;
        }
        // The low word of the HKL is a LANGID. Its lowest ten bits identify the language.
        match (GetKeyboardLayout(thread).0 as usize & 0x3ff) as u32 {
            LANG_ENGLISH => Some("英"),
            LANG_CHINESE => {
                let ime = ImmGetDefaultIMEWnd(window);
                if ime.is_invalid() {
                    return None;
                }
                // IMC_GETOPENSTATUS and IMC_GETCONVERSIONMODE are scalar IMM queries.
                // Bound replies from another process so a hung editor cannot stall the bar.
                let query = |command| {
                    let mut value = 0;
                    let result = SendMessageTimeoutW(
                        ime,
                        WM_IME_CONTROL,
                        WPARAM(command),
                        LPARAM(0),
                        SMTO_ABORTIFHUNG | SMTO_BLOCK,
                        30,
                        Some(&mut value),
                    );
                    (result.0 != 0).then_some(value)
                };
                const IMC_GETOPENSTATUS: usize = 5;
                const IMC_GETCONVERSIONMODE: usize = 1;
                let native = query(IMC_GETOPENSTATUS)? != 0
                    && query(IMC_GETCONVERSIONMODE)? & IME_CMODE_NATIVE.0 as usize != 0;
                Some(if native { "中" } else { "英" })
            }
            _ => None,
        }
    }
}
