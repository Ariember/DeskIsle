use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::core::GUID;

const INSTALL_SCRIPT: &str = include_str!("update/install.ps1");

pub struct StagedUpdate {
    exe: PathBuf,
    directory: PathBuf,
    payload: PathBuf,
    handed_off: bool,
}

pub fn stage() -> Result<StagedUpdate, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位当前程序：{e}"))?;
    StagedUpdate::beside(exe)
}

impl StagedUpdate {
    fn beside(exe: PathBuf) -> Result<Self, String> {
        let parent = exe.parent().ok_or("无法定位程序目录")?;
        let id = GUID::new().map_err(|e| e.to_string())?;
        let directory = parent.join(format!(".deskisle-update-{id:?}"));
        fs::create_dir(&directory).map_err(|e| format!("无法在程序目录中暂存更新：{e}"))?;
        Ok(Self {
            exe,
            payload: directory.join("deskisle.exe"),
            directory,
            handed_off: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.payload
    }

    /// The caller exits normally after the helper acknowledges that it is watching
    /// this process. This lets normal Drop handlers restore Explorer's taskbar.
    pub fn install(mut self) -> Result<(), String> {
        let mut child = helper_command(&self, "Start-UpdateHelper")?
            .env("DESKISLE_UPDATE_PID", std::process::id().to_string())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("无法启动更新程序：{e}"))?;
        let output = child.stdout.take().expect("piped updater stdout");
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::Builder::new().spawn(move || {
            let mut reply = String::new();
            let _ = BufReader::new(output).read_line(&mut reply);
            let _ = sender.send(reply);
        });
        if reader.is_err()
            || !receiver
                .recv_timeout(Duration::from_secs(15))
                .is_ok_and(|line| line.trim_end() == "ready")
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err("更新程序未能就绪，DeskIsle 将继续运行。".into());
        }
        self.handed_off = true;
        Ok(())
    }
}

impl Drop for StagedUpdate {
    fn drop(&mut self) {
        if !self.handed_off {
            let _ = fs::remove_file(&self.payload);
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

fn system_directory() -> Result<PathBuf, String> {
    let mut path = vec![0; 32768];
    let len = unsafe { GetSystemDirectoryW(Some(&mut path)) } as usize;
    if len == 0 || len >= path.len() {
        return Err("无法定位 Windows 系统目录".into());
    }
    Ok(PathBuf::from(OsString::from_wide(&path[..len])))
}

fn helper_command(stage: &StagedUpdate, entry: &'static str) -> Result<Command, String> {
    let mut command =
        Command::new(system_directory()?.join(r"WindowsPowerShell\v1.0\powershell.exe"));
    // The script is compiled into the app. User-controlled paths are data, even
    // when an install directory contains quotes or PowerShell metacharacters.
    command
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!("{INSTALL_SCRIPT}\n{entry}"))
        .env("DESKISLE_UPDATE_EXE", &stage.exe)
        .env("DESKISLE_UPDATE_DIR", &stage.directory)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A console utility blocked on stdin stands in for the running Dock, so
    // these tests exercise real executable sharing, shutdown and process launch.
    fn run_update(valid: bool) {
        let root =
            std::env::temp_dir().join(format!("便携 [A&B] '{}'", GUID::new().unwrap().to_u128()));
        fs::create_dir(&root).unwrap();
        let exe = root.join("my dock.exe");
        let system = system_directory().unwrap();
        let old = fs::read(system.join("findstr.exe")).unwrap();
        let new = if valid {
            fs::read(system.join("whoami.exe")).unwrap()
        } else {
            b"invalid executable".to_vec()
        };
        fs::write(&exe, &old).unwrap();
        let mut parent = Command::new(&exe)
            .arg(".")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .unwrap();
        let stage = StagedUpdate::beside(exe.clone()).unwrap();
        fs::write(stage.path(), &new).unwrap();
        let mut helper = helper_command(&stage, "Invoke-DeskIsleUpdate")
            .unwrap()
            .env("DESKISLE_UPDATE_PID", parent.id().to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        BufReader::new(helper.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        assert_eq!(fs::read(&exe).unwrap(), old);
        drop(parent.stdin.take());
        parent.wait().unwrap();
        let result = helper.wait_with_output().unwrap();
        assert_eq!(
            result.status.success(),
            valid,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fs::read(&exe).unwrap(), if valid { new } else { old });
        assert!(!stage.directory.exists());
        // The relaunched fixture exits on its own, releasing its image mapping.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Err(e) = fs::remove_file(&exe) {
            assert!(std::time::Instant::now() < deadline, "{e}");
            std::thread::sleep(Duration::from_millis(25));
        }
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn replaces_the_running_portable_exe_after_exit() {
        run_update(true);
    }

    #[test]
    fn restores_the_original_when_the_download_cannot_start() {
        run_update(false);
    }
}
