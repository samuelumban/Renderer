//! Per-project log files: streamed to disk, redacted, size-rotated, pruned.

use crate::model::{Event, LogFile};
use regex::Regex;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
pub const KEEP_JOB_LOGS: usize = 10;
pub const MAX_LOG_AGE: Duration = Duration::from_secs(14 * 24 * 3600);

fn ansi() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07").unwrap())
}

fn secrets() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r#"(?i)((?:_authToken|_auth|_password|token|secret|password|api[_-]?key|authorization)["']?\s*[=:]\s*(?:bearer\s+)?)["']?[^\s"']{6,}|\b(npm_[A-Za-z0-9]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9_-]{20,})|(://[^/\s:@]+:)[^@\s/]+@"#,
        )
        .unwrap()
    })
}

/// Remove ANSI escapes and mask anything that looks like a credential.
pub fn clean_line(line: &str) -> String {
    let line = ansi().replace_all(line, "");
    secrets()
        .replace_all(&line, |c: &regex::Captures| {
            if let Some(prefix) = c.get(1) {
                format!("{}[REDACTED]", prefix.as_str())
            } else if let Some(prefix) = c.get(3) {
                format!("{}[REDACTED]@", prefix.as_str())
            } else {
                "[REDACTED]".to_string()
            }
        })
        .into_owned()
}

pub struct LogSink {
    path: PathBuf,
    file: Option<File>,
    written: u64,
    key: String,
    emit: Emit,
}

impl LogSink {
    pub fn create(path: PathBuf, key: impl Into<String>, emit: Emit) -> LogSink {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if path.exists() {
            let _ = fs::rename(&path, rotated(&path));
        }
        let file = OpenOptions::new().create(true).append(true).open(&path).ok();
        LogSink { path, file, written: 0, key: key.into(), emit }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn line(&mut self, raw: &str) {
        let line = clean_line(raw);
        if line.trim().is_empty() {
            return;
        }
        if self.written > MAX_LOG_BYTES {
            let _ = fs::rename(&self.path, rotated(&self.path));
            self.file = OpenOptions::new().create(true).append(true).open(&self.path).ok();
            self.written = 0;
        }
        if let Some(f) = self.file.as_mut() {
            if writeln!(f, "{line}").is_ok() {
                self.written += line.len() as u64 + 1;
            }
        }
        (self.emit)(Event::Log { key: self.key.clone(), line });
    }
}

fn rotated(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".1");
    PathBuf::from(s)
}

/// Read at most `max_bytes` from the end of a log.
pub fn read_tail(path: &Path, max_bytes: u64) -> String {
    let Ok(mut f) = File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.take(max_bytes).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        text.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or(text)
    } else {
        text
    }
}

pub fn list(dir: &Path) -> Vec<LogFile> {
    let mut out: Vec<(SystemTime, LogFile)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            if !m.is_file() {
                return None;
            }
            let t = m.modified().ok()?;
            let modified: chrono::DateTime<chrono::Utc> = t.into();
            Some((
                t,
                LogFile {
                    name: e.file_name().to_string_lossy().into_owned(),
                    bytes: m.len(),
                    modified: modified.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                },
            ))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, l)| l).collect()
}

/// Retention: keep the newest `KEEP_JOB_LOGS` job logs and nothing older than
/// 14 days (whichever keeps less).
pub fn prune(dir: &Path, now: SystemTime) {
    let mut jobs: Vec<(SystemTime, PathBuf)> = Vec::new();
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(m) = e.metadata() else { continue };
        let Ok(t) = m.modified() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        let old = now.duration_since(t).map(|d| d > MAX_LOG_AGE).unwrap_or(false);
        if old {
            let _ = fs::remove_file(e.path());
        } else if name.starts_with("job-") {
            jobs.push((t, e.path()));
        }
    }
    jobs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in jobs.into_iter().skip(KEEP_JOB_LOGS) {
        let _ = fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secrets_and_ansi() {
        assert_eq!(clean_line("\x1b[32mok\x1b[0m"), "ok");
        let l = clean_line("//registry.npmjs.org/:_authToken=npm_abcdefghijklmnopqrstuvwxyz123456");
        assert!(!l.contains("abcdefghij"), "{l}");
        let l = clean_line("fetch https://user:hunter2secret@example.com/x");
        assert!(!l.contains("hunter2secret"), "{l}");
        assert_eq!(clean_line("Rendered 10/150"), "Rendered 10/150");
    }

    #[test]
    fn prune_keeps_newest_ten() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..14 {
            let p = tmp.path().join(format!("job-{i:02}.log"));
            fs::write(&p, "x").unwrap();
            let t = SystemTime::now() - Duration::from_secs(1000 - i * 10);
            File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        }
        fs::write(tmp.path().join("env.log"), "x").unwrap();
        prune(tmp.path(), SystemTime::now());
        let names: Vec<String> = list(tmp.path()).into_iter().map(|l| l.name).collect();
        assert_eq!(names.iter().filter(|n| n.starts_with("job-")).count(), 10);
        assert!(names.contains(&"job-13.log".to_string()));
        assert!(!names.contains(&"job-00.log".to_string()));
        assert!(names.contains(&"env.log".to_string()));
        // Everything is older than 14 days from this point of view.
        prune(tmp.path(), SystemTime::now() + Duration::from_secs(15 * 24 * 3600));
        assert!(list(tmp.path()).is_empty());
    }
}
