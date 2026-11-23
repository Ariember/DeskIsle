use super::{Event, launchpad::LaunchpadEvent, queue};
use std::cell::Cell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows::Win32::{
    Foundation::{DV_E_TYMED, E_INVALIDARG, HWND, POINT, POINTL},
    Graphics::Gdi::ScreenToClient,
    System::{
        Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL},
        Ole::{
            CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_NONE, IDropTarget,
            IDropTarget_Impl, OleInitialize, OleUninitialize, RegisterDragDrop, ReleaseStgMedium,
            RevokeDragDrop,
        },
        SystemServices::MODIFIERKEYS_FLAGS,
    },
    UI::Shell::{DragFinish, DragQueryFileW, DragQueryPoint, HDROP},
};
use windows::core::{Error, Ref, Result, implement};

#[derive(Clone, Copy)]
pub(super) enum Target {
    Dock,
    Launchpad,
}

pub(super) struct Registration(HWND);

impl Target {
    fn accepts(self, path: &std::path::Path) -> bool {
        crate::launchpad::supported(path)
            || (matches!(self, Self::Dock) && super::shell::dock_folder(path))
    }

    fn interface(self, hwnd: HWND) -> IDropTarget {
        FileTarget {
            hwnd,
            target: self,
            files: Cell::new(FileKinds::None),
            hover: Cell::new(None),
        }
        .into()
    }

    pub(super) fn register(self, hwnd: HWND) -> Result<Registration> {
        // OLE retains the interface and calls it on this window's UI thread.
        unsafe {
            OleInitialize(None)?;
            if let Err(error) = RegisterDragDrop(hwnd, &self.interface(hwnd)) {
                OleUninitialize();
                return Err(error);
            }
        }
        Ok(Registration(hwnd))
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // The owning window releases its registration before destroying the HWND.
        unsafe {
            let _ = RevokeDragDrop(self.0);
            OleUninitialize();
        }
    }
}

#[implement(IDropTarget)]
struct FileTarget {
    hwnd: HWND,
    target: Target,
    files: Cell<FileKinds>,
    hover: Cell<Option<(i32, i32)>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileKinds {
    None,
    Unsupported,
    IncludesSupported,
}

fn file_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
        ..Default::default()
    }
}

impl FileTarget {
    fn set_hover(&self, point: Option<(i32, i32)>) {
        let previous = self.hover.replace(point);
        let event = match self.target {
            Target::Dock if previous != point => Event::DockFileHover(point),
            Target::Launchpad if previous.is_some() != point.is_some() => {
                Event::Launchpad(LaunchpadEvent::FileHover(point.is_some()))
            }
            _ => return,
        };
        queue(self.hwnd, event);
    }

    fn destination(
        &self,
        screen: &POINTL,
        allowed: DROPEFFECT,
    ) -> Option<((i32, i32), DROPEFFECT)> {
        // Pinning records a reference. Never tell the source to remove its file.
        let effect = [DROPEFFECT_LINK, DROPEFFECT_COPY]
            .into_iter()
            .find(|&effect| allowed.contains(effect))?;
        let mut pt = POINT {
            x: screen.x,
            y: screen.y,
        };
        (self.files.get() != FileKinds::None
            && unsafe { ScreenToClient(self.hwnd, &mut pt) }.as_bool())
        .then_some(((pt.x, pt.y), effect))
    }
}

impl IDropTarget_Impl for FileTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        // Inspect the names once on entry. The importer checks accessibility at drop
        // time and reports unsupported files, including those in a mixed selection.
        let files = match data.ok().and_then(ole_paths) {
            Ok(paths) if paths.iter().any(|p| self.target.accepts(p)) => {
                FileKinds::IncludesSupported
            }
            Ok(paths) if !paths.is_empty() => FileKinds::Unsupported,
            _ => FileKinds::None,
        };
        self.files.set(files);
        self.DragOver(keys, pt, effect)
    }

    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        // OLE supplies an in/out pointer valid for the duration of this call.
        let effect = unsafe { effect.as_mut() }.ok_or_else(|| Error::from(E_INVALIDARG))?;
        let destination = self.destination(pt, *effect);
        *effect = destination.map_or(DROPEFFECT_NONE, |(_, effect)| effect);
        self.set_hover(
            destination
                .filter(|_| self.files.get() == FileKinds::IncludesSupported)
                .map(|(point, _)| point),
        );
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        self.files.set(FileKinds::None);
        self.set_hover(None);
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let effect = unsafe { effect.as_mut() }.ok_or_else(|| Error::from(E_INVALIDARG))?;
        let destination = self.destination(pt, *effect);
        *effect = DROPEFFECT_NONE;
        let result = if let Some(((x, y), accepted)) = destination {
            data.ok().and_then(ole_paths).map(|paths| {
                if !paths.is_empty() {
                    if paths.iter().any(|p| self.target.accepts(p)) {
                        *effect = accepted;
                    }
                    let event = match self.target {
                        Target::Dock => Event::DropFiles(self.hwnd, x, y, paths),
                        Target::Launchpad => Event::Launchpad(LaunchpadEvent::DropFiles(paths)),
                    };
                    queue(self.hwnd, event);
                }
            })
        } else {
            Ok(())
        };
        self.DragLeave()?;
        result
    }
}

fn ole_paths(data: &IDataObject) -> Result<Vec<PathBuf>> {
    // GetData transfers a storage medium. ReleaseStgMedium also handles providers
    // that retain ownership of the HGLOBAL via pUnkForRelease.
    unsafe {
        let mut medium = data.GetData(&file_format())?;
        let paths = if medium.tymed == TYMED_HGLOBAL.0 as u32 && !medium.u.hGlobal.is_invalid() {
            Ok(file_paths(HDROP(medium.u.hGlobal.0)))
        } else {
            Err(Error::from(DV_E_TYMED))
        };
        ReleaseStgMedium(&mut medium);
        paths
    }
}

/// WM_DROPFILES transfers the handle to the window. Copy the client point and
/// paths before releasing it so no Shell-owned pointer leaves the callback.
pub(super) fn read(drop: HDROP) -> (Option<POINT>, Vec<PathBuf>) {
    unsafe {
        let mut point = POINT::default();
        let in_client = DragQueryPoint(drop, &mut point).as_bool();
        let paths = file_paths(drop);
        DragFinish(drop);
        (in_client.then_some(point), paths)
    }
}

fn file_paths(drop: HDROP) -> Vec<PathBuf> {
    unsafe {
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut paths = Vec::new();
        for index in 0..count {
            let length = DragQueryFileW(drop, index, None);
            let mut buffer = vec![0; length as usize + 1];
            if length > 0 && DragQueryFileW(drop, index, Some(&mut buffer)) == length {
                paths.push(PathBuf::from(OsString::from_wide(
                    &buffer[..length as usize],
                )));
            }
        }
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Bar, EVENTS};
    use windows::Win32::System::Ole::DROPEFFECT_MOVE;
    use windows::Win32::UI::{
        Shell::{BHID_DataObject, IShellItem, SHCreateItemFromParsingName},
        WindowsAndMessaging::{CreateWindowExW, WS_POPUP},
    };
    use windows::core::{HSTRING, w};

    #[test]
    fn shell_drag_previews_cancels_and_imports_at_final_point() -> Result<()> {
        // A hidden popup with a nonzero origin exercises OLE screen coordinates.
        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                None,
                WS_POPUP,
                100,
                200,
                160,
                60,
                None,
                None,
                None,
                None,
            )?
        };
        let mut window = Bar(hwnd, None);
        window.1 = Some(Target::Dock.register(hwnd)?);
        let file = std::env::current_exe().unwrap();
        let item: IShellItem =
            unsafe { SHCreateItemFromParsingName(&HSTRING::from(file.as_path()), None)? };
        let data: IDataObject = unsafe { item.BindToHandler(None, &BHID_DataObject)? };
        let dock = Target::Dock.interface(hwnd);
        let pad = Target::Launchpad.interface(hwnd);
        let start = POINTL { x: 110, y: 207 };
        let end = POINTL { x: 126, y: 217 };
        let mut effect = DROPEFFECT_COPY;
        unsafe {
            let mut move_only = DROPEFFECT_MOVE;
            dock.DragEnter(&data, Default::default(), start, &mut move_only)?;
            assert_eq!(move_only, DROPEFFECT_NONE);
            dock.DragLeave()?;
            dock.DragEnter(&data, Default::default(), start, &mut effect)?;
            assert_eq!(effect, DROPEFFECT_COPY);
            dock.DragOver(Default::default(), end, &mut effect)?;
            dock.DragLeave()?;
            pad.DragEnter(&data, Default::default(), start, &mut effect)?;
            pad.DragLeave()?;
            dock.DragEnter(&data, Default::default(), start, &mut effect)?;
            dock.Drop(&data, Default::default(), end, &mut effect)?;
        }
        let events: Vec<_> = EVENTS.with_borrow_mut(|events| events.drain(..).collect());
        assert!(
            events
                == [
                    Event::DockFileHover(Some((10, 7))),
                    Event::DockFileHover(Some((26, 17))),
                    Event::DockFileHover(None),
                    Event::Launchpad(LaunchpadEvent::FileHover(true)),
                    Event::Launchpad(LaunchpadEvent::FileHover(false)),
                    Event::DockFileHover(Some((10, 7))),
                    Event::DropFiles(hwnd, 26, 17, vec![file]),
                    Event::DockFileHover(None),
                ]
        );
        Ok(())
    }
}
