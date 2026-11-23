use std::path::Path;

/// One absolute path per line in the Dock and launchpad lists.
pub fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|s| !s.chars().any(char::is_control))
}

/// Keep the shortcut itself so the Shell can apply its arguments, working directory
/// and chosen icon. URL files are also used by launchers such as Steam.
pub fn supported(path: &Path) -> bool {
    valid_path(path)
        && path.extension().is_some_and(|ext| {
            ["lnk", "exe", "url"]
                .iter()
                .any(|kind| ext.eq_ignore_ascii_case(kind))
        })
}
