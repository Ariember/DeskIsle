//! Rebuild the Win11 tray inventory from Explorer's settings. The registry includes
//! old installations, so each candidate must also pass a live Shell position query.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, HWND};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::{
    KF_FLAG_DONT_VERIFY, NOTIFYICONIDENTIFIER, SHGetKnownFolderPath, Shell_NotifyIconGetRect,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, FindWindowExW, HWND_MESSAGE, LR_DEFAULTCOLOR,
};
use windows::core::{GUID, Owned, PCWSTR, PWSTR, Result, w};

use super::{NIF_GUID, NIF_ICON, NIF_TIP, NIM_MODIFY, Notify, OwnedIcon, TrayIcon, filter, shell};
use crate::dock::path_id;

pub(super) fn registered(owner: isize, id: u32, guid: Option<GUID>) -> bool {
    let ident = NOTIFYICONIDENTIFIER {
        cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: HWND(owner as _),
        uID: id,
        guidItem: guid.unwrap_or_default(),
    };
    unsafe { Shell_NotifyIconGetRect(&ident) }.is_ok()
}

fn value(key: HKEY, entry: PCWSTR, field: PCWSTR, ty: REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
    let mut size = 0;
    unsafe { RegGetValueW(key, entry, field, ty, None, None, Some(&mut size)) }
        .ok()
        .ok()?;
    // Skip corrupt/unreasonably large cached values. The registration itself survives
    // an unavailable bitmap; a later live notification can still supply it.
    if size == 0 || size > 4 * 1024 * 1024 {
        return None;
    }
    let mut bytes = vec![0; size as usize];
    unsafe {
        RegGetValueW(
            key,
            entry,
            field,
            ty,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()
    .ok()?;
    bytes.truncate(size as usize);
    Some(bytes)
}

fn text(key: HKEY, entry: PCWSTR, field: PCWSTR) -> Option<String> {
    let bytes = value(key, entry, field, RRF_RT_REG_SZ)?;
    let (pairs, _) = bytes.as_chunks::<2>();
    let units: Vec<_> = pairs
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16(&units).ok()
}

fn path(name: &str) -> Option<PathBuf> {
    let Some((folder, rest)) = name.strip_prefix('{').and_then(|s| s.split_once("}\\")) else {
        return Path::new(name).is_absolute().then(|| PathBuf::from(name));
    };
    let id = GUID::try_from(folder).ok()?;
    let raw = unsafe { SHGetKnownFolderPath(&id, KF_FLAG_DONT_VERIFY, None) }.ok()?;
    let directory = unsafe { raw.to_string() };
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    Some(Path::new(&directory.ok()?).join(rest))
}

struct AppWindow {
    hwnd: HWND,
    message_only: bool,
}

fn windows_by_path(
    top_level: Vec<HWND>,
    known: &[TrayIcon],
    request: &mut impl FnMut(HWND),
) -> HashMap<String, Vec<AppWindow>> {
    let pending: HashSet<_> = known
        .iter()
        .filter(|icon| icon.awaiting_registration && icon.owner != 0 && icon.visible())
        .map(|icon| shell::window_process(HWND(icon.owner as _)))
        .filter(|&pid| pid != 0)
        .collect();
    let mut handles: Vec<_> = top_level
        .into_iter()
        .map(|hwnd| AppWindow {
            hwnd,
            message_only: false,
        })
        .collect();
    let mut previous = None;
    while let Ok(hwnd) = unsafe { FindWindowExW(Some(HWND_MESSAGE), previous, None, None) } {
        handles.push(AppWindow {
            hwnd,
            message_only: true,
        });
        previous = Some(hwnd);
    }
    let mut paths = HashMap::new();
    let mut by_path: HashMap<String, Vec<AppWindow>> = HashMap::new();
    for window in handles {
        let pid = shell::window_process(window.hwnd);
        // Some apps handle TaskbarCreated on a different window in the same process.
        if pending.contains(&pid) {
            request(window.hwnd);
        }
        let exe = paths
            .entry(pid)
            .or_insert_with(|| shell::process_path(pid).ok().map(|p| path_id(&p)));
        if let Some(exe) = exe {
            by_path.entry(exe.clone()).or_default().push(window);
        }
    }
    by_path
}

pub(super) fn read(known: &[TrayIcon], mut request: impl FnMut(HWND)) -> Result<Vec<Notify>> {
    let mut raw = HKEY::default();
    unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Control Panel\NotifyIconSettings"),
            None,
            KEY_READ,
            &mut raw,
        )
        .ok()?
    };
    // RegOpenKeyExW returned an owned subkey; HKCU itself is borrowed.
    let key = unsafe { Owned::new(raw) };
    let windows = windows_by_path(shell::window_handles()?, known, &mut request);
    let mut items = Vec::new();
    for index in 0.. {
        // A registry subkey name can contain at most 255 UTF-16 code units.
        let mut name = [0; 256];
        let mut len = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                *key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        status.ok()?;
        let entry = PCWSTR(name.as_ptr());
        let Some(exe) = text(*key, entry, w!("ExecutablePath")).and_then(|s| path(&s)) else {
            continue;
        };
        let candidates = windows
            .get(&path_id(&exe))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let guid = text(*key, entry, w!("IconGuid"))
            .and_then(|s| GUID::try_from(s.trim_matches(['{', '}'])).ok())
            .filter(|g| *g != GUID::zeroed());
        let mut owners = Vec::new();
        let id = if let Some(guid) = guid {
            let existing = known.iter().find(|icon| icon.guid == Some(guid));
            if filter::is_system_indicator(Some(guid), HWND::default())
                || existing.is_some_and(|icon| icon.owner != 0 || !icon.awaiting_registration)
                || !registered(0, 0, Some(guid))
            {
                continue;
            }
            // Message-only listeners miss the broadcast. For an icon that still
            // lacks an owner, retry the app's top-level listeners as well.
            for window in candidates
                .iter()
                .filter(|w| existing.is_some() || w.message_only)
            {
                request(window.hwnd);
            }
            if existing.is_some() {
                continue;
            }
            owners.push(0);
            0
        } else {
            let Some(id) = value(*key, entry, w!("UID"), RRF_RT_REG_DWORD)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes)
            else {
                continue;
            };
            owners.extend(
                candidates
                    .iter()
                    .map(|w| w.hwnd.0 as isize)
                    .filter(|&owner| {
                        !known.iter().any(|icon| icon.identifies(owner, id, None))
                            && registered(owner, id, None)
                            && !filter::is_system_indicator(None, HWND(owner as _))
                    }),
            );
            id
        };
        if owners.is_empty() {
            continue;
        }
        let png = value(*key, entry, w!("IconSnapshot"), RRF_RT_REG_BINARY);
        let tip = text(*key, entry, w!("InitialTooltip")).unwrap_or_default();
        for owner in owners {
            let icon = png
                .as_deref()
                .filter(|p| p.starts_with(b"\x89PNG\r\n\x1a\n"))
                .and_then(|p| {
                    unsafe { CreateIconFromResourceEx(p, true, 0x30000, 0, 0, LR_DEFAULTCOLOR) }
                        .ok()
                })
                .map(OwnedIcon);
            items.push(Notify {
                received: Instant::now(),
                balloon: false,
                message: NIM_MODIFY,
                owner,
                id,
                flags: NIF_TIP
                    | if guid.is_some() { NIF_GUID } else { 0 }
                    | if icon.is_some() { NIF_ICON } else { 0 },
                callback: 0,
                icon,
                tip: tip.clone(),
                state: 0,
                state_mask: 0,
                version: 0,
                guid: guid.unwrap_or_default(),
                snapshot_path: Some(exe.clone()),
            });
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_owner_also_requests_its_process_listener() -> Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetDesktopWindow,
        };

        let create_window = || unsafe {
            CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!(""),
                Default::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        };
        let owner = create_window()?;
        let listener = create_window()?;
        let mut icon = TrayIcon {
            key: String::new(),
            name: String::new(),
            label: String::new(),
            stamp: 0,
            registration: 0,
            attention: Default::default(),
            icon: None,
            owner: owner.0 as isize,
            id: 1,
            guid: None,
            system_indicator: false,
            callback: 0,
            version: 0,
            state: 0,
            awaiting_registration: true,
        };
        let mut targets = Vec::new();
        let desktop = unsafe { GetDesktopWindow() };
        windows_by_path(vec![desktop], std::slice::from_ref(&icon), &mut |w| {
            targets.push(w)
        });
        assert!(targets.contains(&listener));
        assert!(!targets.contains(&desktop));
        targets.clear();
        // An app can explicitly add a noninteractive icon.
        icon.awaiting_registration = false;
        windows_by_path(Vec::new(), &[icon], &mut |w| targets.push(w));
        assert!(targets.is_empty());
        unsafe {
            DestroyWindow(listener)?;
            DestroyWindow(owner)?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires Explorer on the interactive Windows desktop"]
    fn read_explorer_tray_inventory() -> Result<()> {
        assert!(!registered(0, 0, Some(GUID::new()?)));
        let items = read(&[], |_| {})?;
        println!(
            "live Explorer notification icons: {} ({} images)",
            items.len(),
            items.iter().filter(|item| item.icon.is_some()).count()
        );
        Ok(())
    }
}
