use std::collections::{BTreeMap, BTreeSet};
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
    insert_pins(pins, pins.len(), [pinned(path.into())]);
}

/// New pins keep their drop order. Running apps follow the pinned portion of the Dock.
pub fn insert_pins(
    pins: &mut Vec<PinnedApp>,
    index: usize,
    apps: impl IntoIterator<Item = PinnedApp>,
) {
    let at = index.min(pins.len());
    let mut known_apps: BTreeSet<_> = pins.iter().map(|pin| pin.app_id.clone()).collect();
    let mut known_paths: BTreeSet<_> = pins.iter().map(|pin| path_id(&pin.launch)).collect();
    let new_apps = apps.into_iter().filter(|app| {
        // A shortcut can be retargeted while the Dock is running.
        let path = path_id(&app.launch);
        if known_paths.contains(&path) || !known_apps.insert(app.app_id.clone()) {
            return false;
        }
        known_paths.insert(path)
    });
    pins.splice(at..at, new_apps);
}

/// The insertion boundary is measured before the dragged app leaves its original slot.
pub fn moved_pin_index(pin_count: usize, source: usize, insertion: usize) -> usize {
    let at = insertion.min(pin_count);
    at - usize::from(source < at)
}

pub fn move_pin(pins: &mut Vec<PinnedApp>, index: usize, app: PinnedApp) {
    let from = pins.iter().position(|pin| pin.app_id == app.app_id);
    let at = moved_pin_index(pins.len(), from.unwrap_or(pins.len()), index);
    let app = from.map_or(app, |from| pins.remove(from));
    pins.insert(at, app);
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
    /// The foreground window of this application, or its first in desktop Z order.
    pub fn current_window(&self, foreground: Option<isize>) -> Option<&WindowInfo> {
        self.windows
            .iter()
            .find(|window| Some(window.id) == foreground)
            .or_else(|| self.windows.first())
    }

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
    fn drop_inserts_new_apps_once_in_source_order() {
        let app = |p: &str| pinned(p.into());
        let mut pins = vec![app("a.exe"), app("b.exe")];
        insert_pins(
            &mut pins,
            1,
            ["A.EXE", "editor.lnk", "viewer.exe", "Editor.LNK"].map(app),
        );
        assert_eq!(
            pins,
            ["a.exe", "editor.lnk", "viewer.exe", "b.exe"].map(app)
        );
        let mut retargeted = app("EDITOR.LNK");
        retargeted.app_id = "new-editor.exe".into();
        insert_pins(&mut pins, 0, [retargeted]);
        assert_eq!(
            pins,
            ["a.exe", "editor.lnk", "viewer.exe", "b.exe"].map(app)
        );
    }

    #[test]
    fn moving_across_adjacent_slots_and_pinning_a_running_app() {
        let app = |p: &str| pinned(p.into());
        let mut pins = ["editor.lnk", "b.exe", "c.exe"].map(app).to_vec();
        // The shortcut and its running group have the same identity.
        pins[0].app_id = "editor.exe".into();
        move_pin(&mut pins, 2, app("editor.exe"));
        assert_eq!(pins[1].launch, Path::new("editor.lnk"));
        move_pin(&mut pins, 0, app("editor.exe"));
        assert_eq!(pins[0].launch, Path::new("editor.lnk"));
        move_pin(&mut pins, 7, app("running.exe"));
        assert_eq!(
            pins.iter().map(|p| p.launch.as_path()).collect::<Vec<_>>(),
            ["editor.lnk", "b.exe", "c.exe", "running.exe"].map(Path::new),
        );
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
