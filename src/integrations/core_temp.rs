//! Optional read-only client of Core Temp's published shared-memory interface.
//! The mapping exists only when an external provider is already running.

use std::mem::MaybeUninit;
use windows::Win32::System::Diagnostics::ToolHelp::Toolhelp32ReadProcessMemory;
use windows::Win32::System::Memory::{
    FILE_MAP_READ, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile,
};
use windows::core::{Owned, PCWSTR, w};

// CoreTempSharedDataEx through uiStructVersion, with the SDK's 4-byte alignment.
// https://www.alcpu.com/CoreTemp/developers.html
#[repr(C)]
struct Readings {
    _load: [u32; 256],
    _tj_max: [u32; 128],
    cores_per_cpu: u32,
    cpus: u32,
    temperatures: [f32; 256],
    _clock_and_voltage: [f32; 4],
    _name: [u8; 100],
    fahrenheit: u8,
    delta_to_tj_max: u8,
    _tdp_supported: u8,
    _power_supported: u8,
    version: u32,
}

impl Readings {
    fn hottest_celsius(&self) -> Option<u8> {
        // Distance from TjMax is a different measurement. Only consume absolute
        // temperatures, with the units explicitly marked Celsius or Fahrenheit.
        if self.version < 2 || self.delta_to_tj_max != 0 || self.fahrenheit > 1 {
            return None;
        }
        let count = self.cores_per_cpu.checked_mul(self.cpus)? as usize;
        if self.cpus > 128 {
            return None;
        }
        self.temperatures
            .get(..count)?
            .iter()
            .filter_map(|&raw| {
                let celsius = if self.fahrenheit == 1 {
                    (raw - 32.) * (5. / 9.)
                } else {
                    raw
                };
                // Uninitialized slots are zero. Ignore non-finite and implausible sensor values.
                (celsius > 0. && celsius <= 150.).then(|| celsius.round() as u8)
            })
            .max()
    }
}

pub fn temperature() -> Option<u8> {
    read_mapping(w!("CoreTempMappingObjectEx"))
}

fn read_mapping(name: PCWSTR) -> Option<u8> {
    let mut data = MaybeUninit::<Readings>::uninit();
    let len = size_of::<Readings>();
    let mut copied = 0;
    unsafe {
        // Open the existing session-local mapping without creating or writing it.
        let mapping = Owned::new(OpenFileMappingW(FILE_MAP_READ.0, false, name).ok()?);
        let view = MapViewOfFile(*mapping, FILE_MAP_READ, 0, 0, len);
        if view.Value.is_null() {
            return None;
        }
        // Have Windows copy the exact view in our process and report inaccessible
        // pages without dereferencing foreign-controlled bytes.
        let ok = Toolhelp32ReadProcessMemory(
            std::process::id(),
            view.Value,
            data.as_mut_ptr().cast(),
            len,
            &mut copied,
        );
        let _ = UnmapViewOfFile(view);
        if !ok.as_bool() || copied != len {
            return None;
        }
        // All fields are numeric or bytes, with no invalid Rust bit patterns.
        data.assume_init().hottest_celsius()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_WRITE, PAGE_READWRITE};
    use windows::core::HSTRING;

    #[test]
    fn reads_a_running_provider_and_releases_the_mapping_after_each_sample() {
        let name = HSTRING::from(format!("DeskIsle.Test.CoreTemp.{}", std::process::id()));
        let name_ptr = PCWSTR(name.as_ptr());
        assert_eq!(read_mapping(name_ptr), None);
        unsafe {
            let len = size_of::<Readings>();
            let provider = Owned::new(
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    None,
                    PAGE_READWRITE,
                    0,
                    len as u32,
                    name_ptr,
                )
                .unwrap(),
            );
            let view = MapViewOfFile(*provider, FILE_MAP_WRITE, 0, 0, len);
            assert!(!view.Value.is_null());
            // Page-file-backed mappings start zeroed; this test owns the writer.
            let readings = view.Value.cast::<Readings>();
            (*readings).version = 2;
            (*readings).cpus = 1;
            (*readings).cores_per_cpu = 1;
            (*readings).temperatures[0] = 63.;
            UnmapViewOfFile(view).unwrap();
            assert_eq!(read_mapping(name_ptr), Some(63));
            drop(provider);
        }
        assert_eq!(read_mapping(name_ptr), None);
    }

    #[test]
    fn core_temp_selects_hottest_active_sensor_and_rejects_invalid_metadata() {
        // Raw integers and floats accept the zero bit pattern.
        let mut sample: Readings = unsafe { std::mem::zeroed() };
        assert_eq!(size_of::<Readings>(), 2692);
        sample.version = 2;
        sample.cpus = 2;
        sample.cores_per_cpu = 2;
        sample.fahrenheit = 1;
        sample.temperatures[..5].copy_from_slice(&[113., f32::NAN, 140., f32::INFINITY, 212.]);
        assert_eq!(sample.hottest_celsius(), Some(60));
        sample.fahrenheit = 0;
        assert_eq!(sample.hottest_celsius(), Some(140));
        sample.delta_to_tj_max = 1;
        assert_eq!(sample.hottest_celsius(), None);
        sample.delta_to_tj_max = 0;
        sample.fahrenheit = 2;
        assert_eq!(sample.hottest_celsius(), None);
        sample.fahrenheit = 0;
        sample.cores_per_cpu = 129;
        assert_eq!(sample.hottest_celsius(), None);
        sample.cores_per_cpu = u32::MAX;
        assert_eq!(sample.hottest_celsius(), None);
        sample.cores_per_cpu = 0;
        assert_eq!(sample.hottest_celsius(), None);
    }
}
