//! App-managed Node.js runtimes. Downloaded on demand into the app's own
//! `runtimes/` folder; the system Node.js and PATH are never touched.

use crate::model::Settings;
use crate::paths::Paths;
use crate::process::CancelToken;
use crate::{fsutil, Diagnostic, Result};
use node_semver::{Range, Version};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct NodeRuntime {
    pub version: String,
    pub dir: PathBuf,
}

impl NodeRuntime {
    pub fn bin_dir(&self) -> PathBuf {
        if cfg!(windows) { self.dir.clone() } else { self.dir.join("bin") }
    }
    pub fn node_exe(&self) -> PathBuf {
        self.bin_dir().join(if cfg!(windows) { "node.exe" } else { "node" })
    }
    pub fn npm_cli(&self) -> PathBuf {
        let modules = if cfg!(windows) { self.dir.join("node_modules") } else { self.dir.join("lib").join("node_modules") };
        modules.join("npm").join("bin").join("npm-cli.js")
    }
    pub fn is_complete(&self) -> bool {
        self.node_exe().is_file() && self.npm_cli().is_file()
    }
}

/// Parse an npm-style range (`>=18`, `^20.0.0 || ^22`, `18.x`, `20`).
pub fn parse_range(req: &str) -> Option<Range> {
    let req = req.trim().trim_start_matches('v');
    if req.is_empty() {
        return None;
    }
    Range::parse(req).ok()
}

fn platform_file() -> (&'static str, &'static str) {
    // (index.json "files" key, archive suffix)
    if cfg!(windows) {
        if cfg!(target_arch = "aarch64") { ("win-arm64-zip", "win-arm64.zip") } else { ("win-x64-zip", "win-x64.zip") }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") { ("osx-arm64-tar", "darwin-arm64.tar.gz") } else { ("osx-x64-tar", "darwin-x64.tar.gz") }
    } else if cfg!(target_arch = "aarch64") {
        ("linux-arm64", "linux-arm64.tar.gz")
    } else {
        ("linux-x64", "linux-x64.tar.gz")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexEntry {
    pub version: String,
    #[serde(default)]
    pub lts: serde_json::Value,
    #[serde(default)]
    pub files: Vec<String>,
}

impl IndexEntry {
    fn parsed(&self) -> Option<Version> {
        Version::parse(self.version.trim_start_matches('v')).ok()
    }
    fn is_lts(&self) -> bool {
        self.lts.is_string()
    }
}

/// Choose a version from the official index.
/// Policy: with no requirement, the newest release of the default major.
/// With a requirement: the default major if it satisfies it, otherwise the
/// newest satisfying LTS, otherwise the newest satisfying release.
pub fn pick_version(index: &[IndexEntry], range: Option<&Range>, default_major: u32, file_key: &str) -> Option<String> {
    let mut all: Vec<(Version, bool)> = index
        .iter()
        .filter(|e| e.files.is_empty() || e.files.iter().any(|f| f == file_key))
        .filter_map(|e| e.parsed().map(|v| (v, e.is_lts())))
        .filter(|(v, _)| v.pre_release.is_empty())
        .filter(|(v, _)| range.is_none_or(|r| r.satisfies(v)))
        .collect();
    all.sort_by(|a, b| b.0.cmp(&a.0));
    let default = all.iter().find(|(v, _)| v.major == default_major as u64);
    let lts = all.iter().find(|(_, lts)| *lts);
    let chosen = match range {
        None => default,
        Some(_) => default.or(lts).or(all.first()),
    };
    chosen.map(|(v, _)| v.to_string())
}

pub fn installed(paths: &Paths) -> Vec<NodeRuntime> {
    let mut out: Vec<NodeRuntime> = fs::read_dir(paths.runtimes())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let version = name.strip_prefix("node-v")?.to_string();
            Version::parse(&version).ok()?;
            let rt = NodeRuntime { version, dir: e.path() };
            rt.is_complete().then_some(rt)
        })
        .collect();
    out.sort_by(|a, b| Version::parse(&b.version).ok().cmp(&Version::parse(&a.version).ok()));
    out
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .user_agent("universal-remotion-studio")
        .build()
}

fn net_error(what: &str, url: &str, e: impl std::fmt::Display) -> Diagnostic {
    let mut d = Diagnostic::new(format!("{what} failed."))
        .cause("The Node.js download server could not be reached.", true)
        .suggest("Check the internet connection or proxy, or change the Node.js mirror in Settings, then retry.");
    d.excerpt = Some(format!("{url}\n{e}"));
    d
}

fn fetch_text(url: &str) -> Result<String> {
    let resp = agent().get(url).call().map_err(|e| net_error("Downloading Node.js release information", url, e))?;
    let mut text = String::new();
    resp.into_reader()
        .take(64 * 1024 * 1024)
        .read_to_string(&mut text)
        .map_err(|e| net_error("Downloading Node.js release information", url, e))?;
    Ok(text)
}

/// Decide which Node.js version a project should use. Reuses an installed
/// runtime when one satisfies the requirement (works offline).
pub fn resolve(paths: &Paths, settings: &Settings, requirement: Option<&str>, log: &mut dyn FnMut(&str)) -> Result<String> {
    let range = requirement.and_then(parse_range);
    if let (Some(req), None) = (requirement, &range) {
        log(&format!("Node requirement \"{req}\" could not be parsed; using the default Node {}.", settings.default_node_major));
    }
    let have = installed(paths);
    let satisfies = |rt: &&NodeRuntime| {
        let v = Version::parse(&rt.version).ok();
        match (&range, v) {
            (Some(r), Some(v)) => r.satisfies(&v),
            (None, Some(v)) => v.major == settings.default_node_major as u64,
            _ => false,
        }
    };
    let preferred = have
        .iter()
        .filter(satisfies)
        .find(|rt| rt.version.starts_with(&format!("{}.", settings.default_node_major)))
        .or_else(|| have.iter().find(satisfies));
    if let Some(rt) = preferred {
        log(&format!("Reusing installed Node.js v{} for requirement {}.", rt.version, requirement.unwrap_or("(none, default)")));
        return Ok(rt.version.clone());
    }
    let url = format!("{}/index.json", settings.node_mirror.trim_end_matches('/'));
    log(&format!("Fetching Node.js release index: {url}"));
    let index: Vec<IndexEntry> = serde_json::from_str(&fetch_text(&url)?)
        .map_err(|e| net_error("Reading Node.js release information", &url, e))?;
    pick_version(&index, range.as_ref(), settings.default_node_major, platform_file().0).ok_or_else(|| {
        Diagnostic::new(format!("No Node.js release satisfies the project's requirement \"{}\".", requirement.unwrap_or("")))
            .cause("The engines.node range in package.json matches no published Node.js version for this platform.", false)
            .suggest("Correct engines.node in the source project and re-import.")
    })
}

/// Download (if needed), verify against SHASUMS256.txt and unpack a runtime.
pub fn ensure(paths: &Paths, settings: &Settings, version: &str, log: &mut dyn FnMut(&str), cancel: &CancelToken) -> Result<NodeRuntime> {
    let rt = NodeRuntime { version: version.to_string(), dir: paths.runtimes().join(format!("node-v{version}")) };
    if rt.is_complete() {
        return Ok(rt);
    }
    let runtimes = paths.runtimes();
    fs::create_dir_all(&runtimes)?;
    let (_, suffix) = platform_file();
    let file_name = format!("node-v{version}-{suffix}");
    let base = format!("{}/v{version}", settings.node_mirror.trim_end_matches('/'));
    let archive = runtimes.join(format!("{file_name}.partial"));
    let unpack = runtimes.join(format!("node-v{version}.partial"));
    let cleanup = || {
        let _ = fsutil::safe_remove(&runtimes, &archive);
        let _ = fsutil::safe_remove(&runtimes, &unpack);
    };
    cleanup();

    let sums = fetch_text(&format!("{base}/SHASUMS256.txt"))?;
    let expected = sums
        .lines()
        .find_map(|l| {
            let (hash, name) = l.split_once(char::is_whitespace)?;
            (name.trim() == file_name).then(|| hash.trim().to_lowercase())
        })
        .ok_or_else(|| Diagnostic::new(format!("Node.js v{version} has no download for this platform ({file_name}).")))?;

    let url = format!("{base}/{file_name}");
    log(&format!("Downloading {url}"));
    let result = (|| -> Result<()> {
        let resp = agent().get(&url).call().map_err(|e| net_error("Downloading Node.js", &url, e))?;
        let total: u64 = resp.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut reader = resp.into_reader();
        let mut out = File::create(&archive)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        let (mut done, mut last_pct) = (0u64, 0u64);
        loop {
            if cancel.is_cancelled() {
                return Err(Diagnostic::new("Cancelled."));
            }
            let n = reader.read(&mut buf).map_err(|e| net_error("Downloading Node.js", &url, e))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            done += n as u64;
            let pct = if total > 0 { done * 100 / total } else { 0 };
            if pct >= last_pct + 10 {
                last_pct = pct;
                log(&format!("Downloading Node.js v{version}: {pct}%"));
            }
        }
        out.flush()?;
        let actual = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>();
        if actual != expected {
            return Err(Diagnostic::new("The downloaded Node.js archive failed its checksum.")
                .cause("The download was corrupted or tampered with.", true)
                .suggest("Retry. If it keeps failing, check the Node.js mirror setting."));
        }
        log("Checksum verified. Unpacking...");
        unpack_archive(&archive, &unpack, cancel)?;
        // Archives contain a single top-level folder.
        let inner = fs::read_dir(&unpack)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .ok_or_else(|| Diagnostic::new("Unexpected Node.js archive layout."))?;
        fs::rename(&inner, &rt.dir)?;
        Ok(())
    })();
    cleanup();
    result?;
    if !rt.is_complete() {
        let _ = fsutil::safe_remove(&runtimes, &rt.dir);
        return Err(Diagnostic::new("The Node.js runtime was unpacked but is incomplete."));
    }
    log(&format!("Node.js v{version} ready at {}", rt.dir.display()));
    Ok(rt)
}

#[cfg(windows)]
fn unpack_archive(archive: &Path, dest: &Path, cancel: &CancelToken) -> Result<()> {
    use crate::project::archive::{extract_zip, ExtractLimits};
    let limits = ExtractLimits { max_zip_bytes: 1 << 30, max_extracted_bytes: 2 << 30, max_entries: 200_000 };
    extract_zip(archive, dest, &limits, false, cancel).map(|_| ())
}

#[cfg(not(windows))]
fn unpack_archive(archive: &Path, dest: &Path, _cancel: &CancelToken) -> Result<()> {
    fs::create_dir_all(dest)?;
    let gz = flate2::read::GzDecoder::new(File::open(archive)?);
    tar::Archive::new(gz).unpack(dest).map_err(|e| Diagnostic::io("Unpacking Node.js", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(v: &str, lts: bool) -> IndexEntry {
        IndexEntry {
            version: v.into(),
            lts: if lts { serde_json::json!("Name") } else { serde_json::json!(false) },
            files: vec!["win-x64-zip".into()],
        }
    }

    fn index() -> Vec<IndexEntry> {
        vec![entry("v25.1.0", false), entry("v24.3.0", true), entry("v22.11.0", true), entry("v22.9.0", false), entry("v20.18.0", true), entry("v18.20.4", true), entry("v16.20.2", true)]
    }

    fn pick(req: Option<&str>) -> Option<String> {
        let range = req.map(|r| parse_range(r).unwrap());
        pick_version(&index(), range.as_ref(), 22, "win-x64-zip")
    }

    #[test]
    fn version_selection_policy() {
        assert_eq!(pick(None).as_deref(), Some("22.11.0"));
        assert_eq!(pick(Some(">=16")).as_deref(), Some("22.11.0"));
        assert_eq!(pick(Some("^18.0.0 || ^20.0.0")).as_deref(), Some("20.18.0"));
        assert_eq!(pick(Some("18.x")).as_deref(), Some("18.20.4"));
        assert_eq!(pick(Some("16")).as_deref(), Some("16.20.2"));
        assert_eq!(pick(Some(">=25")).as_deref(), Some("25.1.0"));
        assert_eq!(pick(Some(">=99")), None);
        assert_eq!(pick(Some(">=18 <21")).as_deref(), Some("20.18.0"));
    }

    #[test]
    fn range_parsing() {
        assert!(parse_range(">=18.0.0").is_some());
        assert!(parse_range("v20.11.0").is_some());
        assert!(parse_range("lts/*").is_none());
        assert!(parse_range("").is_none());
    }

    #[test]
    fn platform_without_file_is_skipped() {
        let mut idx = index();
        idx[2].files = vec!["linux-x64".into()];
        assert_eq!(pick_version(&idx, None, 22, "win-x64-zip").as_deref(), Some("22.9.0"));
    }
}
