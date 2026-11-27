//! Reflect installed plugins' local snapshots without reading plugin files on the UI thread.

use super::Bars;
use crate::cards::{Card, CardState};
use crate::integrations::{magpie, plugins::load_cards};
use crate::platform;
use crate::render::is_device_lost;
use crate::settings::Key;
use crate::storage;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::core::{Error, Result};

struct Snapshot {
    owner: Option<usize>,
    dirty: bool,
    cards: Vec<Card>,
    magpie: Option<Vec<Card>>,
}

struct Shared {
    snapshot: Mutex<Snapshot>,
    stop: Condvar,
}

/// Owned by one card reader. A retired reader drops its results; the bounded
/// gateway request never owns the window or writes into the plugin directory.
struct MagpiePoll {
    request: Option<JoinHandle<Vec<Card>>>,
    next: Instant,
    cards: Vec<Card>,
}

impl MagpiePoll {
    fn new(now: Instant) -> Self {
        Self {
            request: None,
            next: now,
            cards: vec![Card::simple(
                "Magpie 数据面板",
                "正在连接",
                "正在查询本机 Magpie 网关",
                CardState::Setup,
            )],
        }
    }

    fn refresh(
        &mut self,
        now: Instant,
        query: impl FnOnce() -> Vec<Card> + Send + 'static,
    ) -> Vec<Card> {
        let retry = Duration::from_secs(15);
        if self.request.as_ref().is_some_and(JoinHandle::is_finished) {
            self.cards = self.request.take().unwrap().join().unwrap_or_else(|_| {
                vec![Card::simple(
                    "Magpie 数据面板",
                    "同步失败",
                    "稍后自动重试",
                    CardState::Error,
                )]
            });
            self.next = now + retry;
        }
        if self.request.is_none() && now >= self.next {
            match std::thread::Builder::new()
                .name("DeskIsle Magpie".into())
                .spawn(query)
            {
                Ok(request) => self.request = Some(request),
                Err(_) => {
                    self.cards = vec![Card::simple(
                        "Magpie 数据面板",
                        "无法启动查询",
                        "稍后自动重试",
                        CardState::Error,
                    )];
                    self.next = now + retry;
                }
            }
        }
        self.cards.clone()
    }
}

pub(super) struct CardReader(Arc<Shared>);

impl CardReader {
    fn start(hwnd: HWND) -> Result<Self> {
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot {
                owner: Some(hwnd.0 as usize),
                dirty: false,
                cards: Vec::new(),
                magpie: None,
            }),
            stop: Condvar::new(),
        });
        let reader = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("DeskIsle desktop cards".into())
            .spawn(move || {
                let mut poll = None;
                loop {
                    reader.snapshot.lock().expect("card reader").dirty = false;
                    let mut live = None;
                    let cards = match storage::plugins_directory() {
                        Ok(directory) => load_cards(&directory, || {
                            if platform::cards::external_magpie_active() {
                                return None;
                            }
                            let now = Instant::now();
                            let poll = poll.get_or_insert_with(|| MagpiePoll::new(now));
                            live = Some(poll.refresh(now, magpie::read_cards));
                            live.clone()
                        }),
                        Err(_) => vec![Card::simple(
                            "桌面卡片",
                            "目录不可用",
                            "请检查当前用户的应用数据目录",
                            CardState::Error,
                        )],
                    };
                    if live.is_none() {
                        poll = None;
                    }
                    let mut snapshot = reader.snapshot.lock().expect("card reader");
                    let Some(owner) = snapshot.owner else { return };
                    snapshot.magpie = live;
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

    pub(super) fn magpie_cards(&self) -> Option<Vec<Card>> {
        self.0.snapshot.lock().expect("card reader").magpie.clone()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn magpie_publishes_completed_queries_and_retries_after_the_interval() {
        let now = Instant::now();
        let mut poll = MagpiePoll::new(now);
        let (finish, gate) = mpsc::channel();
        let query = move || {
            gate.recv().unwrap();
            vec![Card::simple("Magpie", "离线", "", CardState::Error)]
        };
        assert_eq!(poll.refresh(now, query)[0].state, CardState::Setup);
        assert_eq!(
            poll.refresh(now, || panic!("duplicate query"))[0].state,
            CardState::Setup
        );
        finish.send(()).unwrap();
        wait_for_query(&poll);
        assert_eq!(
            poll.refresh(now, || panic!("early retry"))[0].state,
            CardState::Error
        );
        let later = now + Duration::from_secs(15);
        poll.refresh(later, || {
            vec![Card::simple("Magpie", "已恢复", "", CardState::Ready)]
        });
        wait_for_query(&poll);
        assert_eq!(
            poll.refresh(later, || panic!("early retry"))[0].value,
            "已恢复"
        );
    }

    fn wait_for_query(poll: &MagpiePoll) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !poll.request.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
