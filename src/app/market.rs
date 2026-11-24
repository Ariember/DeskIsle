use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;

use semver::Version;
use windows::Win32::Foundation::E_FAIL;
use windows::core::{Error, Result};

use super::Bars;
use crate::integrations::market::Market;
use crate::integrations::plugins::{self, LocalPlugin, LocalPluginStatus};
use crate::platform;
use crate::settings::{SECTIONS, Section};
use crate::storage;
use crate::ui::panel::{
    Action, InstalledHealth, InstalledItem, MarketItem, MarketState, MarketView,
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Job {
    ScanLocal,
    Browse,
    Install(usize),
    Remove(String, usize),
}

impl Job {
    fn busy_action(&self) -> Option<Action> {
        match *self {
            Self::ScanLocal => None,
            Self::Browse => Some(Action::BrowseMarket),
            Self::Install(index) => Some(Action::InstallPlugin(index)),
            Self::Remove(_, index) => Some(Action::RemovePlugin(index)),
        }
    }

    fn mutates_disk(&self) -> bool {
        matches!(self, Self::Install(_) | Self::Remove(..))
    }
}

struct Completion {
    catalog: Option<Arc<Market>>,
    installed: Vec<InstalledItem>,
    installed_ids: Vec<String>,
    items: Option<Vec<MarketItem>>,
}

#[derive(Default)]
pub(super) struct PluginMarket {
    catalog: Option<Arc<Market>>,
    installed_ids: Vec<String>,
    pub(super) view: MarketView,
    active: Option<Job>,
    queued: Option<Job>,
    auto_browsed: bool,
    worker: Option<JoinHandle<io::Result<Completion>>>,
}

impl PluginMarket {
    fn start(&mut self, job: Job) -> io::Result<()> {
        let catalog = self.catalog.clone();
        let active_job = job.clone();
        self.worker = Some(
            std::thread::Builder::new()
                .name("DeskIsle plugin market".into())
                .spawn(move || {
                    let directory = storage::plugins_directory().map_err(io::Error::other)?;
                    match active_job {
                        Job::ScanLocal => {
                            let local = plugins::list_installed(&directory)?;
                            let (installed, installed_ids, items) =
                                reconcile(&directory, &local, catalog.as_deref());
                            Ok(Completion {
                                catalog: None,
                                installed,
                                installed_ids,
                                items,
                            })
                        }
                        Job::Browse => {
                            let market = Arc::new(Market::load()?);
                            let local = plugins::list_installed(&directory)?;
                            let (installed, installed_ids, items) =
                                reconcile(&directory, &local, Some(&market));
                            Ok(Completion {
                                catalog: Some(market),
                                installed,
                                installed_ids,
                                items,
                            })
                        }
                        Job::Install(index) => {
                            let catalog =
                                catalog.expect("installation requires a displayed catalog");
                            let entry = catalog.entries.get(index).ok_or_else(|| {
                                io::Error::new(io::ErrorKind::NotFound, "未找到所选插件")
                            })?;
                            catalog.install(&entry.id, &directory)?;
                            let local = plugins::list_installed(&directory)?;
                            let (installed, installed_ids, items) =
                                reconcile(&directory, &local, Some(&catalog));
                            Ok(Completion {
                                catalog: None,
                                installed,
                                installed_ids,
                                items,
                            })
                        }
                        Job::Remove(id, _) => {
                            plugins::remove(&directory, &id)?;
                            let local = plugins::list_installed(&directory)?;
                            let (installed, installed_ids, items) =
                                reconcile(&directory, &local, catalog.as_deref());
                            Ok(Completion {
                                catalog: None,
                                installed,
                                installed_ids,
                                items,
                            })
                        }
                    }
                })?,
        );
        self.view.busy = job.busy_action();
        self.active = Some(job);
        Ok(())
    }
}

fn reconcile(
    directory: &Path,
    local: &[LocalPlugin],
    catalog: Option<&Market>,
) -> (Vec<InstalledItem>, Vec<String>, Option<Vec<MarketItem>>) {
    let local_by_id: HashMap<&str, &LocalPlugin> =
        local.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut updates_by_id: HashMap<&str, (usize, String)> = HashMap::new();
    let mut descriptions_by_id: HashMap<&str, &str> = HashMap::new();

    let market_items = catalog.map(|market| {
        market
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                descriptions_by_id.insert(entry.id.as_str(), entry.description.as_str());
                let Some(loc) = local_by_id.get(entry.id.as_str()) else {
                    return MarketItem {
                        id: entry.id.clone(),
                        name: entry.name.clone(),
                        version: entry.version.clone(),
                        installed_version: None,
                        description: entry.description.clone(),
                        state: MarketState::Install,
                    };
                };
                let installed_version = loc.version.as_ref().map(ToString::to_string);
                let (state, description) = match entry.is_installed(directory) {
                    Ok(true) => (MarketState::Installed, entry.description.clone()),
                    Ok(false) => {
                        let newer = loc
                            .version
                            .as_ref()
                            .zip(Version::parse(&entry.version).ok().as_ref())
                            .is_some_and(|(lv, mv)| mv > lv);
                        if newer {
                            updates_by_id.insert(entry.id.as_str(), (index, entry.version.clone()));
                            (MarketState::Update, entry.description.clone())
                        } else {
                            (MarketState::Reinstall, entry.description.clone())
                        }
                    }
                    Err(error) => (MarketState::Reinstall, format!("无法读取本地插件：{error}")),
                };
                MarketItem {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                    version: entry.version.clone(),
                    installed_version,
                    description,
                    state,
                }
            })
            .collect()
    });

    let mut installed_ids = Vec::with_capacity(local.len());
    let installed = local
        .iter()
        .map(|loc| {
            installed_ids.push(loc.id.clone());
            let (cards, health, summary) = match &loc.status {
                LocalPluginStatus::Invalid(reason) => (0, InstalledHealth::Error, reason.clone()),
                LocalPluginStatus::Ready {
                    cards,
                    setup_cards,
                    error_cards,
                } => {
                    let health = if *error_cards > 0 {
                        InstalledHealth::Error
                    } else if *setup_cards > 0 {
                        InstalledHealth::Setup
                    } else {
                        InstalledHealth::Ready
                    };
                    let summary = if *error_cards > 0 {
                        format!("{cards} 张卡片 · {error_cards} 张数据不可用")
                    } else if *setup_cards > 0 {
                        format!("{cards} 张卡片 · {setup_cards} 张等待配置")
                    } else if *cards == 0 {
                        if let Some(desc) = descriptions_by_id.get(loc.id.as_str()) {
                            format!("当前无活动卡片 · {desc}")
                        } else {
                            format!("ID: {} · 当前未发布卡片", loc.id)
                        }
                    } else {
                        format!("{cards} 张卡片 · {}", loc.card_titles.join("、"))
                    };
                    (*cards, health, summary)
                }
            };
            InstalledItem {
                id: loc.id.clone(),
                title: loc.name.clone(),
                version: loc.version.as_ref().map(ToString::to_string),
                market_update: updates_by_id.get(loc.id.as_str()).cloned(),
                summary,
                cards,
                health,
                removable: loc.removable,
            }
        })
        .collect();

    (installed, installed_ids, market_items)
}

impl Drop for PluginMarket {
    fn drop(&mut self) {
        // Bars restores the desktop before dropping this field. A user-requested
        // install or removal can finish its atomic file operation even if closed.
        if self.active.as_ref().is_some_and(Job::mutates_disk)
            && let Some(worker) = self.worker.take()
        {
            drop(worker.join());
        }
    }
}

impl Bars {
    pub(super) fn enter_plugins_page(&mut self) {
        let should_browse = !self.market.auto_browsed && !self.market.view.loaded;
        if should_browse {
            self.market.auto_browsed = true;
        }
        if self.market.worker.is_some() {
            if should_browse {
                self.market.queued = Some(Job::Browse);
            } else if self.market.queued.is_none() {
                self.market.queued = Some(Job::ScanLocal);
            }
            return;
        }
        if should_browse {
            self.market.queued = Some(Job::Browse);
        }
        self.launch_market_job(Job::ScanLocal);
    }

    pub(super) fn sync_plugins_if_open(&mut self) {
        let on_plugins = self
            .panel
            .as_ref()
            .is_some_and(|panel| SECTIONS[panel.page()] == Section::Plugins);
        if !on_plugins {
            return;
        }
        if self.market.worker.is_some() {
            if self.market.queued.is_none() {
                self.market.queued = Some(Job::ScanLocal);
            }
        } else {
            self.launch_market_job(Job::ScanLocal);
        }
    }

    pub(super) fn start_market(&mut self, action: Action) {
        if self.market.worker.is_some() {
            return;
        }
        let job = match action {
            Action::BrowseMarket => {
                self.market.auto_browsed = true;
                Job::Browse
            }
            Action::InstallPlugin(index) => Job::Install(index),
            Action::RemovePlugin(index) => {
                let Some(id) = self.market.installed_ids.get(index).cloned() else {
                    return;
                };
                Job::Remove(id, index)
            }
            _ => return,
        };
        self.launch_market_job(job);
    }

    fn launch_market_job(&mut self, job: Job) {
        let report_failure = job != Job::ScanLocal;
        let result = platform::market_timer(self.top.0, true).and_then(|()| {
            self.market
                .start(job)
                .map_err(|e| Error::new(E_FAIL, format!("无法启动插件任务：{e}")))
        });
        if let Err(error) = result {
            let _ = platform::market_timer(self.top.0, false);
            if report_failure {
                platform::report_error(Some(self.settings_owner()), &error);
            }
        }
    }

    pub(super) fn market_progress(&mut self) -> Result<()> {
        if !self
            .market
            .worker
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            return Ok(());
        }
        let worker = self.market.worker.take().expect("finished market job");
        let finished_job = self.market.active.take();
        self.market.view.busy = None;
        let mutated = finished_job.as_ref().is_some_and(Job::mutates_disk);
        match worker
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("插件后台线程意外退出")))
        {
            Ok(completion) => {
                if let Some(catalog) = completion.catalog {
                    self.market.view.source = Some(catalog.label.clone());
                    self.market.view.loaded = true;
                    self.market.view.error = None;
                    self.market.catalog = Some(catalog);
                }
                self.market.installed_ids = completion.installed_ids;
                self.market.view.installed = completion.installed.into();
                if let Some(items) = completion.items {
                    self.market.view.items = items.into();
                }
                if mutated {
                    self.wake_card_reader();
                }
            }
            Err(error) => match finished_job {
                Some(Job::Browse) => {
                    self.market.view.error = Some(error.to_string());
                }
                Some(Job::Install(_)) => {
                    platform::report_error(
                        Some(self.settings_owner()),
                        &Error::new(E_FAIL, format!("安装插件失败：{error}")),
                    );
                }
                Some(Job::Remove(..)) => {
                    platform::report_error(
                        Some(self.settings_owner()),
                        &Error::new(E_FAIL, format!("移除插件失败：{error}")),
                    );
                }
                _ => {}
            },
        }
        if let Some(next) = self.market.queued.take() {
            if let Err(error) = self.market.start(next) {
                let _ = platform::market_timer(self.top.0, false);
                platform::report_error(
                    Some(self.settings_owner()),
                    &Error::new(E_FAIL, format!("无法启动插件任务：{error}")),
                );
            }
        } else {
            platform::market_timer(self.top.0, false)?;
        }
        self.relayout_panel()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::market::MarketEntry;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::fs;

    #[test]
    fn reconcile_distinguishes_install_installed_update_reinstall_and_broken_files() {
        let dir = std::env::temp_dir().join(format!("deskisle-reconcile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let doc_a = r#"{"protocol":1,"name":"快捷速查","version":"1.0.0","contributions":[{"point":"desktop.cards","version":1,"data":{"cards":[{"title":"显示桌面","value":"Win+D"}]}}]}"#.as_bytes();
        let doc_b = r#"{"protocol":1,"name":"待办清单","version":"1.0.0","contributions":[{"point":"desktop.cards","version":1,"data":{"cards":[{"title":"配置","value":"待登录","state":"setup"}]}}]}"#.as_bytes();
        plugins::publish(&dir, "alpha", doc_a).unwrap();
        plugins::publish(&dir, "beta", doc_b).unwrap();
        fs::write(dir.join("corrupt.json"), b"not-json").unwrap();

        let hash_a = format!("{:x}", Sha256::digest(doc_a));
        let entries: Vec<MarketEntry> = serde_json::from_value(json!([
            {"id":"alpha","name":"快捷速查","version":"1.0.0","description":"常用快捷键","sha256":hash_a},
            {"id":"beta","name":"待办清单","version":"1.2.0","description":"桌面待办","sha256":"0".repeat(64)},
            {"id":"corrupt","name":"损坏恢复","version":"1.0.0","description":"可重装修复","sha256":"1".repeat(64)},
            {"id":"fresh","name":"新卡片","version":"0.1.0","description":"尚未安装","sha256":"2".repeat(64)}
        ]))
        .unwrap();
        let market = Market::test_with_entries(dir.join("index.json"), entries);

        let local = plugins::list_installed(&dir).unwrap();
        let (installed, ids, items) = reconcile(&dir, &local, Some(&market));
        let items = items.unwrap();
        assert_eq!(ids, ["alpha", "beta", "corrupt"]);
        assert_eq!(items[0].state, MarketState::Installed);
        assert_eq!(items[1].state, MarketState::Update);
        assert_eq!(items[2].state, MarketState::Reinstall);
        assert_eq!(items[3].state, MarketState::Install);

        assert_eq!(installed[0].health, InstalledHealth::Ready);
        assert_eq!(installed[0].summary, "1 张卡片 · 显示桌面");
        assert_eq!(installed[1].health, InstalledHealth::Setup);
        assert_eq!(installed[1].market_update, Some((1, "1.2.0".into())));
        assert_eq!(installed[2].health, InstalledHealth::Error);
        assert!(installed[2].removable);

        fs::remove_dir_all(dir).unwrap();
    }
}
