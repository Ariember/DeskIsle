//! Read-only, machine-wide resource sampling. All handles belong to the sampling thread.

use crate::metrics::{Sample, Sources};
use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_ADAPTERINFO, D3DKMT_CLOSEADAPTER, D3DKMT_ENUMADAPTERS2,
    D3DKMT_QUERYADAPTERINFO, D3DKMTCloseAdapter, D3DKMTEnumAdapters2, D3DKMTQueryAdapterInfo,
    KMTQAITYPE_ADAPTERPERFDATA,
};
use windows::Win32::{
    Foundation::FILETIME,
    System::{
        Performance::{
            PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
            PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCollectQueryData,
            PdhGetFormattedCounterValue, PdhOpenQueryW,
        },
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::{GetActiveProcessorGroupCount, GetSystemTimes},
    },
};
use windows::core::{Owned, PCWSTR, w};

#[derive(Default)]
pub struct Sampler {
    previous_cpu: Option<CpuTimes>,
    cpu_counter: Option<Counter>,
    disk_counter: Option<Counter>,
}

impl Sampler {
    pub fn sample(&mut self, enabled: Sources) -> Sample {
        let cpu = if !enabled.cpu {
            self.previous_cpu = None;
            self.cpu_counter = None;
            None
        } else if unsafe { GetActiveProcessorGroupCount() } > 1 {
            // GetSystemTimes would only report this thread's primary processor group.
            Counter::sample(
                &mut self.cpu_counter,
                w!("\\Processor Information(_Total)\\% Processor Time"),
            )
        } else {
            let current = CpuTimes::read();
            let previous = std::mem::replace(&mut self.previous_cpu, current);
            current
                .zip(previous)
                .and_then(|(now, old)| now.busy_since(old))
        };
        let disk = if enabled.disk {
            Counter::sample(
                &mut self.disk_counter,
                w!("\\PhysicalDisk(_Total)\\% Idle Time"),
            )
            .map(|idle| 100 - idle)
        } else {
            self.disk_counter = None;
            None
        };
        Sample {
            cpu,
            memory: enabled.memory.then(memory_load).flatten(),
            disk,
            // Windows thermal zones have no portable mapping to CPU core sensors.
            cpu_temperature: None,
            gpu_temperature: enabled.gpu_temperature.then(gpu_temperature).flatten(),
        }
    }
}

#[derive(Clone, Copy)]
struct CpuTimes {
    idle: u64,
    kernel: u64,
    user: u64,
}

impl CpuTimes {
    fn read() -> Option<Self> {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        let ticks =
            |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        Some(Self {
            idle: ticks(idle),
            kernel: ticks(kernel),
            user: ticks(user),
        })
    }

    fn busy_since(self, old: Self) -> Option<u8> {
        // Kernel time includes idle time. Invalid or stationary counters need a new baseline.
        let kernel = self.kernel.checked_sub(old.kernel)?;
        let user = self.user.checked_sub(old.user)?;
        let total = kernel.checked_add(user)?;
        let busy = total.checked_sub(self.idle.checked_sub(old.idle)?)?;
        (total != 0).then(|| (100. * busy as f64 / total as f64).round() as u8)
    }
}

fn memory_load() -> Option<u8> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    (status.dwMemoryLoad <= 100).then_some(status.dwMemoryLoad as u8)
}

struct Counter {
    query: Owned<PDH_HQUERY>,
    handle: PDH_HCOUNTER,
}

impl Counter {
    fn open(path: PCWSTR) -> Option<Self> {
        let mut query = PDH_HQUERY::default();
        let mut handle = PDH_HCOUNTER::default();
        unsafe {
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            // Closing the query releases all attached counters, including on partial setup.
            let query = Owned::new(query);
            (PdhAddEnglishCounterW(*query, path, 0, &mut handle) == 0)
                .then_some(Self { query, handle })
        }
    }

    fn sample(slot: &mut Option<Self>, path: PCWSTR) -> Option<u8> {
        let fresh = slot.is_none();
        if fresh {
            *slot = Self::open(path);
        }
        let counter = slot.as_ref()?;
        let mut value = PDH_FMT_COUNTERVALUE::default();
        unsafe {
            if PdhCollectQueryData(*counter.query) != 0 {
                *slot = None;
                return None;
            }
            // Both configured counters calculate percentages over two collections.
            if fresh
                || PdhGetFormattedCounterValue(counter.handle, PDH_FMT_DOUBLE, None, &mut value)
                    != 0
                || !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA)
            {
                return None;
            }
            let percent = value.Anonymous.doubleValue;
            (0.0..=100.0)
                .contains(&percent)
                .then(|| percent.round() as u8)
        }
    }
}

/// Highest reported main sensor temperature among the enumerated display adapters.
fn gpu_temperature() -> Option<u8> {
    let mut list = D3DKMT_ENUMADAPTERS2::default();
    unsafe { D3DKMTEnumAdapters2(&mut list) }.ok().ok()?;
    if list.NumAdapters == 0 {
        return None;
    }
    let mut adapters = vec![D3DKMT_ADAPTERINFO::default(); list.NumAdapters as usize];
    list.pAdapters = adapters.as_mut_ptr();
    // On success the API transfers a handle for each returned adapter. Consume all of
    // them, even if an individual driver doesn't implement the temperature query.
    unsafe { D3DKMTEnumAdapters2(&mut list) }.ok().ok()?;
    let mut hottest = None;
    for adapter in adapters.iter().take(list.NumAdapters as usize) {
        let mut data = D3DKMT_ADAPTER_PERFDATA::default();
        let mut query = D3DKMT_QUERYADAPTERINFO {
            hAdapter: adapter.hAdapter,
            Type: KMTQAITYPE_ADAPTERPERFDATA,
            pPrivateDriverData: (&raw mut data).cast(),
            PrivateDriverDataSize: size_of::<D3DKMT_ADAPTER_PERFDATA>() as u32,
        };
        let status = unsafe { D3DKMTQueryAdapterInfo(&mut query) };
        let _ = unsafe {
            D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER {
                hAdapter: adapter.hAdapter,
            })
        };
        if status.is_ok() && data.Temperature != 0 {
            // Drivers report tenths of a degree Celsius. Skip missing and unrepresentable values.
            let degrees = (u64::from(data.Temperature) + 5) / 10;
            hottest = hottest.max(u8::try_from(degrees).ok());
        }
    }
    hottest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "samples live Windows performance counters and optional GPU sensors"]
    fn live_readings_and_rate_warmup() {
        let mut sampler = Sampler::default();
        let enabled = Sources {
            cpu: true,
            memory: true,
            disk: true,
            cpu_temperature: true,
            gpu_temperature: true,
        };
        let first = sampler.sample(enabled);
        assert_eq!(first.cpu, None);
        assert_eq!(first.disk, None);
        std::thread::sleep(std::time::Duration::from_secs(1));
        let sample = sampler.sample(enabled);
        eprintln!("Windows readings: {sample:?}");
        assert!(sample.cpu.is_some_and(|n| n <= 100));
        assert!(sample.memory.is_some_and(|n| n <= 100));
        assert!(sample.disk.is_none_or(|n| n <= 100));
        sampler.sample(Sources::default());
        let restarted = sampler.sample(enabled);
        assert_eq!(restarted.cpu, None);
        assert_eq!(restarted.disk, None);
    }

    #[test]
    fn cpu_usage_excludes_idle_and_rejects_discontinuous_intervals() {
        let first = CpuTimes {
            idle: 30,
            kernel: 70,
            user: 50,
        };
        let next = CpuTimes {
            idle: 60,
            kernel: 105,
            user: 65,
        };
        assert_eq!(next.busy_since(first), Some(40));
        assert_eq!(first.busy_since(first), None);
        assert_eq!(first.busy_since(next), None);
        assert_eq!(CpuTimes { idle: 100, ..next }.busy_since(first), None);
    }
}
