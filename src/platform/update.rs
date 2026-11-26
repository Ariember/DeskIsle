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
    directory: PathBuf,
    payload: PathBuf,
    handed_off: bool,
}

pub fn stage() -> Result<StagedUpdate, String> {
    let id = GUID::new().map_err(|e| e.to_string())?;
    let directory = std::env::temp_dir().join(format!("deskisle-update-{id:?}"));
    fs::create_dir(&directory).map_err(|e| format!("无法暂存安装包：{e}"))?;
    Ok(StagedUpdate {
        payload: directory.join("DeskIsle-Setup-x64.exe"),
        directory,
        handed_off: false,
    })
}

impl StagedUpdate {
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
        .env("DESKISLE_UPDATE_PACKAGE", stage.path())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_update(setup_succeeds: bool) {
        let root = std::env::temp_dir().join(format!(
            "DeskIsle [A&B] '{}'",
            GUID::new().unwrap().to_u128()
        ));
        fs::create_dir(&root).unwrap();
        let target = root.join("deskisle.cmd");
        let launched = root.join("launched.txt");
        fs::write(&target, "@echo old>launched.txt\r\n").unwrap();

        // A batch fixture implements the installer's /S contract and leaves a
        // launch marker through the app it installs. No real profile is changed.
        let mut stage = stage().unwrap();
        stage.payload.set_extension("cmd");
        let setup = if setup_succeeds {
            "@if not \"%1\"==\"/S\" exit /b 2\r\n@echo @echo new ^>launched.txt > \"%DESKISLE_TEST_TARGET%\"\r\n"
        } else {
            "@exit /b 23\r\n"
        };
        fs::write(stage.path(), setup).unwrap();

        let mut parent = Command::new(system_directory().unwrap().join("findstr.exe"))
            .arg(".")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .unwrap();
        let mut helper = helper_command(
            &stage,
            "$target = $env:DESKISLE_TEST_TARGET\nInvoke-DeskIsleUpdate",
        )
        .unwrap()
        .env("DESKISLE_TEST_TARGET", &target)
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
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "@echo old>launched.txt\r\n"
        );
        drop(parent.stdin.take());
        parent.wait().unwrap();
        let result = helper.wait_with_output().unwrap();
        assert_eq!(
            result.status.success(),
            setup_succeeds,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!stage.directory.exists());
        if setup_succeeds {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !fs::read_to_string(&launched).is_ok_and(|s| s.trim() == "new") {
                assert!(
                    std::time::Instant::now() < deadline,
                    "updated app did not start"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        } else {
            assert!(String::from_utf8_lossy(&result.stderr).contains("23"));
            assert!(!launched.exists());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runs_setup_after_exit_and_launches_the_installed_app() {
        run_update(true);
    }

    #[test]
    fn reports_setup_failure_without_launching_the_app() {
        run_update(false);
    }
}
