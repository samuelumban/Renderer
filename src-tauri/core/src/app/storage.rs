//! Storage accounting and cleanup. Every deletion goes through
//! `fsutil::safe_remove` against an app-owned root.

use super::{lock, Core};
use crate::model::{EnvStatus, Event, JobStatus, StorageUsage};
use crate::{fsutil, logs, runtime, Diagnostic, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn busy() -> Diagnostic {
    Diagnostic::new("An install or render is running.").suggest("Wait for it to finish, then clean up.")
}

fn children(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect()
}

fn is_partial(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().contains(".partial"))
}

impl Core {
    pub fn storage_usage(&self) -> StorageUsage {
        let mut temp = 0;
        let mut browser = fsutil::dir_size(&self.paths.browser_cache());
        let mut workspaces = fsutil::dir_size(&self.paths.workspaces());
        for ws in children(&self.paths.workspaces()) {
            temp += fsutil::dir_size(&ws.join("temp"));
        }
        // Remotion downloads its rendering browser into node_modules/.remotion.
        for p in self.list_projects() {
            browser += fsutil::dir_size(&Path::new(&p.project_root).join("node_modules").join(".remotion"));
        }
        workspaces = workspaces.saturating_sub(temp).saturating_sub(browser.saturating_sub(fsutil::dir_size(&self.paths.browser_cache())));
        let runtimes_all = fsutil::dir_size(&self.paths.runtimes());
        let partial: u64 = children(&self.paths.runtimes()).iter().filter(|p| is_partial(p)).map(|p| if p.is_dir() { fsutil::dir_size(p) } else { fs::metadata(p).map(|m| m.len()).unwrap_or(0) }).sum();
        StorageUsage {
            workspaces,
            runtimes: runtimes_all.saturating_sub(partial),
            package_cache: fsutil::dir_size(&self.paths.package_cache()),
            browser_cache: browser,
            temp: temp + partial,
            logs: fsutil::dir_size(&self.paths.logs()),
            free_bytes: fsutil::available_space(&self.paths.base),
            cache_limit_bytes: self.settings().cache_limit_bytes,
            base_dir: self.paths.base.to_string_lossy().into_owned(),
        }
    }

    /// Delete temporary folders of every project that has nothing running.
    pub fn clean_temp(&self) -> Result<u64> {
        let ws_root = self.paths.workspaces();
        let previews: Vec<String> = lock(&self.previews).iter().filter(|(_, e)| e.info.status != "failed").map(|(k, _)| k.clone()).collect();
        let mut freed = 0;
        for p in self.list_projects() {
            if self.project_has_active_work(&p.id) || previews.contains(&p.id) {
                continue;
            }
            freed += fsutil::clear_dir(&ws_root, &self.paths.temp(&p.id))?;
        }
        if !self.is_busy() {
            let runtimes = self.paths.runtimes();
            for p in children(&runtimes).into_iter().filter(|p| is_partial(p)) {
                let size = if p.is_dir() { fsutil::dir_size(&p) } else { fs::metadata(&p).map(|m| m.len()).unwrap_or(0) };
                fsutil::safe_remove(&runtimes, &p)?;
                freed += size;
            }
        }
        Ok(freed)
    }

    pub fn clear_package_cache(&self) -> Result<u64> {
        if self.is_busy() {
            return Err(busy());
        }
        let cache = self.paths.package_cache();
        fsutil::clear_dir(&cache, &cache)
    }

    /// Remove Node runtimes no project refers to, and the on-demand
    /// pnpm/Yarn tool installs (they are re-fetched when next needed).
    pub fn remove_unused_runtimes(&self) -> Result<u64> {
        if self.is_busy() {
            return Err(busy());
        }
        let in_use: Vec<String> = self.list_projects().into_iter().filter_map(|p| p.node_version).collect();
        let runtimes = self.paths.runtimes();
        let mut freed = 0;
        for rt in runtime::installed(&self.paths) {
            if !in_use.contains(&rt.version) {
                let size = fsutil::dir_size(&rt.dir);
                fsutil::safe_remove(&runtimes, &rt.dir)?;
                freed += size;
            }
        }
        let tools = self.paths.tools();
        if tools.is_dir() {
            freed += fsutil::clear_dir(&runtimes, &tools)?;
        }
        Ok(freed)
    }

    /// "Clean now": temp files, old logs, and the cache size limit.
    pub fn clean_now(&self) -> Result<u64> {
        let freed = self.clean_temp()?;
        for dir in children(&self.paths.logs()) {
            logs::prune(&dir, SystemTime::now());
        }
        Ok(freed + self.enforce_cache_limit())
    }

    /// Keep the package cache under its limit, least-recently-written first.
    /// npm's content-addressed cache self-heals when entries are missing, and
    /// Yarn cache entries are removed as whole packages; the pnpm store is
    /// only cleared as a whole. Never runs during an install or render.
    pub(crate) fn enforce_cache_limit(&self) -> u64 {
        if self.is_busy() {
            return 0;
        }
        let cache = self.paths.package_cache();
        let limit = self.settings().cache_limit_bytes;
        let mut total = fsutil::dir_size(&cache);
        if total <= limit {
            return 0;
        }
        let before = total;
        let target = limit / 10 * 8;
        let mut units: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        for e in walkdir::WalkDir::new(cache.join("npm").join("_cacache")).follow_links(false).into_iter().flatten() {
            if e.file_type().is_file() {
                if let Ok(m) = e.metadata() {
                    units.push((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), e.path().to_path_buf()));
                }
            }
        }
        for p in children(&cache.join("yarn")) {
            let modified = fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let size = if p.is_dir() { fsutil::dir_size(&p) } else { fs::metadata(&p).map(|m| m.len()).unwrap_or(0) };
            units.push((modified, size, p));
        }
        units.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, size, path) in units {
            if total <= target {
                break;
            }
            if fsutil::safe_remove(&cache, &path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
        if total > limit {
            let store = cache.join("pnpm-store");
            if let Ok(freed) = fsutil::clear_dir(&cache, &store) {
                total = total.saturating_sub(freed);
            }
        }
        before.saturating_sub(total)
    }

    /// Recover from a previous crash: nothing is running at startup, so every
    /// temp folder, orphaned workspace and partial download is stale.
    pub(crate) fn startup_recovery(&self) {
        let ws_root = self.paths.workspaces();
        let known: Vec<String> = self.list_projects().into_iter().map(|p| p.id).collect();
        for ws in children(&ws_root) {
            let name = ws.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if known.contains(&name) {
                let _ = fsutil::safe_remove(&ws_root, &ws.join("temp"));
            } else {
                // Import that never completed.
                let _ = fsutil::safe_remove(&ws_root, &ws);
            }
        }
        let runtimes = self.paths.runtimes();
        for p in children(&runtimes).into_iter().filter(|p| is_partial(p)) {
            let _ = fsutil::safe_remove(&runtimes, &p);
        }

        let stale_temp: Vec<String> = {
            let mut db = lock(&self.db);
            let mut stale = Vec::new();
            for j in db.jobs.iter_mut().filter(|j| !j.status.is_finished()) {
                stale.append(&mut j.temporary_paths);
                j.status = JobStatus::Failed;
                j.phase = "failed".into();
                j.finished_at = Some(crate::now());
                j.error = Some(
                    Diagnostic::new("The app closed while this job was queued or running.")
                        .cause("The app exited or crashed before the render finished.", false)
                        .suggest("Queue the render again."),
                );
            }
            for p in db.projects.iter_mut() {
                if p.env_status == EnvStatus::Preparing {
                    p.env_status = EnvStatus::NotPrepared;
                }
                if p.env_status == EnvStatus::Ready && !Path::new(&p.project_root).join("node_modules").is_dir() {
                    p.env_status = EnvStatus::NotPrepared;
                }
            }
            self.save_jobs(&db);
            self.save_projects(&db);
            stale
        };
        for p in stale_temp {
            self.remove_registered_temp(Path::new(&p));
        }
        for dir in children(&self.paths.logs()) {
            logs::prune(&dir, SystemTime::now());
        }
        self.emit(Event::Projects);
    }
}
