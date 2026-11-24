use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::cards::{Card, CardState};
use crate::plugins::{self, Contribution, MAX_DOCUMENT_BYTES, Publication, valid_plugin_id};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LocalPluginStatus {
    Ready {
        cards: usize,
        setup_cards: usize,
        error_cards: usize,
    },
    Invalid(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalPlugin {
    pub id: String,
    pub removable: bool,
    pub name: String,
    pub version: Option<semver::Version>,
    pub card_titles: Vec<String>,
    pub status: LocalPluginStatus,
}

/// Inspects every `.json` publication in `dir` in plugin ID order for the management panel.
pub(crate) fn list_installed(dir: &Path) -> io::Result<Vec<LocalPlugin>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        match entry.file_type() {
            Ok(ty) if ty.is_file() => paths.push(path),
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    paths.sort_unstable_by(|a, b| a.file_stem().cmp(&b.file_stem()));
    let mut installed = Vec::new();
    for path in paths {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let valid_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(valid_plugin_id);
        if !valid_id {
            installed.push(LocalPlugin {
                name: stem.clone(),
                id: stem,
                removable: false,
                version: None,
                card_titles: Vec::new(),
                status: LocalPluginStatus::Invalid(
                    "文件名不符合插件 ID 规范，请在插件目录中处理".into(),
                ),
            });
            continue;
        }
        match read_document(&path) {
            Ok(plugin) => {
                let mut cards = 0;
                let mut setup_cards = 0;
                let mut error_cards = 0;
                let mut card_titles = Vec::new();
                for contribution in plugin.contributions {
                    match contribution {
                        Contribution::DesktopCards(entries) => {
                            for card in entries {
                                cards += 1;
                                match card.state {
                                    CardState::Ready => {}
                                    CardState::Setup => setup_cards += 1,
                                    CardState::Error => error_cards += 1,
                                }
                                if card_titles.len() < 3 {
                                    card_titles.push(card.title);
                                }
                            }
                        }
                    }
                }
                installed.push(LocalPlugin {
                    id: stem,
                    removable: true,
                    name: plugin.name,
                    version: Some(plugin.version),
                    card_titles,
                    status: LocalPluginStatus::Ready {
                        cards,
                        setup_cards,
                        error_cards,
                    },
                });
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => installed.push(LocalPlugin {
                name: stem.clone(),
                id: stem,
                removable: true,
                version: None,
                card_titles: Vec::new(),
                status: LocalPluginStatus::Invalid(e.to_string()),
            }),
        }
    }
    Ok(installed)
}

/// Load completed publications in plugin ID order, preserving each author's card order.
pub(crate) fn load_cards(dir: &Path) -> Vec<Card> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(_) => return vec![error_card("插件目录", "无法读取插件目录")],
    };
    let mut paths = Vec::new();
    let mut incomplete = false;
    for entry in entries {
        let Ok(entry) = entry else {
            incomplete = true;
            continue;
        };
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        match entry.file_type() {
            Ok(ty) if ty.is_file() => paths.push(path),
            Err(e) if e.kind() != ErrorKind::NotFound => incomplete = true,
            _ => {}
        }
    }
    // Compare stems rather than full filenames: "a" sorts before "a-b".
    paths.sort_unstable_by(|a, b| a.file_stem().cmp(&b.file_stem()));
    let mut cards = Vec::new();
    for path in paths {
        let Some(id) = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|id| valid_plugin_id(id))
        else {
            cards.push(error_card("插件文件", "文件名不符合插件 ID 规范"));
            continue;
        };
        match read_document(&path) {
            Ok(plugin) => {
                for contribution in plugin.contributions {
                    match contribution {
                        Contribution::DesktopCards(entries) => cards.extend(entries),
                    }
                }
            }
            // A plugin can withdraw its cards between enumeration and opening.
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => cards.push(error_card(id, &e.to_string())),
        }
    }
    if incomplete {
        cards.push(error_card("插件目录", "部分插件文件无法枚举"));
    }
    cards
}

fn error_card(title: &str, detail: &str) -> Card {
    Card {
        title: title.into(),
        value: "暂不可用".into(),
        detail: detail.into(),
        state: CardState::Error,
    }
}

fn read_document(path: &Path) -> io::Result<Publication> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    plugins::parse(&bytes).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

pub(crate) fn validate(id: &str, bytes: &[u8]) -> io::Result<Publication> {
    check_id(id)?;
    plugins::parse(bytes).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

pub(crate) fn publish(dir: &Path, id: &str, bytes: &[u8]) -> io::Result<()> {
    publish_checked(dir, id, bytes, |_| Ok(()))
}

pub(super) fn publish_checked(
    dir: &Path,
    id: &str,
    bytes: &[u8],
    check: impl FnOnce(&Publication) -> io::Result<()>,
) -> io::Result<()> {
    check(&validate(id, bytes)?)?;
    let target = dir.join(format!("{id}.json"));
    fs::create_dir_all(dir)?;
    let (temp, mut file) = create_temp(dir, id)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| fs::rename(&temp, target));
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub(crate) fn remove(dir: &Path, id: &str) -> io::Result<()> {
    check_id(id)?;
    let path = dir.join(format!("{id}.json"));
    match fs::remove_file(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn check_id(id: &str) -> io::Result<()> {
    if !valid_plugin_id(id) {
        return Err(io::Error::new(ErrorKind::InvalidInput, "插件 ID 无效"));
    }
    Ok(())
}

fn create_temp(dir: &Path, id: &str) -> io::Result<(PathBuf, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".{id}.{}.{serial}.tmp", std::process::id()));
        match File::create_new(&path) {
            Ok(file) => return Ok((path, file)),
            // A previous writer may have exited before renaming its temporary file.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_replacement_failure_isolation_and_withdrawal() {
        let workspace = std::env::temp_dir().join(format!(
            "deskisle-cards-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&workspace).unwrap();
        let dir = workspace.join("plugins");
        let multiple = br#"{"protocol":1,"name":"Test","version":"1.0.0","contributions":[{"point":"desktop.cards","version":1,"data":{"cards":[{"title":"one","value":"1"},{"title":"two","value":"2"}]}}]}"#;
        let empty = br#"{"protocol":1,"name":"Test","version":"1.0.0","contributions":[]}"#;
        assert!(load_cards(&dir).is_empty());
        publish(&dir, "b", multiple).unwrap();
        publish(&dir, "a", empty).unwrap();
        assert!(publish(&dir, "b", b"invalid").is_err());
        fs::write(dir.join("a-b.json"), b"invalid").unwrap();
        fs::write(dir.join("hidden.tmp"), b"invalid").unwrap();
        publish(
            &dir,
            "a",
            br#"{"protocol":1,"name":"Test","version":"1.0.0","contributions":[{"point":"desktop.cards","version":1,"data":{"cards":[{"title":"first","value":"3"}]}}]}"#,
        )
        .unwrap();
        let cards = load_cards(&dir);
        assert_eq!(
            cards.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(),
            ["first", "a-b", "one", "two"]
        );
        assert_eq!(cards[1].state, CardState::Error);
        let listed = list_installed(&dir).unwrap();
        assert_eq!(
            listed.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["a", "a-b", "b"]
        );
        assert_eq!(
            listed[0].status,
            LocalPluginStatus::Ready {
                cards: 1,
                setup_cards: 0,
                error_cards: 0,
            }
        );
        assert!(matches!(listed[1].status, LocalPluginStatus::Invalid(_)));
        assert!(listed[1].removable);
        assert!(publish(&dir, "../outside", empty).is_err());
        assert!(remove(&dir, "../outside").is_err());
        remove(&dir, "b").unwrap();
        remove(&dir, "b").unwrap();
        publish(&dir, "a", empty).unwrap();
        assert_eq!(load_cards(&dir), vec![cards[1].clone()]);
        remove(&dir, "a").unwrap();
        remove(&dir, "a-b").unwrap();
        fs::remove_file(dir.join("hidden.tmp")).unwrap();
        fs::remove_dir(dir).unwrap();
        fs::remove_dir(workspace).unwrap();
    }
}
