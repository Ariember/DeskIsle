use std::cell::RefCell;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::core::GUID;

use super::{TRAY_SIGNATURE, TrayIcon};
use crate::geometry::Rect;

pub(super) const COPYDATA_ICON_RECT: usize = 3;

// Shell_NotifyIconGetRect uses the same wire layout from x86 and x64.
#[repr(C)]
struct RectRequest {
    signature: u32,
    operation: u32,
    size: u32,
    reserved: u32,
    owner: u32,
    id: u32,
    guid: GUID,
}

struct Placement {
    owner: u32,
    id: u32,
    guid: Option<GUID>,
    rect: Rect,
}

thread_local! {
    // The UI thread may receive a synchronous query while dispatching a tray click.
    static PLACEMENTS: RefCell<Vec<Placement>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn update<'a>(icons: impl Iterator<Item = (&'a TrayIcon, Rect)>) {
    PLACEMENTS.with_borrow_mut(|placements| {
        placements.clear();
        placements.extend(icons.map(|(icon, rect)| Placement {
            owner: icon.owner as u32,
            id: icon.id,
            guid: icon.guid,
            rect,
        }));
    });
}

/// The system owns the COPYDATA buffer until the host procedure returns.
pub(super) unsafe fn reply(data: &COPYDATASTRUCT) -> Option<LRESULT> {
    if data.lpData.is_null() || data.cbData as usize != size_of::<RectRequest>() {
        return None;
    }
    let request = unsafe { data.lpData.cast::<RectRequest>().read_unaligned() };
    if request.signature != TRAY_SIGNATURE || request.size != 32 {
        return None;
    }
    PLACEMENTS.with_borrow(|placements| {
        let rect = placements
            .iter()
            .find(|p| {
                if request.guid != GUID::zeroed() {
                    p.guid == Some(request.guid)
                } else {
                    p.guid.is_none() && (p.owner, p.id) == (request.owner, request.id)
                }
            })?
            .rect;
        // Shell32 adds the extent to the origin to construct the public RECT.
        let (x, y) = match request.operation {
            1 => (rect.x, rect.y),
            2 => (rect.width, rect.height),
            _ => return None,
        };
        Some(LRESULT(
            (u32::from(x as u16) | u32::from(y as u16) << 16) as isize,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_queries_get_the_published_origin_and_extent_for_each_icon() {
        let guid = GUID::from_u128(0x1234);
        let first = Rect {
            x: -250,
            y: -100,
            width: 32,
            height: 28,
        };
        PLACEMENTS.with_borrow_mut(|placements| {
            *placements = vec![
                Placement {
                    owner: 55,
                    id: 77,
                    guid: None,
                    rect: first,
                },
                Placement {
                    owner: 55,
                    id: 78,
                    guid: Some(guid),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        ..first
                    },
                },
            ];
        });
        let mut request = RectRequest {
            signature: TRAY_SIGNATURE,
            operation: 1,
            size: 32,
            reserved: 0,
            owner: 55,
            id: 77,
            guid: GUID::zeroed(),
        };
        let query = |request: &RectRequest, len| {
            let data = COPYDATASTRUCT {
                dwData: COPYDATA_ICON_RECT,
                cbData: len,
                lpData: std::ptr::from_ref(request).cast_mut().cast(),
            };
            unsafe { reply(&data) }.map(|result| (result.0 as i16, (result.0 >> 16) as i16))
        };
        assert_eq!(size_of::<RectRequest>(), 40);
        assert_eq!(query(&request, 40), Some((-250, -100)));
        request.operation = 2;
        assert_eq!(query(&request, 40), Some((32, 28)));
        request.guid = guid;
        request.operation = 1;
        assert_eq!(query(&request, 40), Some((0, 0)));
        assert_eq!(query(&request, 39), None);
        request.guid = GUID::from_u128(0x9999);
        assert_eq!(query(&request, 40), None);
        update(std::iter::empty());
        request.guid = GUID::zeroed();
        assert_eq!(query(&request, 40), None);
    }
}
