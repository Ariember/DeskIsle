use std::io;
use std::sync::Arc;
use std::thread::JoinHandle;

use super::Bars;
use crate::integrations::market::Market;
use crate::platform;
use crate::storage;
use crate::ui::panel::{Action, MarketItem, MarketView};
use windows::Win32::Foundation::E_FAIL;
use windows::core::{Error, Result};

enum Completion {
    Catalog(Arc<Market>, Vec<MarketItem>),
    Installed(usize),
}

#[derive(Default)]
pub(super) struct PluginMarket {
    catalog: Option<Arc<Market>>,
    pub(super) view: MarketView,
    worker: Option<JoinHandle<io::Result<Completion>>>,
}

impl PluginMarket {
    fn start(&mut self, action: Action) -> io::Result<()> {
        let catalog = self.catalog.clone();
        self.worker = Some(
            std::thread::Builder::new()
                .name("DeskIsle plugin market".into())
                .spawn(move || {
                    let directory = storage::plugins_directory().map_err(io::Error::other)?;
                    match action {
                        Action::BrowseMarket => {
                            let market = Market::load()?;
                            let items = market
                                .entries
                                .iter()
                                .map(|entry| {
                                    let (installed, description) = match entry
                                        .is_installed(&directory)
                                    {
                                        Ok(installed) => (installed, entry.description.clone()),
                                        Err(error) => (false, format!("无法读取本地插件：{error}")),
                                    };
                                    MarketItem {
                                        title: format!("{} · {}", entry.name, entry.version),
                                        description,
                                        installed,
                                    }
                                })
                                .collect();
                            Ok(Completion::Catalog(Arc::new(market), items))
                        }
                        Action::InstallPlugin(index) => {
                            let catalog =
                                catalog.expect("installation requires a displayed catalog");
                            catalog.install(&catalog.entries[index].id, &directory)?;
                            Ok(Completion::Installed(index))
                        }
                        _ => unreachable!("only market actions start jobs"),
                    }
                })?,
        );
        self.view.busy = Some(action);
        Ok(())
    }
}

impl Drop for PluginMarket {
    fn drop(&mut self) {
        // Bars restores the desktop before dropping this field. A user-requested
        // install can finish its atomic publication even if the panel was closed.
        if matches!(self.view.busy, Some(Action::InstallPlugin(_)))
            && let Some(worker) = self.worker.take()
        {
            drop(worker.join());
        }
    }
}

impl Bars {
    pub(super) fn start_market(&mut self, action: Action) {
        if self.market.worker.is_some() {
            return;
        }
        let result = platform::market_timer(self.top.0, true).and_then(|()| {
            self.market
                .start(action)
                .map_err(|e| Error::new(E_FAIL, format!("无法启动插件市场：{e}")))
        });
        if let Err(error) = result {
            let _ = platform::market_timer(self.top.0, false);
            platform::report_error(Some(self.settings_owner()), &error);
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
        platform::market_timer(self.top.0, false)?;
        let worker = self.market.worker.take().expect("finished market job");
        let action = self.market.view.busy.take();
        match worker
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("插件市场线程意外退出")))
        {
            Ok(Completion::Catalog(catalog, items)) => {
                self.market.catalog = Some(catalog);
                self.market.view.items = items.into();
                self.market.view.loaded = true;
            }
            Ok(Completion::Installed(index)) => {
                let item = &mut Arc::make_mut(&mut self.market.view.items)[index];
                item.installed = true;
                if let Some(catalog) = &self.market.catalog {
                    item.description
                        .clone_from(&catalog.entries[index].description);
                }
            }
            Err(error) => {
                let operation = if action == Some(Action::BrowseMarket) {
                    "读取插件市场失败"
                } else {
                    "安装插件失败"
                };
                platform::report_error(
                    Some(self.settings_owner()),
                    &Error::new(E_FAIL, format!("{operation}：{error}")),
                );
            }
        }
        self.relayout_panel()
    }
}
