use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows::Win32::{
    Foundation::POINT,
    UI::Shell::{DragFinish, DragQueryFileW, DragQueryPoint, HDROP},
};

/// WM_DROPFILES transfers the handle to the window. Copy the client point and
/// paths before releasing it so no Shell-owned pointer leaves the callback.
pub(super) fn read(drop: HDROP) -> (Option<POINT>, Vec<PathBuf>) {
    unsafe {
        let mut point = POINT::default();
        let in_client = DragQueryPoint(drop, &mut point).as_bool();
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
        DragFinish(drop);
        (in_client.then_some(point), paths)
    }
}
