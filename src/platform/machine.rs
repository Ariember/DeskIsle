//! Reads what the About card shows from Windows: the computer name, the firmware's
//! model and chassis, the processor and Windows release from the registry, graphics
//! adapters from DXGI and memory from the memory status and SMBIOS tables. Each fact
//! is read on its own; one that cannot be read is left out rather than failing the card.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::SystemInformation::{
    ComputerNamePhysicalDnsHostname, GetComputerNameExW, GetSystemFirmwareTable,
    GlobalMemoryStatusEx, MEMORYSTATUSEX, RSMB,
};
use windows::core::{PCWSTR, PWSTR, w};

use super::power;
use crate::machine::{Cpu, Form, Gpu, Machine, Memory, Os, real_model};

const BIOS: PCWSTR = w!(r"HARDWARE\DESCRIPTION\System\BIOS");
const PROCESSOR: PCWSTR = w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
const RELEASE: PCWSTR = w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
/// Microsoft's vendor ID, carried by the Basic Render Driver and other software adapters.
const MICROSOFT: u32 = 0x1414;

/// Everything the About card shows, read now.
pub fn query() -> Machine {
    let smbios = firmware_table()
        .map(|t| parse_smbios(&t))
        .unwrap_or_default();
    let form = match smbios.chassis {
        Some(chassis) => Form::from_chassis(chassis),
        // Without SMBIOS a battery is the best hint of a portable.
        None if matches!(power::battery(), Ok(Some(_))) => Form::Laptop,
        None => Form::Desktop,
    };
    let model = text(HKEY_LOCAL_MACHINE, BIOS, w!("SystemProductName"))
        .and_then(|m| real_model(&m))
        .or_else(|| {
            text(HKEY_LOCAL_MACHINE, BIOS, w!("SystemFamily")).and_then(|m| real_model(&m))
        });
    Machine {
        name: computer_name()
            .or_else(|| std::env::var("COMPUTERNAME").ok())
            .unwrap_or_else(|| "此电脑".to_owned()),
        model,
        form,
        cpu: text(HKEY_LOCAL_MACHINE, PROCESSOR, w!("ProcessorNameString")).map(|name| Cpu {
            name,
            mhz: dword(HKEY_LOCAL_MACHINE, PROCESSOR, w!("~MHz")),
        }),
        gpu: gpu(),
        memory: physical_memory().map(|bytes| Memory {
            bytes,
            speed: smbios.memory_speed,
            kind: smbios.memory_kind,
        }),
        os: os(),
    }
}

fn computer_name() -> Option<String> {
    let mut size = 0u32;
    unsafe {
        // The first call fails with ERROR_MORE_DATA and reports the length with its NUL.
        let _ = GetComputerNameExW(ComputerNamePhysicalDnsHostname, None, &mut size);
        if size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; size as usize];
        GetComputerNameExW(
            ComputerNamePhysicalDnsHostname,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut size,
        )
        .ok()?;
        buffer.truncate(size as usize);
        Some(String::from_utf16_lossy(&buffer)).filter(|n| !n.is_empty())
    }
}

/// A string value, without the trailing NUL the registry stores.
fn text(key: HKEY, path: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(key, path, name, RRF_RT_REG_SZ, None, None, Some(&mut size))
            != ERROR_SUCCESS
            || size == 0
            || size > 64 * 1024
        {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        if RegGetValueW(
            key,
            path,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..end]);
        (!text.trim().is_empty()).then_some(text)
    }
}

fn dword(key: HKEY, path: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            key,
            path,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

fn os() -> Option<Os> {
    let build = text(HKEY_LOCAL_MACHINE, RELEASE, w!("CurrentBuildNumber"))?
        .trim()
        .parse()
        .ok()?;
    Some(Os {
        product: text(HKEY_LOCAL_MACHINE, RELEASE, w!("ProductName"))
            .unwrap_or_else(|| "Windows".to_owned()),
        edition: text(HKEY_LOCAL_MACHINE, RELEASE, w!("EditionID")).unwrap_or_default(),
        build,
        revision: dword(HKEY_LOCAL_MACHINE, RELEASE, w!("UBR")),
    })
}

fn physical_memory() -> Option<u64> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    (status.ullTotalPhys > 0).then_some(status.ullTotalPhys)
}

/// The hardware adapter with the most dedicated memory, which on a hybrid laptop is the
/// discrete GPU. Software and virtual display adapters report little or none.
fn gpu() -> Option<Gpu> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    let mut best: Option<Gpu> = None;
    // EnumAdapters1 fails with DXGI_ERROR_NOT_FOUND after the last adapter.
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || desc.VendorId == MICROSOFT {
            continue;
        }
        let end = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..end]);
        let memory = desc.DedicatedVideoMemory as u64;
        if name.trim().is_empty() || best.as_ref().is_some_and(|b| b.memory >= memory) {
            continue;
        }
        best = Some(Gpu { name, memory });
    }
    best
}

fn firmware_table() -> Option<Vec<u8>> {
    unsafe {
        let size = GetSystemFirmwareTable(RSMB, 0, None);
        if size == 0 {
            return None;
        }
        let mut table = vec![0u8; size as usize];
        let written = GetSystemFirmwareTable(RSMB, 0, Some(&mut table));
        if written == 0 || written > size {
            return None;
        }
        table.truncate(written as usize);
        Some(table)
    }
}

/// The SMBIOS facts the card uses, from the first chassis and populated memory device.
#[derive(Debug, Default, PartialEq, Eq)]
struct Smbios {
    chassis: Option<u8>,
    memory_kind: Option<u8>,
    memory_speed: Option<u32>,
}

/// Parses the `RawSMBIOSData` that `GetSystemFirmwareTable` returns: an 8-byte header
/// with the table length, then structures of a formatted area followed by strings that
/// end in a double NUL. Truncated structures end the walk.
fn parse_smbios(raw: &[u8]) -> Smbios {
    let mut found = Smbios::default();
    let Some(length) = raw
        .get(4..8)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    else {
        return found;
    };
    let end = raw.len().min(8usize.saturating_add(length as usize));
    let data = raw.get(8..end).unwrap_or_default();
    let word = |s: &[u8], at: usize| s.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let long = |s: &[u8], at: usize| {
        s.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let mut at = 0;
    while at + 4 <= data.len() {
        let (kind, size) = (data[at], data[at + 1] as usize);
        if size < 4 || at + size > data.len() {
            break;
        }
        let s = &data[at..at + size];
        match kind {
            3 if found.chassis.is_none() => found.chassis = s.get(5).copied(),
            // An empty slot reports a size of zero.
            17 if found.memory_kind.is_none() && word(s, 0x0c).is_some_and(|z| z != 0) => {
                found.memory_kind = s.get(0x12).copied();
                // 0xffff defers to the 32-bit extended speeds of SMBIOS 3.3.
                let speed = |short: usize, extended: usize| match word(s, short) {
                    Some(0xffff) => long(s, extended),
                    other => other.map(u32::from),
                };
                found.memory_speed = speed(0x20, 0x58)
                    .filter(|&v| v > 0)
                    .or_else(|| speed(0x15, 0x54))
                    .filter(|&v| v > 0);
            }
            127 => break,
            _ => {}
        }
        // Skip the string set, which ends in two NULs even when it is empty.
        let mut next = at + size;
        while next + 1 < data.len() && (data[next] != 0 || data[next + 1] != 0) {
            next += 1;
        }
        at = next + 2;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn structure(kind: u8, formatted: &[(usize, &[u8])], size: usize, strings: &[u8]) -> Vec<u8> {
        let mut s = vec![0u8; size];
        s[0] = kind;
        s[1] = size as u8;
        for (offset, bytes) in formatted {
            s[*offset..offset + bytes.len()].copy_from_slice(bytes);
        }
        // Each string ends in a NUL and the set in one more; an empty set is two NULs.
        if strings.is_empty() {
            s.extend_from_slice(&[0, 0]);
        } else {
            s.extend_from_slice(strings);
            s.push(0);
        }
        s
    }

    fn table(structures: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = structures.concat();
        let mut raw = vec![0, 3, 4, 0];
        raw.extend_from_slice(&(body.len() as u32).to_le_bytes());
        raw.extend(body);
        raw
    }

    #[test]
    fn smbios_gives_the_chassis_and_first_populated_module() {
        let raw = table(&[
            structure(0, &[], 0x18, b"BIOS vendor\0"),
            structure(3, &[(5, &[0x8a])], 0x15, b""),
            // An empty slot comes first and must be skipped.
            structure(17, &[(0x12, &[0x02])], 0x28, b""),
            structure(
                17,
                &[
                    (0x0c, &16384u16.to_le_bytes()),
                    (0x12, &[0x22]),
                    (0x15, &5600u16.to_le_bytes()),
                    (0x20, &5200u16.to_le_bytes()),
                ],
                0x28,
                b"DIMM A\0Vendor\0",
            ),
            structure(127, &[], 4, b""),
        ]);
        let found = parse_smbios(&raw);
        assert_eq!(Form::from_chassis(found.chassis.unwrap()), Form::Laptop);
        assert_eq!(found.memory_kind, Some(0x22));
        assert_eq!(found.memory_speed, Some(5200));
    }

    #[test]
    fn smbios_falls_back_to_the_rated_and_extended_speeds() {
        let rated = table(&[structure(
            17,
            &[
                (0x0c, &8192u16.to_le_bytes()),
                (0x15, &3200u16.to_le_bytes()),
            ],
            0x1c,
            b"",
        )]);
        assert_eq!(parse_smbios(&rated).memory_speed, Some(3200));
        let extended = table(&[structure(
            17,
            &[
                (0x0c, &8192u16.to_le_bytes()),
                (0x20, &0xffffu16.to_le_bytes()),
                (0x58, &8400u32.to_le_bytes()),
            ],
            0x5c,
            b"",
        )]);
        assert_eq!(parse_smbios(&extended).memory_speed, Some(8400));
    }

    #[test]
    fn truncated_tables_end_the_walk() {
        assert_eq!(parse_smbios(&[]), Smbios::default());
        let mut raw = table(&[structure(3, &[(5, &[0x03])], 0x15, b"")]);
        raw.truncate(raw.len() - 6);
        assert_eq!(parse_smbios(&raw), Smbios::default());
    }

    #[test]
    fn query_reads_this_computer() {
        let machine = query();
        assert!(!machine.name.is_empty());
        assert!(machine.os.is_some());
    }
}
