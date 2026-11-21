use super::{load_shortcut, plain_link_target, window_info};
use crate::dock::AppGroup;
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};
use windows::Win32::{
    Foundation::{HWND, PROPERTYKEY},
    System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
    UI::Shell::{
        ADLT_RECENT, ApplicationDocumentLists,
        Common::IObjectArray,
        IApplicationDocumentLists, IShellItem, IShellLinkW,
        PropertiesSystem::{IPropertyStore, PSGetPropertyKeyFromName, SHGetPropertyStoreForWindow},
        SIGDN_FILESYSPATH,
    },
};
use windows::core::{BSTR, Interface, PCWSTR, w};

fn explicit_id(store: IPropertyStore, key: &PROPERTYKEY) -> Option<BSTR> {
    let value = unsafe { store.GetValue(key) }.ok()?;
    let id = BSTR::try_from(&value).ok()?;
    (!id.is_empty()).then_some(id)
}

fn app_id(app: &AppGroup, foreground: Option<isize>) -> Option<BSTR> {
    let mut key = PROPERTYKEY::default();
    unsafe { PSGetPropertyKeyFromName(w!("System.AppUserModel.ID"), &mut key) }.ok()?;
    if let Some(window) = app.current_window(foreground) {
        let hwnd = HWND(window.id as _);
        if window_info(hwnd)
            .ok()
            .flatten()
            .is_some_and(|info| info.app_id == app.app_id)
            && let Ok(store) = unsafe { SHGetPropertyStoreForWindow(hwnd) }
            && let Some(id) = explicit_id(store, &key)
        {
            return Some(id);
        }
    }
    let path = app.executable()?;
    if !path.extension()?.eq_ignore_ascii_case("lnk") {
        return None;
    }
    explicit_id(load_shortcut(path)?.cast().ok()?, &key)
}

fn document_path(items: &IObjectArray, index: u32) -> Option<PathBuf> {
    if let Ok(item) = unsafe { items.GetAt::<IShellItem>(index) } {
        let raw = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.ok()?;
        // GetDisplayName transfers a CoTaskMem-allocated, terminated path to the caller.
        // Preserve its UTF-16 units, including paths which cannot be represented as UTF-8.
        let path = PathBuf::from(OsString::from_wide(unsafe { raw.as_wide() }));
        unsafe { CoTaskMemFree(Some(raw.0.cast())) };
        return path.is_absolute().then_some(path);
    }
    // A Jump List may contain links to application commands with arguments. Only a
    // plain filesystem target can be reopened as a document.
    let link: IShellLinkW = unsafe { items.GetAt(index) }.ok()?;
    plain_link_target(&link)
}

/// Unsupported applications, disabled history, and virtual destinations have no file
/// rows. Never infer an identity from DeskIsle itself or show its own Jump List.
pub fn recent_documents(
    app: &AppGroup,
    foreground: Option<isize>,
    limit: u32,
) -> Option<Vec<PathBuf>> {
    if limit == 0 {
        return None;
    }
    let id = app_id(app, foreground)?;
    let lists: IApplicationDocumentLists =
        unsafe { CoCreateInstance(&ApplicationDocumentLists, None, CLSCTX_INPROC_SERVER) }.ok()?;
    unsafe { lists.SetAppID(PCWSTR(id.as_ptr())) }.ok()?;
    let items: IObjectArray = unsafe { lists.GetList(ADLT_RECENT, limit) }.ok()?;
    let count = unsafe { items.GetCount() }.ok()?.min(limit);
    Some(
        (0..count)
            .filter_map(|index| document_path(&items, index))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
    use windows::Win32::UI::Shell::{
        Common::IObjectCollection, EnumerableObjectCollection, SHCreateItemFromParsingName,
        ShellLink,
    };
    use windows::core::{HSTRING, Result};

    #[test]
    fn jump_list_file_and_parameterized_link() -> Result<()> {
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
        let file = std::env::current_exe().unwrap();
        let paths = unsafe {
            let path = HSTRING::from(file.as_path());
            let items: IObjectCollection =
                CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
            let item: IShellItem = SHCreateItemFromParsingName(&path, None)?;
            items.AddObject(&item)?;
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&path)?;
            link.SetArguments(w!("--open-last"))?;
            items.AddObject(&link)?;
            (document_path(&items, 0), document_path(&items, 1))
        };
        unsafe { RoUninitialize() };
        assert_eq!(paths, (Some(file), None));
        Ok(())
    }
}
