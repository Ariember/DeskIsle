use std::cell::Cell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemWindowsDirectoryW};
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Error, HSTRING, Owned, PCWSTR, PWSTR, Result, w};

use crate::dock::{PinnedApp, WindowInfo};

pub const WINDOWS_CHANGED: u32 = WM_APP + 1;

thread_local! {
    // OUTOFCONTEXT callbacks run on the subscribing message-loop thread.
    static EVENT_TARGET: Cell<Option<HWND>> = const { Cell::new(None) };
    static CHANGE_POSTED: Cell<bool> = const { Cell::new(false) };
}

pub struct WindowEvents {
    _hooks: Vec<Owned<HWINEVENTHOOK>>,
}

impl WindowEvents {
    pub fn subscribe(hwnd: HWND) -> Result<Self> {
        let mut hooks = Vec::new();
        for (first, last, flags) in [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, 0),
            (
                EVENT_OBJECT_DESTROY,
                EVENT_OBJECT_HIDE,
                WINEVENT_SKIPOWNPROCESS,
            ),
            (
                EVENT_OBJECT_NAMECHANGE,
                EVENT_OBJECT_NAMECHANGE,
                WINEVENT_SKIPOWNPROCESS,
            ),
            (
                EVENT_OBJECT_CLOAKED,
                EVENT_OBJECT_UNCLOAKED,
                WINEVENT_SKIPOWNPROCESS,
            ),
        ] {
            let hook = unsafe {
                SetWinEventHook(
                    first,
                    last,
                    None,
                    Some(window_event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | flags,
                )
            };
            if hook.is_invalid() {
                return Err(Error::new(E_FAIL, "无法订阅窗口事件"));
            }
            // Each registration must be unhooked on this thread, including partial startup.
            hooks.push(unsafe { Owned::new(hook) });
        }
        EVENT_TARGET.set(Some(hwnd));
        Ok(Self { _hooks: hooks })
    }

    pub fn acknowledge(&self) {
        CHANGE_POSTED.set(false);
    }
}

impl Drop for WindowEvents {
    fn drop(&mut self) {
        EVENT_TARGET.set(None);
        CHANGE_POSTED.set(false);
    }
}

unsafe extern "system" fn window_event(
    _: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _: u32,
    _: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND
        && (hwnd.is_invalid() || object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32)
    {
        return;
    }
    if let Some(target) = EVENT_TARGET.get()
        && !CHANGE_POSTED.replace(true)
        && unsafe { PostMessageW(Some(target), WINDOWS_CHANGED, WPARAM(0), LPARAM(0)) }.is_err()
    {
        CHANGE_POSTED.set(false);
    }
}

// Windows paths and window captions are counted in UTF-16 code units.
const MAX_PATH_UNITS: usize = 32768;

pub fn pins() -> Result<Vec<PinnedApp>> {
    let mut path = vec![0; MAX_PATH_UNITS];
    let mut pins = Vec::new();
    for (filename, label) in [
        ("explorer.exe", "文件资源管理器"),
        ("Taskmgr.exe", "任务管理器"),
    ] {
        let len = unsafe {
            if filename == "explorer.exe" {
                GetSystemWindowsDirectoryW(Some(&mut path))
            } else {
                GetSystemDirectoryW(Some(&mut path))
            }
        } as usize;
        if len == 0 {
            return Err(Error::from_thread());
        }
        if len >= path.len() {
            return Err(ERROR_INSUFFICIENT_BUFFER.into());
        }
        let exe = PathBuf::from(OsString::from_wide(&path[..len])).join(filename);
        pins.push(PinnedApp {
            app_id: path_id(&exe),
            label: label.into(),
            icon_path: exe.clone(),
            launch: exe,
        });
    }
    Ok(pins)
}

fn path_id(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn process_path(pid: u32) -> Result<PathBuf> {
    unsafe {
        let process = Owned::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?);
        let mut path = vec![0; MAX_PATH_UNITS];
        let mut len = path.len() as u32;
        QueryFullProcessImageNameW(
            *process,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut len,
        )?;
        Ok(PathBuf::from(OsString::from_wide(&path[..len as usize])))
    }
}

fn window_info(hwnd: HWND) -> Result<Option<WindowInfo>> {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0
            || pid == std::process::id()
            || !IsWindowVisible(hwnd).as_bool()
            || hwnd == GetShellWindow()
        {
            return Ok(None);
        }
        let style = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
        if style.contains(WS_EX_TOOLWINDOW)
            || (!style.contains(WS_EX_APPWINDOW)
                && (style.contains(WS_EX_NOACTIVATE) || GetWindow(hwnd, GW_OWNER).is_ok()))
        {
            return Ok(None);
        }
        let mut class = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class) as usize;
        if matches!(
            String::from_utf16_lossy(&class[..len]).as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return Ok(None);
        }
        let mut cloaked = 0u32;
        let cloak_result = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        );
        if !IsWindow(Some(hwnd)).as_bool() || cloaked != 0 {
            return Ok(None);
        }
        if let Err(error) = cloak_result
            && error.code() != E_ACCESSDENIED
        {
            return Err(error);
        }
        let title_len = GetWindowTextLengthW(hwnd);
        let mut title = vec![0; (title_len as usize + 1).min(MAX_PATH_UNITS)];
        let len = GetWindowTextW(hwnd, &mut title) as usize;
        let path = process_path(pid);
        let mut current_pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut current_pid));
        if current_pid != pid {
            // The window vanished or its handle was reused during the queries.
            return Ok(None);
        }
        let title = String::from_utf16_lossy(&title[..len]);
        let (app_id, path, label) = match path {
            Ok(path) => {
                let label = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                (path_id(&path), path, label)
            }
            Err(error) if error.code() == E_ACCESSDENIED => {
                // The PID lets the user switch to this window without guessing its executable.
                (
                    format!("pid:{pid}"),
                    PathBuf::new(),
                    if title.is_empty() {
                        "受限应用".into()
                    } else {
                        title.clone()
                    },
                )
            }
            Err(error) => return Err(error),
        };
        Ok(Some(WindowInfo {
            id: hwnd.0 as isize,
            app_id,
            title: if title.is_empty() {
                label.clone()
            } else {
                title
            },
            label,
            icon_path: path,
        }))
    }
}

pub fn enumerate() -> Result<(Vec<WindowInfo>, Option<isize>)> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        // EnumWindows calls synchronously; this pointer belongs to the caller for that call.
        unsafe { (&mut *(data.0 as *mut Vec<HWND>)).push(hwnd) };
        true.into()
    }

    let mut handles: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect), LPARAM((&raw mut handles) as isize))? };
    let mut windows = Vec::new();
    for hwnd in handles {
        if let Some(window) = window_info(hwnd)? {
            windows.push(window);
        }
    }
    let foreground = unsafe { GetForegroundWindow() };
    // An owned modal dialog shares its taskbar entry with its top-level owner.
    let active = if windows
        .iter()
        .any(|window| window.id == foreground.0 as isize)
    {
        foreground
    } else {
        unsafe { GetAncestor(foreground, GA_ROOTOWNER) }
    };
    Ok((windows, (!active.is_invalid()).then_some(active.0 as isize)))
}

fn check_restricted_windows(path: &Path, windows: &[WindowInfo]) -> Result<()> {
    let pids: Vec<u32> = windows
        .iter()
        .filter_map(|window| window.app_id.strip_prefix("pid:")?.parse().ok())
        .collect();
    if pids.is_empty() {
        return Ok(());
    }
    // The process snapshot exposes a basename even when OpenProcess denies image-path access.
    // A name match is enough to refuse a duplicate launch, never enough to activate a window.
    unsafe {
        let snapshot = Owned::new(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        Process32FirstW(*snapshot, &mut entry)?;
        loop {
            if pids.contains(&entry.th32ProcessID) {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&unit| unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = OsString::from_wide(&entry.szExeFile[..end]);
                if name.is_empty()
                    || path
                        .file_name()
                        .is_some_and(|file| file.eq_ignore_ascii_case(&name))
                {
                    return Err(Error::new(
                        E_ACCESSDENIED,
                        format!(
                            "无法确认进程 {} 的应用路径，已取消启动以避免重复实例",
                            entry.th32ProcessID
                        ),
                    ));
                }
            }
            match Process32NextW(*snapshot, &mut entry) {
                Ok(()) => {}
                Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }
}

pub fn launch(owner: HWND, path: &Path, windows: &[WindowInfo]) -> Result<()> {
    check_restricted_windows(path, windows)?;
    let executable = HSTRING::from(path.as_os_str());
    let status = unsafe {
        ShellExecuteW(
            Some(owner),
            w!("open"),
            &executable,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    }
    .0 as isize;
    if status <= 32 {
        return Err(Error::new(
            E_FAIL,
            format!("Shell 启动失败（返回值 {status}）"),
        ));
    }
    Ok(())
}

pub fn activate(app_id: &str, id: isize) -> Result<bool> {
    let hwnd = HWND(id as _);
    if window_info(hwnd)?.is_none_or(|info| info.app_id != app_id) {
        return Ok(false);
    }
    unsafe {
        if IsIconic(hwnd).as_bool() && !ShowWindowAsync(hwnd, SW_RESTORE).as_bool() {
            if !IsWindow(Some(hwnd)).as_bool() {
                return Ok(false);
            }
            return Err(Error::new(E_FAIL, "窗口未接受还原请求"));
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            if !IsWindow(Some(hwnd)).as_bool() {
                return Ok(false);
            }
            return Err(Error::new(E_FAIL, "Windows 未允许切换到此窗口"));
        }
    }
    Ok(true)
}

pub fn choose_window(owner: HWND, windows: &[WindowInfo], point: POINT) -> Result<Option<isize>> {
    let labels = windows
        .iter()
        .map(|window| window.title.clone())
        .collect::<Vec<_>>();
    Ok(super::popup_menu(owner, point, &labels)?.map(|index| windows[index].id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_window_blocks_only_a_possible_duplicate_launch() {
        let windows = [WindowInfo {
            id: 1,
            app_id: format!("pid:{}", std::process::id()),
            label: String::new(),
            title: String::new(),
            icon_path: PathBuf::new(),
        }];
        let exe = std::env::current_exe().unwrap();
        assert_eq!(
            check_restricted_windows(&exe, &windows).unwrap_err().code(),
            E_ACCESSDENIED
        );
        check_restricted_windows(Path::new("explorer.exe"), &windows).unwrap();
    }
}
