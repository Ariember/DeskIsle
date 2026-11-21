use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemWindowsDirectoryW};
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::Shell::{IShellLinkW, ShellExecuteW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Error, HSTRING, Interface, Owned, PCWSTR, PWSTR, Result, w};

use crate::dock::{PinnedApp, WindowInfo, path_id, pinned};

pub const WINDOWS_CHANGED: u32 = WM_APP + 1;

/// A simple program shortcut shares its executable's running windows. Keep launching
/// the shortcut itself so the Shell applies its working directory and other options.
pub fn pin_from_path(path: PathBuf) -> PinnedApp {
    let mut pin = pinned(path);
    if pin
        .launch
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
        && let Some(exe) = shortcut_program(&pin.launch)
    {
        pin.app_id = path_id(&exe);
    }
    pin
}

fn shortcut_program(path: &Path) -> Option<PathBuf> {
    // Read the saved target without invoking Resolve, which can search or prompt.
    // Virtual, unreadable and parameterized links remain independently launchable.
    let link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let file: IPersistFile = link.cast().ok()?;
    let mut text = vec![0u16; 32_768];
    unsafe {
        file.Load(&HSTRING::from(path), STGM_READ).ok()?;
        link.GetArguments(&mut text).ok()?;
        if text[0] != 0 {
            return None;
        }
        link.GetPath(&mut text, std::ptr::null_mut(), 0).ok()?;
    }
    let len = text.iter().position(|&unit| unit == 0)?;
    let path = PathBuf::from(OsString::from_wide(&text[..len]));
    (path.is_absolute()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe")))
    .then_some(path)
}

/// The name an executable gives itself in its version resource: the product name, else
/// the file description. `None` when it has neither or the file cannot be read.
pub fn product_name(path: &Path) -> Option<String> {
    let file = HSTRING::from(path.as_os_str());
    let size = unsafe { GetFileVersionInfoSizeW(&file, None) };
    if size == 0 {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    unsafe { GetFileVersionInfoW(&file, None, size, block.as_mut_ptr().cast()) }.ok()?;
    // Pointers returned by VerQueryValueW point into `block`, which outlives them here.
    let query = |sub: &str| -> Option<(*const u8, u32)> {
        let sub = HSTRING::from(sub);
        let mut data = std::ptr::null_mut();
        let mut len = 0;
        let found = unsafe { VerQueryValueW(block.as_ptr().cast(), &sub, &mut data, &mut len) };
        (found.as_bool() && !data.is_null() && len > 0).then_some((data as *const u8, len))
    };
    // Each translation is a language and code page pair; US English is a common default.
    let mut languages = Vec::new();
    if let Some((data, len)) = query(r"\VarFileInfo\Translation") {
        let pairs = unsafe { std::slice::from_raw_parts(data.cast::<u16>(), len as usize / 2) };
        let (translations, _) = pairs.as_chunks::<2>();
        languages.extend(
            translations
                .iter()
                .map(|[language, page]| format!("{language:04x}{page:04x}")),
        );
    }
    languages.extend(["040904b0".to_owned(), "040904e4".to_owned()]);
    for key in ["ProductName", "FileDescription"] {
        for language in &languages {
            let Some((data, len)) = query(&format!(r"\StringFileInfo\{language}\{key}")) else {
                continue;
            };
            // `len` counts UTF-16 units including the terminator.
            let units = unsafe { std::slice::from_raw_parts(data.cast::<u16>(), len as usize) };
            let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
            let name = String::from_utf16_lossy(&units[..end]).trim().to_owned();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

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
    event_time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND
        && (hwnd.is_invalid() || object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32)
    {
        return;
    }
    if matches!(
        event,
        EVENT_OBJECT_SHOW | EVENT_OBJECT_HIDE | EVENT_OBJECT_DESTROY
    ) {
        super::tray::menu_event(event, hwnd, event_time);
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

/// Explorer and Task Manager, which the Dock keeps until the user changes its pins.
pub fn default_pins() -> Result<Vec<PinnedApp>> {
    let mut path = vec![0; MAX_PATH_UNITS];
    let mut pins = Vec::new();
    for filename in ["explorer.exe", "Taskmgr.exe"] {
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
        pins.push(pinned(exe));
    }
    Ok(pins)
}

pub(super) fn process_path(pid: u32) -> Result<PathBuf> {
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

pub(super) fn window_info(hwnd: HWND) -> Result<Option<WindowInfo>> {
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

pub(super) fn window_handles() -> Result<Vec<HWND>> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        // EnumWindows calls synchronously; this pointer belongs to the caller for that call.
        unsafe { (&mut *(data.0 as *mut Vec<HWND>)).push(hwnd) };
        true.into()
    }

    let mut handles: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect), LPARAM((&raw mut handles) as isize))? };
    Ok(handles)
}

pub fn enumerate() -> Result<(Vec<WindowInfo>, Option<isize>)> {
    let mut windows = Vec::new();
    for hwnd in window_handles()? {
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

pub fn launch(owner: HWND, path: &Path, app_id: &str, windows: &[WindowInfo]) -> Result<()> {
    check_restricted_windows(Path::new(app_id), windows)?;
    execute(owner, path)
}

/// Opens an executable, folder or URI with the Shell.
pub fn execute(owner: HWND, target: impl AsRef<OsStr>) -> Result<()> {
    execute_with_parameters(owner, &HSTRING::from(target.as_ref()), None)
}

fn execute_with_parameters(
    owner: HWND,
    file: &HSTRING,
    parameters: Option<&HSTRING>,
) -> Result<()> {
    let parameters = parameters.map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr()));
    let status = unsafe {
        ShellExecuteW(
            Some(owner),
            w!("open"),
            file,
            parameters,
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    }
    .0 as isize;
    if status <= 32 {
        return Err(Error::new(
            E_FAIL,
            format!("Shell 打开 {file} 失败（返回值 {status}）"),
        ));
    }
    Ok(())
}

/// Opens an Explorer window with `path` selected.
pub fn reveal(owner: HWND, path: &Path) -> Result<()> {
    let parameters = HSTRING::from(format!("/select,\"{}\"", path.display()));
    execute_with_parameters(owner, &HSTRING::from("explorer.exe"), Some(&parameters))
}

/// Asks each window of `app_id` to close, as the title bar's close button would; the
/// application may prompt to save, hide the window, or exit.
pub fn close_windows(app_id: &str, ids: &[isize]) -> Result<()> {
    let mut refused = None;
    for &id in ids {
        let hwnd = HWND(id as _);
        // The handle may have closed or been reused since the menu opened.
        if window_info(hwnd)?.is_none_or(|info| info.app_id != app_id) {
            continue;
        }
        if let Err(error) = unsafe {
            PostMessageW(
                Some(hwnd),
                WM_SYSCOMMAND,
                WPARAM(SC_CLOSE as usize),
                LPARAM(0),
            )
        } {
            refused = Some(error);
        }
    }
    match refused {
        // UIPI blocks messages to elevated applications.
        Some(error) => Err(Error::new(
            error.code(),
            format!("无法关闭窗口，该应用可能以管理员权限运行：{error}"),
        )),
        None => Ok(()),
    }
}

pub fn activate(app_id: &str, id: isize) -> Result<bool> {
    let hwnd = HWND(id as _);
    if window_info(hwnd)?.is_none_or(|info| info.app_id != app_id) {
        return Ok(false);
    }
    activate_window(hwnd)
}

pub(super) fn window_process(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn activate_window(hwnd: HWND) -> Result<bool> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_shortcut_groups_with_executable_and_keeps_launch_settings() -> Result<()> {
        use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};

        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
        let exe = std::env::current_exe().unwrap();
        let link_path =
            std::env::temp_dir().join(format!("deskisle-pin-{}.lnk", std::process::id()));
        let (plain, with_args) = unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(exe.as_path()))?;
            let file: IPersistFile = link.cast()?;
            let name = HSTRING::from(link_path.as_path());
            file.Save(&name, true)?;
            let plain = pin_from_path(link_path.clone());
            link.SetArguments(w!("--profile other"))?;
            file.Save(&name, true)?;
            (plain, pin_from_path(link_path.clone()))
        };
        unsafe { RoUninitialize() };
        std::fs::remove_file(&link_path).unwrap();
        assert_eq!(plain.app_id, path_id(&exe));
        assert_eq!(plain.launch, link_path);
        assert_eq!(plain.icon_path, link_path);
        assert_eq!(with_args.app_id, path_id(&link_path));
        Ok(())
    }

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

    #[test]
    fn executables_name_themselves_from_their_version_resource() {
        let mut cmd = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        cmd.push(r"System32\cmd.exe");
        let name = product_name(&cmd).expect("Windows binaries carry a version resource");
        assert!(!name.is_empty() && !name.contains('\0'));
        assert_eq!(product_name(Path::new(r"Z:\missing\none.exe")), None);
    }
}
