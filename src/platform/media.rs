//! The media session Windows shows in its own media controls.

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::Foundation::E_FAIL;
use windows::core::{Error, Result};

use crate::system::{Media, MediaAction};

fn session() -> Option<Session> {
    // Without a playing application there is no session; the call reports an error.
    Manager::RequestAsync()
        .ok()?
        .join()
        .ok()?
        .GetCurrentSession()
        .ok()
}

/// What is playing, or `None` when no application publishes a media session.
pub fn state() -> Option<Media> {
    let session = session()?;
    let properties = session.TryGetMediaPropertiesAsync().ok()?.join().ok();
    let playing = session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus())
        .is_ok_and(|status| status == Status::Playing);
    let text =
        |value: Result<windows::core::HSTRING>| value.map(|s| s.to_string()).unwrap_or_default();
    Some(Media {
        title: properties
            .as_ref()
            .map_or(String::new(), |p| text(p.Title())),
        artist: properties
            .as_ref()
            .map_or(String::new(), |p| text(p.Artist())),
        playing,
    })
}

pub fn control(action: MediaAction) -> Result<()> {
    let session = session().ok_or_else(|| Error::new(E_FAIL, "当前没有正在播放的媒体"))?;
    let request = match action {
        MediaAction::Previous => session.TrySkipPreviousAsync()?,
        MediaAction::TogglePlay => session.TryTogglePlayPauseAsync()?,
        MediaAction::Next => session.TrySkipNextAsync()?,
    };
    if request.join()? {
        Ok(())
    } else {
        Err(Error::new(E_FAIL, "播放器未接受此操作"))
    }
}
