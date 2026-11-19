use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedApp {
    pub app_id: String,
    pub label: String,
    pub icon_path: PathBuf,
    pub launch: PathBuf,
}

pub fn pinned(exe: PathBuf) -> PinnedApp {
    let name = exe
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let label = match name.as_str() {
        "explorer.exe" => "文件资源管理器".into(),
        "taskmgr.exe" => "任务管理器".into(),
        _ => exe
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    };
    PinnedApp {
        app_id: path_id(&exe),
        label,
        icon_path: exe.clone(),
        launch: exe,
    }
}

/// Pins the executable at `path`, keeping the Dock order otherwise.
pub fn pin(pins: &mut Vec<PinnedApp>, path: &Path) {
    let pin = pinned(path.into());
    if !pins.iter().any(|p| p.app_id == pin.app_id) {
        pins.push(pin);
    }
}

pub fn path_id(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: isize,
    pub app_id: String,
    pub title: String,
    pub label: String,
    pub icon_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppGroup {
    pub app_id: String,
    pub label: String,
    pub icon_path: PathBuf,
    launch: Option<PathBuf>,
    pub windows: Vec<WindowInfo>,
    pub active: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action<'a> {
    Launch(&'a Path),
    Activate(&'a WindowInfo),
    Choose(&'a [WindowInfo]),
}

impl AppGroup {
    /// Pinned applications stay on the Dock without windows.
    pub fn pinned(&self) -> bool {
        self.launch.is_some()
    }

    /// The executable that starts another instance, when its path is known.
    pub fn executable(&self) -> Option<&Path> {
        self.launch
            .as_deref()
            .or_else(|| Some(self.icon_path.as_path()).filter(|p| !p.as_os_str().is_empty()))
    }

    pub fn action(&self) -> Action<'_> {
        match (self.launch.as_deref(), self.windows.as_slice()) {
            (Some(path), []) => Action::Launch(path),
            (_, [only]) => Action::Activate(only),
            (_, choices) => Action::Choose(choices),
        }
    }
}

pub fn group_apps(pins: &[PinnedApp], windows: &[WindowInfo], foreground: isize) -> Vec<AppGroup> {
    // Stable application order keeps the target under the pointer when the Z order changes.
    let mut running: BTreeMap<&str, Vec<WindowInfo>> = BTreeMap::new();
    for window in windows {
        running
            .entry(&window.app_id)
            .or_default()
            .push(window.clone());
    }
    let mut groups = Vec::new();
    for pin in pins {
        let windows = running.remove(pin.app_id.as_str()).unwrap_or_default();
        groups.push(AppGroup {
            app_id: pin.app_id.clone(),
            label: pin.label.clone(),
            icon_path: pin.icon_path.clone(),
            launch: Some(pin.launch.clone()),
            active: windows.iter().any(|w| w.id == foreground),
            windows,
        });
    }
    for (app_id, windows) in running {
        let representative = &windows[0];
        groups.push(AppGroup {
            app_id: app_id.to_owned(),
            label: representative.label.clone(),
            icon_path: representative.icon_path.clone(),
            launch: None,
            active: windows.iter().any(|w| w.id == foreground),
            windows,
        });
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: isize, app: &str) -> WindowInfo {
        WindowInfo {
            id,
            app_id: app.into(),
            title: format!("{app} {id}"),
            label: app.into(),
            icon_path: app.into(),
        }
    }

    #[test]
    fn launch_switch_and_choose_follow_the_observed_windows() {
        let pins = [PinnedApp {
            app_id: "editor".into(),
            label: "Editor".into(),
            icon_path: "editor.exe".into(),
            launch: "editor.exe".into(),
        }];
        let windows = [
            window(3, "viewer"),
            window(1, "editor"),
            window(2, "editor"),
        ];
        let groups = group_apps(&pins, &windows[..1], 3);
        assert_eq!(groups[0].action(), Action::Launch(Path::new("editor.exe")));
        assert!(groups[1].active);
        let groups = group_apps(&pins, &windows[..2], 1);
        assert_eq!(groups[0].action(), Action::Activate(&windows[1]));
        assert!(groups[0].active);
        assert!(!groups[1].active);
        let groups = group_apps(&pins, &windows, 0);
        assert_eq!(groups[0].action(), Action::Choose(&windows[1..]));
        assert!(!groups.iter().any(|g| g.active));
    }
}
