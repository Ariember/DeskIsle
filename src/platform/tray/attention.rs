use std::time::{Duration, Instant};

use windows::Win32::Graphics::Imaging::{
    GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICConvertBitmapSource,
};
use windows::Win32::UI::WindowsAndMessaging::HICON;

// A longer interval between distinct frames starts a new animation. Repeated
// submissions of the same static frame do not prolong a previous sequence.
const FRAME_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Default, PartialEq)]
pub(super) struct Pixels {
    width: u32,
    bytes: Vec<u8>,
}

impl Pixels {
    pub(super) fn read(factory: &IWICImagingFactory, icon: HICON) -> Option<Self> {
        unsafe {
            let bitmap = factory.CreateBitmapFromHICON(icon).ok()?;
            let (mut width, mut height) = (0, 0);
            bitmap.GetSize(&mut width, &mut height).ok()?;
            // Larger artwork can still be displayed, without retaining full-size
            // copies just to recognise a notification area's animation.
            if width == 0 || height == 0 || width > 256 || height > 256 {
                return None;
            }
            let source = WICConvertBitmapSource(&GUID_WICPixelFormat32bppPBGRA, &bitmap).ok()?;
            let mut bytes = vec![0; (width * height * 4) as usize];
            source
                .CopyPixels(std::ptr::null(), width * 4, &mut bytes)
                .ok()?;
            if bytes.chunks_exact(4).all(|pixel| pixel[3] == 0) {
                return Some(Self::default());
            }
            Some(Self { width, bytes })
        }
    }

    fn blank(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[derive(Default)]
pub(super) struct Attention {
    latest: Option<Pixels>,
    previous: Option<Pixels>,
    last_change: Option<Instant>,
    alternations: u8,
    acknowledged: bool,
    last_click: Option<Instant>,
    previous_tip: String,
    pub(super) unread: bool,
}

impl Attention {
    /// Returns true when the bar should adopt the incoming image.
    pub(super) fn frame(&mut self, incoming: Pixels, now: Instant) -> bool {
        if self.latest.as_ref() == Some(&incoming) {
            return false;
        }
        let repeats = self.previous.as_ref() == Some(&incoming);
        let same_burst = self
            .last_change
            .is_some_and(|last| now - last <= FRAME_INTERVAL);
        if same_burst && repeats {
            self.alternations = self.alternations.saturating_add(1);
        } else {
            self.alternations = 0;
            // A balloon and the first animation frames may arrive on either
            // side of the user's click for that same notification.
            self.acknowledged &= self
                .last_click
                .is_some_and(|click| now - click <= FRAME_INTERVAL);
        }
        let blank = incoming.blank();
        let was_blank = self.latest.as_ref().is_some_and(Pixels::blank);
        let had_frame = self.latest.is_some();
        self.previous = self.latest.replace(incoming);
        self.last_change = Some(now);

        // Keep the last visible image through an empty phase. For two visible
        // frames, require two full A-B-A cycles before treating them as a blink.
        if (blank && had_frame || self.alternations >= 3) && !self.acknowledged {
            self.notify(now);
        }
        if blank || (was_blank && repeats) {
            return false;
        }
        self.alternations <= 3
    }

    pub(super) fn tooltip(&mut self, label: &mut String, tip: String, now: Instant) {
        if tip == *label || tip.is_empty() {
            return;
        }
        let blinking = self.alternations >= 3
            || (self.previous.is_some()
                && (self.latest.as_ref().is_some_and(Pixels::blank)
                    || self.previous.as_ref().is_some_and(Pixels::blank)));
        if blinking
            && self
                .last_change
                .is_some_and(|last| now - last <= FRAME_INTERVAL)
            && self.previous_tip != tip
        {
            self.notify(now);
        }
        // Some applications alternate their tooltip with their two icon frames.
        self.previous_tip = std::mem::replace(label, tip);
    }

    pub(super) fn notify(&mut self, received: Instant) {
        // A menu defers tray updates until after its activation command.
        if self.last_click.is_some_and(|click| received <= click) {
            self.acknowledged = true;
        } else {
            self.unread = true;
        }
    }

    pub(super) fn acknowledge(&mut self, now: Instant) -> bool {
        self.last_click = Some(now);
        // A user may click on the very first alternate image, before enough
        // frames have arrived to distinguish a blink from a status change.
        self.acknowledged = self.unread
            || (self.previous.is_some()
                && self
                    .last_change
                    .is_some_and(|last| now - last <= FRAME_INTERVAL));
        std::mem::take(&mut self.unread)
    }

    pub(super) fn reset_frames(&mut self) {
        *self = Self {
            unread: self.unread,
            last_click: self.last_click,
            ..Self::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_icon_and_transparent_frame_do_not_replace_the_displayed_image()
    -> windows::core::Result<()> {
        use crate::platform::tray::OwnedIcon;
        use windows::Win32::Graphics::Imaging::CLSID_WICImagingFactory;
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
        use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
        use windows::Win32::UI::WindowsAndMessaging::{
            CopyIcon, CreateIcon, IDI_APPLICATION, LoadIconW,
        };

        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { RoUninitialize() }
            }
        }
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
        let _apartment = Apartment;
        let factory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
        let shared = unsafe { LoadIconW(None, IDI_APPLICATION)? };
        let copy = OwnedIcon(unsafe { CopyIcon(shared)? });
        // Two bytes per scanline in each monochrome mask. AND=1, XOR=0 preserves the backdrop.
        let blank = OwnedIcon(unsafe {
            CreateIcon(None, 16, 16, 1, 1, [0xff; 32].as_ptr(), [0; 32].as_ptr())?
        });
        let read = |icon| Pixels::read(&factory, icon).expect("decode test icon");
        let mut attention = Attention::default();
        let now = Instant::now();
        assert!(attention.frame(read(shared), now));
        assert!(!attention.frame(read(copy.0), now));
        assert!(!attention.unread);
        assert!(!attention.frame(read(blank.0), now));
        assert!(attention.unread);
        Ok(())
    }

    fn image(value: u8) -> Pixels {
        Pixels {
            width: 1,
            bytes: vec![value, 0, 0, 255],
        }
    }

    #[test]
    fn empty_flash_frames_keep_the_app_visible_and_click_acknowledges_the_burst() {
        let start = Instant::now();
        let mut attention = Attention::default();
        let at = |ms| start + Duration::from_millis(ms);
        assert!(attention.frame(image(1), at(0)));
        assert!(!attention.frame(Pixels::default(), at(500)));
        assert!(attention.unread);
        assert!(attention.acknowledge(at(510)));
        for time in [1000, 2000, 3000] {
            assert!(!attention.frame(image(1), at(time)));
            assert!(!attention.frame(Pixels::default(), at(time + 500)));
            assert!(!attention.unread);
        }
        attention.frame(image(1), at(4000));
        attention.frame(Pixels::default(), at(10000));
        assert!(attention.unread);
    }

    #[test]
    fn two_repeating_images_settle_and_a_distinct_status_image_still_updates() {
        let start = Instant::now();
        let mut attention = Attention::default();
        for (i, frame) in [1, 2, 1, 2, 1].into_iter().enumerate() {
            assert!(attention.frame(image(frame), start + Duration::from_millis(i as u64 * 500)));
        }
        assert!(attention.acknowledge(start + Duration::from_millis(2100)));
        for (i, frame) in [2, 1, 2].into_iter().enumerate() {
            assert!(!attention.frame(
                image(frame),
                start + Duration::from_millis(2500 + i as u64 * 500)
            ));
            assert!(!attention.unread);
        }
        assert!(attention.frame(image(3), start + Duration::from_secs(4)));
        assert!(!attention.unread);
    }

    #[test]
    fn click_on_first_alternate_frame_covers_the_rest_of_the_animation() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut attention = Attention::default();
        attention.frame(image(1), at(0));
        attention.frame(image(2), at(500));
        attention.acknowledge(at(550));
        for (i, frame) in [1, 2, 1, 2, 1].into_iter().enumerate() {
            attention.frame(image(frame), at(1000 + i as u64 * 500));
            assert!(!attention.unread);
        }
    }

    #[test]
    fn new_tooltip_during_a_blink_rearms_but_the_same_alternating_tips_do_not() {
        let now = Instant::now();
        let mut attention = Attention::default();
        let mut label = "App".to_string();
        attention.frame(image(1), now);
        attention.tooltip(&mut label, "1 message".into(), now);
        assert!(!attention.unread);
        attention.frame(Pixels::default(), now);
        assert!(attention.acknowledge(now));
        let later = now + Duration::from_millis(200);
        attention.frame(image(1), later);
        attention.tooltip(&mut label, "App".into(), later);
        attention.tooltip(&mut label, "1 message".into(), later);
        assert!(!attention.unread);
        attention.tooltip(&mut label, "2 messages".into(), later);
        assert!(attention.unread);
    }

    #[test]
    fn clicks_cover_deferred_notifications_and_following_animation_frames() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut attention = Attention::default();
        attention.frame(image(1), at(0));
        attention.acknowledge(at(100));
        attention.notify(at(50));
        assert!(!attention.unread);
        attention.frame(Pixels::default(), at(200));
        assert!(!attention.unread);
        attention.notify(at(400));
        assert!(attention.acknowledge(at(410)));
        attention.frame(image(1), at(500));
        attention.frame(Pixels::default(), at(600));
        assert!(!attention.unread);
    }
}
