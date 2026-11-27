use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::Bars;
use crate::integrations::updates::{self, Release};
use crate::platform::{self, update::StagedUpdate};
use crate::settings::Key;
use crate::ui::update_prompt::Prompt;
use windows::core::Result;

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

type Worker<T> = JoinHandle<std::result::Result<T, String>>;

enum Job {
    Check {
        manual: bool,
        worker: Worker<Option<Release>>,
    },
    Download {
        worker: Worker<StagedUpdate>,
        cancelled: Arc<AtomicBool>,
    },
    Install(Worker<()>),
}

impl Job {
    fn finished(&self) -> bool {
        match self {
            Self::Check { worker, .. } => worker.is_finished(),
            Self::Download { worker, .. } => worker.is_finished(),
            Self::Install(worker) => worker.is_finished(),
        }
    }
}

#[derive(Default)]
pub(super) struct Updates {
    last_check: Option<Instant>,
    job: Option<Job>,
}

impl Updates {
    fn request_check(
        &mut self,
        manual: bool,
        enabled: bool,
        fullscreen: bool,
        now: Instant,
    ) -> bool {
        if let Some(job) = &mut self.job {
            if let Job::Check {
                manual: requested, ..
            } = job
            {
                *requested |= manual;
            }
            return false;
        }
        if !manual
            && (!enabled
                || fullscreen
                || self
                    .last_check
                    .is_some_and(|last| now.duration_since(last) < CHECK_INTERVAL))
        {
            return false;
        }
        self.last_check = Some(now);
        true
    }

    pub(super) fn install_ready(&self) -> bool {
        matches!(&self.job, Some(Job::Install(worker)) if worker.is_finished())
    }
}

impl Drop for Updates {
    fn drop(&mut self) {
        match self.job.take() {
            // On normal exit let the staged file's owner finish cleaning up.
            Some(Job::Download { worker, cancelled }) => {
                cancelled.store(true, Ordering::Relaxed);
                drop(worker.join());
            }
            Some(Job::Install(worker)) => drop(worker.join()),
            _ => {}
        }
    }
}

fn spawn<F, T>(work: F) -> std::result::Result<Worker<T>, String>
where
    F: FnOnce() -> std::result::Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    thread::Builder::new()
        .name("DeskIsle update".into())
        .spawn(work)
        .map_err(|e| format!("无法启动更新线程：{e}"))
}

fn finish<T>(worker: Worker<T>) -> std::result::Result<T, String> {
    worker.join().map_err(|_| "更新线程意外退出".to_owned())?
}

fn download(release: Release) -> std::result::Result<Job, String> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let worker = spawn(move || {
        let staged = platform::update::stage()?;
        release.download(staged.path(), &signal)?;
        Ok(staged)
    })?;
    Ok(Job::Download { worker, cancelled })
}

impl Bars {
    pub(super) fn update_status_label(&self) -> Option<&'static str> {
        Some(match self.updates.job.as_ref()? {
            Job::Check { .. } => "检查中…",
            Job::Download { .. } => "下载中…",
            Job::Install(_) => "准备重启…",
        })
    }

    pub(super) fn check_for_updates(&mut self, manual: bool) -> Result<()> {
        if self.bring_update_prompt_forward(manual) {
            return Ok(());
        }
        if !cfg!(target_arch = "x86_64") {
            if manual {
                self.show_update_prompt(Prompt::Unsupported, None)?;
            }
            return Ok(());
        }
        if !self.updates.request_check(
            manual,
            self.settings.enabled(Key::AutoCheckUpdates),
            self.fullscreen,
            Instant::now(),
        ) {
            return Ok(());
        }
        self.start_update_job(manual, || {
            spawn(updates::check).map(|worker| Job::Check { manual, worker })
        })
    }

    pub(super) fn start_update_download(&mut self, release: Release) -> Result<()> {
        self.start_update_job(true, || download(release))
    }

    fn start_update_job(
        &mut self,
        show_errors: bool,
        start: impl FnOnce() -> std::result::Result<Job, String>,
    ) -> Result<()> {
        let start = platform::update_timer(self.top.0, true)
            .map_err(|error| error.to_string())
            .and_then(|()| start());
        match start {
            Ok(job) => self.updates.job = Some(job),
            Err(error) => {
                let _ = platform::update_timer(self.top.0, false);
                if show_errors {
                    self.show_update_prompt(Prompt::Failed(error), None)?;
                }
            }
        }
        self.relayout_panel()
    }

    /// Returns true once the helper is ready for the normal shutdown sequence.
    pub(super) fn update_progress(&mut self) -> Result<bool> {
        if !self.updates.job.as_ref().is_some_and(Job::finished) {
            return Ok(false);
        }
        let next = match self.updates.job.take().expect("finished update job") {
            Job::Check { manual, worker } => match finish(worker) {
                Ok(Some(_)) if !manual && self.fullscreen => {
                    // Recheck after leaving full screen, when it is OK to show the prompt.
                    self.updates.last_check = None;
                    Ok(None)
                }
                Ok(Some(release)) if manual || self.settings.enabled(Key::AutoCheckUpdates) => {
                    self.show_update_prompt(
                        Prompt::Available(release.version.clone()),
                        Some(release),
                    )?;
                    Ok(None)
                }
                Ok(None) if manual => {
                    self.show_update_prompt(Prompt::Current, None)?;
                    Ok(None)
                }
                Err(error) if manual => Err(format!("检查更新失败：{error}")),
                _ => Ok(None),
            },
            Job::Download { worker, .. } => finish(worker)
                .and_then(|staged| spawn(move || staged.install()))
                .map(Job::Install)
                .map(Some),
            Job::Install(worker) => match finish(worker) {
                Ok(()) => return Ok(true),
                Err(error) => Err(error),
            },
        };
        match next {
            Ok(job) => self.updates.job = job,
            Err(error) => self.show_update_prompt(Prompt::Failed(error), None)?,
        }
        if self.updates.job.is_none() {
            platform::update_timer(self.top.0, false)?;
        }
        self.relayout_panel()?;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_checks_wait_for_desktop_and_interval_manual_checks_do_not() {
        let now = Instant::now();
        let mut state = Updates::default();
        assert!(!state.request_check(false, false, false, now));
        assert!(!state.request_check(false, true, true, now));
        assert!(state.request_check(false, true, false, now));
        assert!(!state.request_check(false, true, false, now + CHECK_INTERVAL / 2));
        assert!(state.request_check(false, true, false, now + CHECK_INTERVAL));
        assert!(state.request_check(true, false, true, now + CHECK_INTERVAL));
    }

    #[test]
    fn manual_check_joins_an_automatic_request_and_receives_the_result() {
        let mut state = Updates::default();
        state.job = Some(Job::Check {
            manual: false,
            worker: spawn(|| Ok(None)).unwrap(),
        });
        assert!(!state.request_check(true, false, false, Instant::now()));
        assert!(matches!(state.job, Some(Job::Check { manual: true, .. })));
    }
}
