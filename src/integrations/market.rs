//! Install versioned publications from a static local or HTTPS catalog.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use ureq::{Agent, http::Uri};

use super::plugins;
use crate::plugins::{MAX_DOCUMENT_BYTES, valid_plugin_id};

const MARKET_REPOSITORY: &str = "Ariember/DeskIsle-PluginMarket";
const MAX_INDEX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
enum Source {
    File(PathBuf),
    Https(Uri),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MarketEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    schema: u32,
    plugins: Vec<MarketEntry>,
}

impl MarketEntry {
    pub fn is_installed(&self, directory: &Path) -> io::Result<bool> {
        let path = directory.join(format!("{}.json", self.id));
        let file = match File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err),
        };
        let mut bytes = Vec::new();
        file.take(MAX_DOCUMENT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes.len() <= MAX_DOCUMENT_BYTES && self.matches(&bytes))
    }

    fn matches(&self, bytes: &[u8]) -> bool {
        format!("{:x}", Sha256::digest(bytes)) == self.sha256
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Market {
    source: Source,
    pub entries: Vec<MarketEntry>,
}

impl Market {
    pub fn load() -> io::Result<Self> {
        let source = match std::env::var_os("DESKISLE_MARKET_INDEX") {
            Some(value) => Source::configured(value)?,
            None => Source::latest_release()?,
        };
        Self::from_source(source)
    }

    fn from_source(source: Source) -> io::Result<Self> {
        let bytes = source.read(MAX_INDEX_BYTES)?;
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let index: Index =
            serde_json::from_slice(bytes).map_err(|_| invalid("市场索引 JSON 格式无效"))?;
        if index.schema != 1 {
            return Err(invalid("不支持此市场索引版本"));
        }
        let mut ids = HashSet::new();
        for entry in &index.plugins {
            if !valid_plugin_id(&entry.id) || !ids.insert(&entry.id) {
                return Err(invalid("市场索引包含无效或重复的插件 ID"));
            }
            if !catalog_text(&entry.name, 64)
                || !catalog_text(&entry.description, 256)
                || Version::parse(&entry.version).is_err()
                || entry.sha256.len() != 64
                || !entry
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(invalid("市场索引中的插件信息无效"));
            }
        }
        Ok(Self {
            entries: index.plugins,
            source,
        })
    }

    pub fn install(&self, id: &str, directory: &Path) -> io::Result<()> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "市场索引中没有此插件"))?;
        let bytes = self.source.package(&entry.id)?.read(MAX_DOCUMENT_BYTES)?;
        if !entry.matches(&bytes) {
            return Err(invalid("插件包未通过 SHA-256 完整性校验"));
        }
        plugins::publish_checked(directory, id, &bytes, |publication| {
            if publication.name != entry.name || publication.version.to_string() != entry.version {
                return Err(invalid("插件包名称或版本与市场索引不一致"));
            }
            Ok(())
        })
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

fn catalog_text(text: &str, limit: usize) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= limit
        && !text
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
}

impl Source {
    fn configured(value: OsString) -> io::Result<Self> {
        if value.is_empty() {
            return Err(invalid("DESKISLE_MARKET_INDEX 为空"));
        }
        let Some(url) = value.to_str().filter(|s| s.contains("://")) else {
            return Ok(Self::File(value.into()));
        };
        let uri: Uri = url.parse().map_err(|_| invalid("市场索引 URL 无效"))?;
        if uri.scheme_str() != Some("https")
            || uri.authority().is_none_or(|a| a.as_str().contains('@'))
            || url.contains('#')
            || uri.path().ends_with('/')
        {
            return Err(invalid("市场索引需要使用指向文件的 HTTPS 地址"));
        }
        Ok(Self::Https(uri))
    }

    fn latest_release() -> io::Result<Self> {
        let api = format!("https://api.github.com/repos/{MARKET_REPOSITORY}/releases/latest");
        let bytes = fetch_json(&api, MAX_INDEX_BYTES, "application/vnd.github+json")?;
        Self::release_index(&bytes)
    }

    fn release_index(bytes: &[u8]) -> io::Result<Self> {
        #[derive(Deserialize)]
        struct Release {
            tag_name: String,
            draft: bool,
            prerelease: bool,
        }
        let release: Release =
            serde_json::from_slice(bytes).map_err(|_| invalid("GitHub 市场发布信息格式无效"))?;
        let version = release
            .tag_name
            .strip_prefix('v')
            .and_then(|tag| Version::parse(tag).ok())
            .ok_or_else(|| invalid("市场发布标签需要采用 v 加语义化版本号"))?;
        if release.draft || release.prerelease || !version.pre.is_empty() {
            return Err(invalid("市场发布版本需要是已公开的稳定版"));
        }
        let url =
            format!("https://raw.githubusercontent.com/{MARKET_REPOSITORY}/v{version}/index.json");
        Self::configured(url.into())
    }

    fn package(&self, id: &str) -> io::Result<Self> {
        let relative = format!("packages/{id}.json");
        match self {
            Self::File(path) => Ok(Self::File(path.with_file_name(relative))),
            Self::Https(index) => {
                let directory = index.path().rsplit_once('/').map_or("", |(dir, _)| dir);
                let uri = Uri::builder()
                    .scheme("https")
                    .authority(
                        index
                            .authority()
                            .expect("validated HTTPS authority")
                            .clone(),
                    )
                    .path_and_query(format!("{directory}/{relative}"))
                    .build()
                    .map_err(|_| invalid("市场插件包 URL 无效"))?;
                Ok(Self::Https(uri))
            }
        }
    }

    fn read(&self, limit: usize) -> io::Result<Vec<u8>> {
        match self {
            Self::File(path) => {
                let mut bytes = Vec::new();
                File::open(path)?
                    .take(limit as u64 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > limit {
                    return Err(invalid("市场文件过大"));
                }
                Ok(bytes)
            }
            Self::Https(uri) => fetch_json(&uri.to_string(), limit, "application/json"),
        }
    }
}

fn fetch_json(url: &str, limit: usize, accept: &str) -> io::Result<Vec<u8>> {
    let agent: Agent = Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .user_agent(concat!("DeskIsle/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("Accept", accept)
        .call()
        .map_err(|err| match err {
            ureq::Error::StatusCode(status) => io::Error::other(format!("市场返回 HTTP {status}")),
            ureq::Error::Timeout(_) => io::Error::new(ErrorKind::TimedOut, "连接插件市场超时"),
            _ => io::Error::other("无法连接插件市场，请检查网络或市场地址"),
        })?;
    if response.status() != 200 {
        return Err(io::Error::other(format!(
            "市场返回 HTTP {}",
            response.status()
        )));
    }
    response
        .body_mut()
        .with_config()
        .limit(limit as u64)
        .read_to_vec()
        .map_err(|_| io::Error::other("市场文件读取失败或超过大小限制"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn installs_indexed_package_and_preserves_it_on_failed_updates() {
        let root = std::env::temp_dir().join(format!("deskisle-market-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let index = root.join("index.json");
        let packages = root.join("packages");
        fs::create_dir(&packages).unwrap();
        let installed = root.join("installed");
        let package = packages.join("example.json");
        let document = br#"{"protocol":1,"name":"Example","version":"1.0.0","contributions":[{"point":"desktop.cards","version":1,"data":{"cards":[{"title":"Example","value":"Ready"}]}}]}"#;
        let hash = format!("{:x}", Sha256::digest(document));
        let entry = json!({"id":"example","name":"Example","version":"1.0.0","description":"Sample","sha256":hash});
        let write_index = |entries| {
            fs::write(
                &index,
                serde_json::to_vec(&json!({"schema":1,"plugins":entries})).unwrap(),
            )
            .unwrap();
            Market::from_source(Source::File(index.clone()))
        };
        assert!(write_index(json!([entry, entry])).is_err());
        let mut invalid_entry = entry.clone();
        invalid_entry["id"] = json!("../outside");
        assert!(write_index(json!([invalid_entry])).is_err());
        let market = write_index(json!([entry])).unwrap();
        assert!(!market.entries[0].is_installed(&installed).unwrap());
        assert!(market.install("example", &installed).is_err());
        assert!(!installed.exists());
        fs::write(&package, document).unwrap();
        market.install("example", &installed).unwrap();
        assert!(market.entries[0].is_installed(&installed).unwrap());
        assert_eq!(plugins::load_cards(&installed)[0].value, "Ready");
        let mut newer = entry.clone();
        newer["version"] = json!("2.0.0");
        let market = write_index(json!([newer])).unwrap();
        assert!(market.install("example", &installed).is_err());
        fs::write(&package, b"corrupt").unwrap();
        assert!(market.install("example", &installed).is_err());
        assert_eq!(fs::read(installed.join("example.json")).unwrap(), document);
        assert_eq!(fs::read_dir(&installed).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn official_market_installs_from_the_release_that_was_browsed() {
        let release = |tag: &str, draft, prerelease| {
            Source::release_index(
                &serde_json::to_vec(&json!({
                    "tag_name":tag, "draft":draft, "prerelease":prerelease
                }))
                .unwrap(),
            )
        };
        let source = release("v2.0.1", false, false).unwrap();
        let Source::Https(ref index) = source else {
            panic!()
        };
        assert_eq!(
            index.to_string(),
            "https://raw.githubusercontent.com/Ariember/DeskIsle-PluginMarket/v2.0.1/index.json"
        );
        let Source::Https(package) = source.package("card-one").unwrap() else {
            panic!()
        };
        assert_eq!(
            package.to_string(),
            "https://raw.githubusercontent.com/Ariember/DeskIsle-PluginMarket/v2.0.1/packages/card-one.json"
        );
        for tag in ["main", "2.0.1", "v2.0.1/../main", "v2.0.1-beta.1"] {
            assert!(release(tag, false, false).is_err());
        }
        assert!(release("v2.0.1", true, false).is_err());
        assert!(release("v2.0.1", false, true).is_err());
    }

    #[test]
    fn resolves_remote_package_against_index_directory() {
        let source =
            Source::configured("https://example.org/v1/index.json?ref=test".into()).unwrap();
        let Source::Https(uri) = source.package("card-one").unwrap() else {
            panic!()
        };
        assert_eq!(
            uri.to_string(),
            "https://example.org/v1/packages/card-one.json"
        );
        for url in [
            "http://example.org/index.json",
            "https://a@b/index.json",
            "https://example.org/",
        ] {
            assert!(Source::configured(url.into()).is_err());
        }
    }
}
