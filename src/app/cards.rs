//! Reflect installed plugins' local snapshots without reading plugin files on the UI thread.

use super::Bars;
use crate::cards::{Card, CardState};
use crate::integrations::plugins::load_cards;
use crate::platform;
use crate::render::is_device_lost;
use crate::settings::Key;
use crate::storage;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::core::{Error, Result};

struct Snapshot {
    owner: Option<usize>,
    dirty: bool,
    cards: Vec<Card>,
}

struct Shared {
    snapshot: Mutex<Snapshot>,
    stop: Condvar,
}

pub(super) struct CardReader(Arc<Shared>);

impl CardReader {
    fn start(hwnd: HWND) -> Result<Self> {
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot {
                owner: Some(hwnd.0 as usize),
                dirty: false,
                cards: Vec::new(),
            }),
            stop: Condvar::new(),
        });
        let reader = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("DeskIsle desktop cards".into())
            .spawn(move || {
                loop {
                    reader.snapshot.lock().expect("card reader").dirty = false;
                    let cards = match storage::plugins_directory() {
                        Ok(directory) => load_cards(&directory),
                        Err(_) => vec![Card {
                            title: "桌面卡片".into(),
                            value: "目录不可用".into(),
                            detail: "请检查当前用户的应用数据目录".into(),
                            state: CardState::Error,
                        }],
                    };
                    let mut snapshot = reader.snapshot.lock().expect("card reader");
                    let Some(owner) = snapshot.owner else { return };
                    // Drop clears owner under the same lock before the HWND can be destroyed.
                    if snapshot.cards != cards && platform::notify_cards(HWND(owner as _)).is_ok() {
                        snapshot.cards = cards;
                    }
                    let (snapshot, _) = reader
                        .stop
                        .wait_timeout_while(snapshot, Duration::from_secs(3), |s| {
                            s.owner.is_some() && !s.dirty
                        })
                        .expect("card reader");
                    if snapshot.owner.is_none() {
                        return;
                    }
                }
            })
            .map_err(|e| Error::new(E_FAIL, format!("无法启动桌面卡片读取线程：{e}")))?;
        Ok(Self(shared))
    }

    pub(super) fn wake(&self) {
        self.0.snapshot.lock().expect("card reader").dirty = true;
        self.0.stop.notify_one();
    }
}

impl Drop for CardReader {
    fn drop(&mut self) {
        self.0.snapshot.lock().expect("card reader").owner = None;
        self.0.stop.notify_one();
    }
}

impl Bars {
    pub(super) fn refresh_desktop_cards(&mut self) -> Result<()> {
        if !self.settings.enabled(Key::DesktopCards) {
            self.card_reader = None;
            self.card_data.clear();
        } else if self.card_reader.is_none() {
            self.card_reader = Some(CardReader::start(self.top.0)?);
        }
        match self.paint_desktop_cards() {
            Err(error) if is_device_lost(&error) => self.rebuild_renderer(),
            result => result,
        }
    }

    pub(super) fn wake_card_reader(&self) {
        if let Some(reader) = &self.card_reader {
            reader.wake();
        }
    }

    pub(super) fn receive_cards(&mut self) -> Result<()> {
        if let Some(reader) = &self.card_reader {
            self.card_data
                .clone_from(&reader.0.snapshot.lock().expect("card reader").cards);
        }
        self.sync_plugins_if_open();
        self.refresh_desktop_cards()
    }

    pub(super) fn paint_desktop_cards(&mut self) -> Result<()> {
        if self.fullscreen || self.card_data.is_empty() {
            self.desktop_cards.hide();
            return Ok(());
        }
        let renderer = self.renderer.as_mut().expect("renderer bound to bars");
        self.desktop_cards.update(
            renderer,
            &self.card_data,
            self.monitor.work,
            self.monitor.dpi,
        )
    }
}
