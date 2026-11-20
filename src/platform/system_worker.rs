//! Serializes the network and media WinRT operations whose completion can depend on
//! a permission prompt, a device, or another application.

use std::sync::{Arc, Mutex, mpsc};
use windows::Win32::Foundation::{E_FAIL, HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{Error, Result};

use super::{media, network};
use crate::system::{Media, MediaAction, Network, RadioKind};

pub const UPDATED: u32 = WM_APP + 5;

pub enum Snapshot {
    Network(Network),
    Media(Option<Media>),
}

pub struct Update {
    pub snapshot: Snapshot,
    pub result: Result<()>,
}

type Task = Box<dyn FnOnce() -> Update + Send>;

pub struct SystemWorker {
    tasks: mpsc::Sender<Task>,
    updates: mpsc::Receiver<Update>,
    /// Cleared before the status bar is destroyed, including while consent is pending.
    target: Arc<Mutex<Option<usize>>>,
    network_pending: usize,
    media_pending: usize,
}

impl SystemWorker {
    pub fn new(hwnd: HWND) -> Result<Self> {
        let (tasks, requests) = mpsc::channel::<Task>();
        let (completed, updates) = mpsc::channel();
        let (started, ready) = mpsc::sync_channel(1);
        let target = Arc::new(Mutex::new(Some(hwnd.0 as usize)));
        let window = target.clone();
        std::thread::Builder::new()
            .name("DeskIsle system controls".into())
            .spawn(move || {
                let init = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
                let initialized = init.is_ok();
                let _ = started.send(init);
                if initialized {
                    for request in requests {
                        let update = request();
                        let target = window.lock().expect("system window lock");
                        let Some(hwnd) = *target else { break };
                        if completed.send(update).is_err() {
                            break;
                        }
                        // The lock keeps the handle valid until the message has been posted.
                        let _ = unsafe {
                            PostMessageW(Some(HWND(hwnd as _)), UPDATED, WPARAM(0), LPARAM(0))
                        };
                    }
                    unsafe { RoUninitialize() };
                }
            })
            .map_err(|e| Error::new(E_FAIL, format!("无法启动系统控件线程：{e}")))?;
        ready.recv().map_err(|_| stopped())??;
        Ok(Self {
            tasks,
            updates,
            target,
            network_pending: 0,
            media_pending: 0,
        })
    }

    pub fn refresh_network(&mut self) -> Result<()> {
        if self.network_pending == 0 {
            self.network(|| Ok(()))?;
        }
        Ok(())
    }

    pub fn set_radio(&mut self, kind: RadioKind, on: bool) -> Result<()> {
        self.network(network::request_change(kind, on)?)
    }

    fn network(&mut self, change: impl FnOnce() -> Result<()> + Send + 'static) -> Result<()> {
        self.tasks
            .send(Box::new(move || {
                let result = change();
                Update {
                    snapshot: Snapshot::Network(network::state()),
                    result,
                }
            }))
            .map_err(|_| stopped())?;
        self.network_pending += 1;
        Ok(())
    }

    pub fn media(&mut self, action: Option<MediaAction>) -> Result<()> {
        if action.is_none() && self.media_pending != 0 {
            return Ok(());
        }
        self.tasks
            .send(Box::new(move || {
                let result = action.map_or(Ok(()), media::control);
                Update {
                    snapshot: Snapshot::Media(media::state()),
                    result,
                }
            }))
            .map_err(|_| stopped())?;
        self.media_pending += 1;
        Ok(())
    }

    pub fn take_update(&mut self) -> Option<Update> {
        // A window message is only a notification; it carries no trusted pointers.
        let update = self.updates.try_recv().ok()?;
        match &update.snapshot {
            Snapshot::Network(_) => self.network_pending -= 1,
            Snapshot::Media(_) => self.media_pending -= 1,
        }
        Some(update)
    }
}

fn stopped() -> Error {
    Error::new(E_FAIL, "系统控件线程已退出")
}

impl Drop for SystemWorker {
    fn drop(&mut self) {
        *self.target.lock().expect("system window lock") = None;
    }
}
