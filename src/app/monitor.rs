//! Samples enabled counters off the UI thread. At most one snapshot awaits delivery.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::core::{Error, Result};

use super::Bars;
use crate::metrics::{Indicator, Metric, Sample, Sources};
use crate::platform::{self, metrics::Sampler};
use crate::settings::Key;

const INTERVAL: Duration = Duration::from_secs(2);

impl Bars {
    pub(super) fn apply_monitor_settings(&mut self) -> Result<()> {
        let sources = self.settings.sources();
        let alert = self.settings.monitor_highlight();
        self.indicators = Metric::ALL
            .into_iter()
            .filter(|m| m.enabled(sources))
            .map(|metric| {
                let style = self.settings.monitor_style(metric);
                let mut indicator = self
                    .indicators
                    .iter()
                    .find(|i| i.metric == metric)
                    .cloned()
                    .unwrap_or_else(|| Indicator::new(metric, style));
                indicator.configure(style, alert);
                indicator.fahrenheit = self.settings.enabled(Key::TemperatureUnit)
                    && matches!(metric, Metric::CpuTemperature | Metric::GpuTemperature);
                indicator
            })
            .collect();
        self.restatus();
        self.draw()
    }

    pub(super) fn sync_sampling(&mut self) {
        let count = if self.fullscreen {
            0
        } else {
            self.status.metrics.len()
        };
        let (visible, hidden) = self.indicators.split_at_mut(count);
        let sources: Sources = visible.iter().map(|i| i.metric).collect();
        self.metrics_worker.configure(sources);
        for indicator in hidden {
            indicator.value = None;
            indicator.history.clear();
            indicator.highlight = false;
        }
    }

    pub(super) fn draw_metrics(&mut self) -> Result<()> {
        if let Some(sample) = self.metrics_worker.take_sample() {
            let alert = self.settings.monitor_highlight();
            for indicator in &mut self.indicators[..self.status.metrics.len()] {
                indicator.update(sample, alert);
            }
            let renderer = self.renderer.as_mut().expect("renderer bound to bars");
            renderer.check_device()?;
            renderer.draw_metrics(&self.status, self.monitor.dpi, &self.indicators)?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct State {
    window: Option<usize>,
    sources: Sources,
    revision: u64,
    latest: Option<Sample>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

pub struct MetricsWorker(Arc<Shared>);

impl MetricsWorker {
    pub fn new(hwnd: HWND) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        shared.state.lock().expect("metrics state").window = Some(hwnd.0 as usize);
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("DeskIsle resource monitor".into())
            .spawn(move || sample_loop(&worker))
            .map_err(|e| Error::new(E_FAIL, format!("无法启动监控线程：{e}")))?;
        Ok(Self(shared))
    }

    pub fn configure(&self, sources: Sources) {
        let mut state = self.0.state.lock().expect("metrics state");
        if state.sources != sources {
            state.sources = sources;
            state.revision += 1;
            state.latest = None;
            self.0.changed.notify_one();
        }
    }

    pub fn take_sample(&self) -> Option<Sample> {
        self.0.state.lock().expect("metrics state").latest.take()
    }
}

impl Drop for MetricsWorker {
    fn drop(&mut self) {
        self.0.state.lock().expect("metrics state").window = None;
        self.0.changed.notify_one();
    }
}

fn sample_loop(shared: &Shared) {
    loop {
        let state = shared
            .changed
            .wait_while(shared.state.lock().expect("metrics state"), |s| {
                s.window.is_some() && s.sources == Sources::default()
            })
            .expect("metrics state");
        if state.window.is_none() {
            return;
        }
        let sources = state.sources;
        let revision = state.revision;
        // A new source set starts a fresh rate baseline, including after full screen.
        drop(state);
        let mut sampler = Sampler::default();
        loop {
            let mut sample = sampler.sample(sources);
            if sources.cpu_temperature {
                sample.cpu_temperature = crate::integrations::core_temp::temperature();
            }
            let mut state = shared.state.lock().expect("metrics state");
            let Some(hwnd) = state.window else { return };
            if state.revision != revision {
                break;
            }
            // Holding the lock through PostMessage keeps the borrowed HWND live.
            // A single queued notification covers later samples, including while a menu is open.
            if state.latest.is_some() || platform::notify_metrics(HWND(hwnd as _)).is_ok() {
                state.latest = Some(sample);
            }
            (state, _) = shared
                .changed
                .wait_timeout_while(state, INTERVAL, |s| {
                    s.window.is_some() && s.revision == revision
                })
                .expect("metrics state");
            if state.window.is_none() || state.revision != revision {
                break;
            }
        }
    }
}
