//! The default sound output: its volume and mute state, the list of active outputs,
//! and switching the default device.

use std::ffi::c_void;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE_ACTIVE, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{GUID, PWSTR, Result, implement};

use crate::system::{Audio, AudioOutput};

/// Posted to the window passed to [`AudioWatch::new`] when the volume or mute changes.
pub const AUDIO_CHANGED: u32 = WM_APP + 3;

fn enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn default_volume() -> Result<IAudioEndpointVolume> {
    unsafe {
        let device = enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole)?;
        device.Activate(CLSCTX_ALL, None)
    }
}

/// Copies a COM-allocated string and frees it.
fn take(text: PWSTR) -> String {
    let copy = unsafe { text.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(text.0 as *const c_void)) };
    copy
}

fn name(device: &IMMDevice) -> Result<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ)?;
        let value = store.GetValue(&PKEY_Device_FriendlyName)?;
        Ok(take(PropVariantToStringAlloc(&value)?))
    }
}

/// The default output's volume and every active output. A computer without an output
/// device reports an error, which the caller shows as unavailable sound.
pub fn state() -> Result<Audio> {
    unsafe {
        let devices = enumerator()?;
        let default = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let default_id = take(default.GetId()?);
        let volume: IAudioEndpointVolume = default.Activate(CLSCTX_ALL, None)?;
        let level = volume.GetMasterVolumeLevelScalar()?;
        let muted = volume.GetMute()?.as_bool();
        let collection = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut outputs = Vec::new();
        for index in 0..collection.GetCount()? {
            let device = collection.Item(index)?;
            let id = take(device.GetId()?);
            // A device unplugged during the enumeration keeps its ID as its name.
            let name = name(&device).unwrap_or_else(|_| id.clone());
            outputs.push(AudioOutput {
                default: id == default_id,
                id,
                name,
            });
        }
        Ok(Audio {
            volume: (level.clamp(0., 1.) * 100.).round() as u8,
            muted,
            outputs,
        })
    }
}

pub fn set_volume(percent: u8) -> Result<()> {
    let volume = default_volume()?;
    unsafe {
        volume.SetMasterVolumeLevelScalar(f32::from(percent.min(100)) / 100., &GUID::zeroed())?;
        // Raising the volume from zero should be audible, as the system flyout does.
        if percent > 0 && volume.GetMute()?.as_bool() {
            volume.SetMute(false, &GUID::zeroed())?;
        }
    }
    Ok(())
}

pub fn set_muted(muted: bool) -> Result<()> {
    unsafe { default_volume()?.SetMute(muted, &GUID::zeroed()) }
}

// Method names follow the COM vtable.
#[allow(non_snake_case)]
mod policy {
    use std::ffi::c_void;

    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::ERole;
    use windows::Win32::Media::Audio::{eCommunications, eConsole, eMultimedia};
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR, Result};

    /// The interface the Sound control panel uses to change the default device. Windows does
    /// not document it, but it has kept this layout since Windows 7; only the vtable slot of
    /// `SetDefaultEndpoint` matters here, so the other methods take untyped pointers.
    #[windows::core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    unsafe trait IPolicyConfig: IUnknown {
        fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut c_void) -> HRESULT;
        fn GetDeviceFormat(
            &self,
            device: PCWSTR,
            default: i32,
            format: *mut *mut c_void,
        ) -> HRESULT;
        fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(
            &self,
            device: PCWSTR,
            endpoint: *mut c_void,
            mix: *mut c_void,
        ) -> HRESULT;
        fn GetProcessingPeriod(
            &self,
            device: PCWSTR,
            default: i32,
            period: *mut i64,
            minimum: *mut i64,
        ) -> HRESULT;
        fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
        fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn GetPropertyValue(
            &self,
            device: PCWSTR,
            key: *const PROPERTYKEY,
            value: *mut c_void,
        ) -> HRESULT;
        fn SetPropertyValue(
            &self,
            device: PCWSTR,
            key: *const PROPERTYKEY,
            value: *mut c_void,
        ) -> HRESULT;
        fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
    }

    const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    /// Makes the output with endpoint `id` the default for every role.
    pub fn set_default_output(id: &str) -> Result<()> {
        let wide: Vec<u16> = id.encode_utf16().chain([0]).collect();
        unsafe {
            let policy: IPolicyConfig = CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)?;
            for role in [eConsole, eMultimedia, eCommunications] {
                // `wide` outlives the call.
                policy
                    .SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role)
                    .ok()?;
            }
        }
        Ok(())
    }
}

pub use policy::set_default_output;

#[implement(IAudioEndpointVolumeCallback)]
struct Notify {
    /// The status bar, which outlives the subscription. Kept as a number because
    /// Windows calls back on its own worker thread, where only PostMessageW touches it.
    hwnd: isize,
}

impl IAudioEndpointVolumeCallback_Impl for Notify_Impl {
    fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        // The UI thread queries the new state itself; repeated posts are merged there.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.hwnd as _)),
                AUDIO_CHANGED,
                WPARAM(0),
                LPARAM(0),
            )
        };
        Ok(())
    }
}

/// Tells the status bar about volume changes on the current default output, such as the
/// keyboard's volume keys. Subscribe again after the default device changes.
pub struct AudioWatch {
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl AudioWatch {
    pub fn new(hwnd: HWND) -> Result<Self> {
        let volume = default_volume()?;
        let callback: IAudioEndpointVolumeCallback = Notify {
            hwnd: hwnd.0 as isize,
        }
        .into();
        unsafe { volume.RegisterControlChangeNotify(&callback)? };
        Ok(Self { volume, callback })
    }
}

impl Drop for AudioWatch {
    fn drop(&mut self) {
        let _ = unsafe { self.volume.UnregisterControlChangeNotify(&self.callback) };
    }
}
