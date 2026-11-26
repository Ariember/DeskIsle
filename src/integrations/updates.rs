use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use ureq::{Agent, Body, http::Response};

const RELEASES_API: &str = "https://api.github.com/repos/Ariember/DeskIsle/releases";
const MAX_ASSET_SIZE: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Release {
    pub version: String,
    asset_id: u64,
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    id: u64,
    name: String,
    state: String,
    size: u64,
    digest: Option<String>,
}

pub(crate) fn check() -> Result<Option<Release>, String> {
    check_version(&Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo package version"))
}

fn check_version(current: &Version) -> Result<Option<Release>, String> {
    let mut response = get(
        &format!("{RELEASES_API}/latest"),
        "application/vnd.github+json",
        20,
    )?;
    require_ok(&response)?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_vec()
        .map_err(|_| "无法读取 GitHub 发布信息。")?;
    let release: GithubRelease =
        serde_json::from_slice(&bytes).map_err(|_| "GitHub 发布信息格式无效。")?;
    release.update_for(current)
}

fn get(url: &str, accept: &str, seconds: u64) -> Result<Response<Body>, String> {
    let timeout = Some(Duration::from_secs(10));
    let agent: Agent = Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_resolve(timeout)
        .timeout_connect(timeout)
        .timeout_send_request(timeout)
        .timeout_recv_response(timeout)
        .timeout_per_read(timeout)
        .timeout_global(Some(Duration::from_secs(seconds)))
        .user_agent(concat!("DeskIsle/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    agent
        .get(url)
        .header("Accept", accept)
        .header("Cache-Control", "no-cache")
        .call()
        .map_err(|error| match error {
            ureq::Error::StatusCode(status) => format!("GitHub 返回 HTTP {status}，请稍后再试。"),
            ureq::Error::Timeout(_) => "连接 GitHub 超时，请稍后再试。".into(),
            _ => "无法连接 GitHub，请检查网络连接后重试。".into(),
        })
}

fn require_ok(response: &Response<Body>) -> Result<(), String> {
    if response.status() != 200 {
        return Err(format!(
            "GitHub 返回了意外的 HTTP 状态 {}。",
            response.status()
        ));
    }
    Ok(())
}

impl GithubRelease {
    fn update_for(self, current: &Version) -> Result<Option<Release>, String> {
        if self.draft || self.prerelease {
            return Ok(None);
        }
        let version = self
            .tag_name
            .strip_prefix('v')
            .and_then(|tag| Version::parse(tag).ok())
            .ok_or("发布标签缺少有效的版本号。")?;
        if !version.pre.is_empty() || !version.cmp_precedence(current).is_gt() {
            return Ok(None);
        }
        let asset = self
            .assets
            .into_iter()
            .find(|asset| asset.name == "DeskIsle-Setup-x64.exe" && asset.state == "uploaded")
            .ok_or("此版本缺少 Windows x64 安装包。")?;
        if !(1..=MAX_ASSET_SIZE).contains(&asset.size) {
            return Err("更新文件大小无效。".into());
        }
        let sha256 = asset
            .digest
            .as_deref()
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("此版本缺少有效的 SHA-256 校验值。")?
            .to_ascii_lowercase();
        Ok(Some(Release {
            version: version.to_string(),
            asset_id: asset.id,
            size: asset.size,
            sha256,
        }))
    }
}

impl Release {
    pub(crate) fn download(
        &self,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        check_cancelled(cancelled)?;
        let mut response = get(
            &format!("{RELEASES_API}/assets/{}", self.asset_id),
            "application/octet-stream",
            30,
        )?;
        if response.status() == 302 {
            let location = response
                .headers()
                .get("Location")
                .and_then(|v| v.to_str().ok())
                .filter(|url| {
                    url.starts_with("https://release-assets.githubusercontent.com/")
                        || url.starts_with("https://objects.githubusercontent.com/")
                })
                .ok_or("下载地址不是 GitHub 的发布文件服务器。")?;
            check_cancelled(cancelled)?;
            response = get(location, "application/octet-stream", 180)?;
        }
        require_ok(&response)?;
        self.save(response.into_body().into_reader(), destination, cancelled)
    }

    fn save(
        &self,
        mut source: impl Read,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        let mut file =
            File::create_new(destination).map_err(|e| format!("无法创建更新文件：{e}"))?;
        let result = self.write_verified(&mut source, &mut file, cancelled);
        drop(file);
        if result.is_err() {
            let _ = fs::remove_file(destination);
        }
        result
    }

    fn write_verified(
        &self,
        source: &mut impl Read,
        file: &mut File,
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        let mut hash = Sha256::new();
        let mut total = 0;
        let mut buffer = [0; 32 * 1024];
        loop {
            check_cancelled(cancelled)?;
            let count = source
                .read(&mut buffer)
                .map_err(|_| "读取更新文件失败，请重试。")?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > self.size {
                return Err("下载的更新文件超出发布大小。".into());
            }
            file.write_all(&buffer[..count])
                .map_err(|e| format!("无法写入更新文件：{e}"))?;
            hash.update(&buffer[..count]);
        }
        if total != self.size || format!("{:x}", hash.finalize()) != self.sha256 {
            return Err("更新文件未通过完整性校验，请重新下载。".into());
        }
        file.sync_all()
            .map_err(|e| format!("无法保存更新文件：{e}"))
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("更新下载已取消。".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "downloads the current public release from GitHub"]
    fn download_published_asset() {
        let release = check_version(&Version::new(0, 0, 0)).unwrap().unwrap();
        let path = std::env::temp_dir().join(format!("deskisle-live-test-{}", std::process::id()));
        release.download(&path, &AtomicBool::new(false)).unwrap();
        fs::remove_file(path).unwrap();
    }

    fn fixture(tag: &str) -> GithubRelease {
        GithubRelease {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: vec![Asset {
                id: 23,
                name: "DeskIsle-Setup-x64.exe".into(),
                state: "uploaded".into(),
                size: 3,
                digest: Some(format!("sha256:{:x}", Sha256::digest(b"abc"))),
            }],
        }
    }

    #[test]
    fn selects_only_newer_stable_versions() {
        let current = Version::parse("0.9.0+nightly").unwrap();
        for tag in ["v0.8.1", "v0.9.0+other", "v0.10.0-beta.1"] {
            assert!(fixture(tag).update_for(&current).unwrap().is_none());
        }
        let mut draft = fixture("v0.10.0");
        draft.draft = true;
        assert!(draft.update_for(&current).unwrap().is_none());
        assert_eq!(
            fixture("v0.10.0")
                .update_for(&current)
                .unwrap()
                .unwrap()
                .version,
            "0.10.0"
        );
        let candidate = Version::parse("0.9.0-rc.1").unwrap();
        assert!(fixture("v0.9.0").update_for(&candidate).unwrap().is_some());
    }

    #[test]
    fn rejects_unusable_release_assets() {
        let current = Version::new(0, 1, 0);
        let mut release = fixture("v0.10.0");
        release.assets[0].digest = None;
        assert!(release.update_for(&current).is_err());
        let mut release = fixture("v0.10.0");
        release.assets[0].name = "deskisle.exe".into();
        assert!(release.update_for(&current).is_err());
    }

    #[test]
    fn saves_only_verified_payload_and_preserves_existing_file() {
        let path =
            std::env::temp_dir().join(format!("deskisle-download-test-{}", std::process::id()));
        let release = fixture("v1.0.0")
            .update_for(&Version::new(0, 9, 0))
            .unwrap()
            .unwrap();
        let cancelled = AtomicBool::new(true);
        assert!(release.save(&b"abc"[..], &path, &cancelled).is_err());
        assert!(!path.exists());
        cancelled.store(false, Ordering::Relaxed);
        for invalid in [b"".as_slice(), b"xyz", b"abcd"] {
            assert!(release.save(invalid, &path, &cancelled).is_err());
            assert!(!path.exists());
        }
        release.save(&b"abc"[..], &path, &cancelled).unwrap();
        assert!(release.save(&b"xyz"[..], &path, &cancelled).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        fs::remove_file(path).unwrap();
    }
}
