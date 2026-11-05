//! Hanji 자동 업데이트.
//!
//! 1. [`check`]: GitHub Releases의 최신 릴리스(초안·사전 릴리스 제외)를 읽어 지금 버전보다 새로우면
//!    이 플랫폼의 설치 파일을 돌려준다. 파일 이름 규칙은 [`asset_name`]
//!    (`Hanji-<버전>-arm64.dmg`, `Hanji-<버전>-x64.msi`).
//! 2. [`download`]: 설치 파일을 임시 폴더에 내려받는다.
//! 3. [`install`]: macOS는 dmg 안의 앱이 지금 앱과 같은 팀 ID·번들 ID로 서명·공증됐는지 확인한 뒤
//!    앱 번들을 통째로 바꾼다(실행 중인 앱은 그대로 돌고, 다시 시작하면 새 버전이 뜬다).
//!    Windows는 앱이 끝나기를 기다렸다가 MSI를 설치하고 다시 띄우는 보조 프로세스를 띄운다.
//! 4. [`relaunch`]: 앱이 끝나면 새 버전을 띄운다(macOS). 부른 뒤 앱을 종료한다.
//!
//! 모든 함수는 막는(blocking) 함수다. UI 스레드가 아니라 백그라운드에서 부른다.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

mod platform;

pub use platform::{Installation, installation};

/// 릴리스를 올리는 GitHub 저장소.
pub const REPOSITORY: &str = "drskur-com/writer";

const USER_AGENT: &str = concat!(
    "Hanji-Updater/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/drskur-com/writer)"
);

/// 업데이트 실패.
#[derive(Debug)]
pub enum Error {
    /// 네트워크·HTTP 오류.
    Network(String),
    /// 릴리스 정보를 읽지 못함.
    InvalidRelease(String),
    /// 파일·프로세스 오류.
    Io(String),
    /// 내려받은 앱의 서명이 지금 앱과 맞지 않거나 공증되지 않음.
    Verification(String),
    /// 이 설치는 스스로 바꿀 수 없음(번들이 아님, 서명 없음, 쓰기 권한 없음 등).
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Network(message)
            | Error::InvalidRelease(message)
            | Error::Io(message)
            | Error::Verification(message)
            | Error::Unsupported(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err.to_string())
    }
}

/// `major.minor.patch` 버전. 앞의 `v`와 뒤의 `-pre`·`+build`는 무시한다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches(['v', 'V']).trim_start_matches('.');
        let core = text.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().unwrap_or("0").parse().ok()?;
        let patch = parts.next().unwrap_or("0").parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self(major, minor, patch))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// 새 릴리스.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    /// 릴리스 노트(마크다운).
    pub notes: String,
    /// 릴리스 웹 페이지.
    pub page_url: String,
    pub asset: Asset,
}

/// 내려받을 설치 파일.
#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// 이 플랫폼의 설치 파일 이름. 지원하지 않는 플랫폼이면 `None`.
pub fn asset_name(version: Version) -> Option<String> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(format!("Hanji-{version}-arm64.dmg"))
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(format!("Hanji-{version}-x64.msi"))
    } else {
        None
    }
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// 최신 릴리스 JSON에서 `current`보다 새로운 이 플랫폼용 릴리스를 고른다.
/// 새 버전이지만 이 플랫폼 파일이 아직 없으면(빌드 중) `None`.
fn pick(json: &str, current: Version) -> Result<Option<Release>, Error> {
    let release: GithubRelease =
        serde_json::from_str(json).map_err(|err| Error::InvalidRelease(err.to_string()))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = Version::parse(&release.tag_name).ok_or_else(|| {
        Error::InvalidRelease(format!("Unknown version tag: {}", release.tag_name))
    })?;
    if version <= current {
        return Ok(None);
    }
    let Some(name) = asset_name(version) else {
        return Ok(None);
    };
    let Some(asset) = release.assets.into_iter().find(|asset| asset.name == name) else {
        return Ok(None);
    };
    Ok(Some(Release {
        version,
        notes: release.body.unwrap_or_default(),
        page_url: release.html_url,
        asset: Asset {
            name: asset.name,
            url: asset.browser_download_url,
            size: asset.size,
        },
    }))
}

fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::from)
}

fn client(timeout: Option<Duration>) -> Result<reqwest::Client, Error> {
    let builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15));
    let builder = match timeout {
        Some(timeout) => builder.timeout(timeout),
        None => builder,
    };
    builder.build().map_err(|err| Error::Network(err.to_string()))
}

/// 새 버전이 있으면 그 릴리스를 돌려준다.
pub fn check(current: Version) -> Result<Option<Release>, Error> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    let json = runtime()?.block_on(async {
        let response = client(Some(Duration::from_secs(20)))?
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|err| Error::Network(err.to_string()))?;
        // 릴리스가 하나도 없으면 404다.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Error::Network(format!("GitHub: HTTP {}", response.status())));
        }
        response
            .text()
            .await
            .map(Some)
            .map_err(|err| Error::Network(err.to_string()))
    })?;
    match json {
        Some(json) => pick(&json, current),
        None => Ok(None),
    }
}

/// 내려받기 진행 상황. 다른 스레드에서 읽는다.
#[derive(Clone, Default)]
pub struct Progress {
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

impl Progress {
    /// (받은 바이트, 전체 바이트). 전체를 모르면 0.
    pub fn get(&self) -> (u64, u64) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

/// 설치 파일을 임시 폴더(`<temp>/hanji-update/`)에 내려받고 경로를 돌려준다.
pub fn download(asset: &Asset, progress: &Progress) -> Result<PathBuf, Error> {
    let dir = std::env::temp_dir().join("hanji-update");
    std::fs::create_dir_all(&dir)?;
    // 이름에 경로가 섞여 있으면 임시 폴더 밖으로 나갈 수 있다.
    if asset.name.contains(['/', '\\']) || asset.name.starts_with('.') {
        return Err(Error::InvalidRelease(format!("Bad file name: {}", asset.name)));
    }
    let path = dir.join(&asset.name);
    let partial = dir.join(format!("{}.part", asset.name));
    progress.done.store(0, Ordering::Relaxed);
    progress.total.store(asset.size, Ordering::Relaxed);
    runtime()?.block_on(async {
        let mut response = client(None)?
            .get(&asset.url)
            .send()
            .await
            .map_err(|err| Error::Network(err.to_string()))?;
        if !response.status().is_success() {
            return Err(Error::Network(format!("HTTP {}", response.status())));
        }
        if let Some(length) = response.content_length() {
            progress.total.store(length, Ordering::Relaxed);
        }
        let mut file = std::fs::File::create(&partial)?;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| Error::Network(err.to_string()))?
        {
            std::io::Write::write_all(&mut file, &chunk)?;
            progress.done.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        file.sync_all()?;
        Ok::<(), Error>(())
    })?;
    if asset.size > 0 && std::fs::metadata(&partial)?.len() != asset.size {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::Network("Download was incomplete.".into()));
    }
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

/// 내려받은 설치 파일로 지금 설치를 바꾼다.
///
/// - macOS: 확인을 마치고 앱 번들을 바꾼다. 끝나면 [`relaunch`]로 다시 띄운다.
/// - Windows: 앱이 끝나면 MSI를 설치하고 다시 띄우는 보조 프로세스를 띄운다. 부른 뒤 곧 앱을 끝낸다.
pub fn install(installation: &Installation, downloaded: &Path) -> Result<(), Error> {
    platform::install(installation, downloaded)
}

/// 이 프로세스가 끝나면 앱을 다시 띄운다(macOS). Windows는 [`install`]의 보조 프로세스가 맡으므로
/// 아무것도 하지 않는다. 부른 뒤 앱을 끝낸다.
pub fn relaunch(installation: &Installation) -> Result<(), Error> {
    platform::relaunch(installation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(Version::parse("v0.2.0"), Some(Version(0, 2, 0)));
        assert_eq!(Version::parse("v.0.1.0"), Some(Version(0, 1, 0)));
        assert_eq!(Version::parse("1.10.3-beta.1"), Some(Version(1, 10, 3)));
        assert_eq!(Version::parse("2"), Some(Version(2, 0, 0)));
        assert_eq!(Version::parse("x1.0"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
    }

    fn release_json(tag: &str, names: &[&str]) -> String {
        let assets: Vec<_> = names
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("https://example.com/{name}"),
                    "size": 10
                })
            })
            .collect();
        serde_json::json!({
            "tag_name": tag,
            "body": "notes",
            "html_url": "https://github.com/drskur-com/writer/releases/tag/v0.3.0",
            "draft": false,
            "prerelease": false,
            "assets": assets,
        })
        .to_string()
    }

    #[test]
    fn picks_newer_release_for_this_platform() {
        let Some(name) = asset_name(Version(0, 3, 0)) else {
            return; // 지원하지 않는 플랫폼
        };
        let json = release_json("v0.3.0", &["Hanji-0.3.0-arm64.dmg", "Hanji-0.3.0-x64.msi"]);
        let release = pick(&json, Version(0, 2, 0)).unwrap().unwrap();
        assert_eq!(release.version, Version(0, 3, 0));
        assert_eq!(release.asset.name, name);
        assert_eq!(release.notes, "notes");

        // 같거나 오래된 버전은 고르지 않는다.
        assert!(pick(&json, Version(0, 3, 0)).unwrap().is_none());
        // 이 플랫폼 파일이 아직 없으면 고르지 않는다.
        let json = release_json("v0.3.0", &["Hanji-0.3.0-linux.tar.gz"]);
        assert!(pick(&json, Version(0, 2, 0)).unwrap().is_none());
    }
}
