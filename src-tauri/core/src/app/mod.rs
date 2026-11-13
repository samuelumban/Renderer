//! Application state and orchestration. UI-agnostic: the desktop shell only
//! forwards commands here and relays `Event`s.

mod environment;
mod preview;
mod projects;
mod render;
mod storage;

pub use environment::PrepareOptions;

use crate::logs::Emit;
use crate::model::{Event, Project, RenderJob, Settings};
use crate::paths::Paths;
use crate::process::CancelToken;
use crate::runtime::NodeRuntime;
use crate::{fsutil, Diagnostic, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Default)]
struct Db {
    settings: Settings,
    projects: Vec<Project>,
    jobs: Vec<RenderJob>,
}

pub struct Core {
    pub paths: Paths,
    emit: Emit,
    db: Mutex<Db>,
    /// Running environment preparations, by project id.
    ops: Mutex<HashMap<String, CancelToken>>,
    job_tokens: Mutex<HashMap<String, CancelToken>>,
    previews: Mutex<HashMap<String, preview::Entry>>,
    import_cancel: Mutex<CancelToken>,
    /// Installs and renders in flight; cache cleanup is refused while > 0.
    active: AtomicUsize,
}

pub(crate) struct ActiveGuard<'a>(&'a AtomicUsize);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Core {
    pub fn new(paths: Paths, emit: Emit) -> Result<Arc<Core>> {
        paths.ensure().map_err(|e| Diagnostic::io("Cannot create the app data folder", e))?;
        let dir = paths.settings_dir();
        let db = Db {
            settings: fsutil::read_json(&dir.join("settings.json")).unwrap_or_default(),
            projects: fsutil::read_json(&dir.join("projects.json")).unwrap_or_default(),
            jobs: fsutil::read_json(&dir.join("jobs.json")).unwrap_or_default(),
        };
        let core = Arc::new(Core {
            paths,
            emit,
            db: Mutex::new(db),
            ops: Mutex::default(),
            job_tokens: Mutex::default(),
            previews: Mutex::default(),
            import_cancel: Mutex::default(),
            active: AtomicUsize::new(0),
        });
        core.startup_recovery();
        Ok(core)
    }

    pub(crate) fn emit(&self, e: Event) {
        (self.emit)(e)
    }
    pub(crate) fn emitter(&self) -> Emit {
        self.emit.clone()
    }

    pub(crate) fn begin_active(&self) -> ActiveGuard<'_> {
        self.active.fetch_add(1, Ordering::SeqCst);
        ActiveGuard(&self.active)
    }
    pub(crate) fn is_busy(&self) -> bool {
        self.active.load(Ordering::SeqCst) > 0
    }

    fn save_projects(&self, db: &Db) {
        let _ = fsutil::write_json_atomic(&self.paths.settings_dir().join("projects.json"), &db.projects);
    }
    fn save_jobs(&self, db: &Db) {
        let _ = fsutil::write_json_atomic(&self.paths.settings_dir().join("jobs.json"), &db.jobs);
    }

    // --- settings ----------------------------------------------------------

    pub fn settings(&self) -> Settings {
        lock(&self.db).settings.clone()
    }

    pub fn update_settings(&self, mut s: Settings) -> Result<Settings> {
        if s.output_dir.trim().is_empty() || !Path::new(&s.output_dir).is_absolute() {
            return Err(Diagnostic::new("The default output folder must be an absolute path."));
        }
        if !s.node_mirror.starts_with("https://") {
            return Err(Diagnostic::new("The Node.js mirror must be an https:// URL."));
        }
        s.cache_limit_bytes = s.cache_limit_bytes.max(100 * 1024 * 1024);
        s.max_concurrent_renders = s.max_concurrent_renders.clamp(1, 4);
        s.install_timeout_minutes = s.install_timeout_minutes.clamp(1, 240);
        s.default_node_major = s.default_node_major.clamp(14, 99);
        s.browser_executable = s.browser_executable.filter(|b| !b.trim().is_empty());
        let mut db = lock(&self.db);
        db.settings = s.clone();
        fsutil::write_json_atomic(&self.paths.settings_dir().join("settings.json"), &db.settings)?;
        Ok(s)
    }

    // --- project records ---------------------------------------------------

    pub fn list_projects(&self) -> Vec<Project> {
        lock(&self.db).projects.clone()
    }

    pub fn project(&self, id: &str) -> Result<Project> {
        lock(&self.db)
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| Diagnostic::new("Project not found."))
    }

    pub(crate) fn update_project(&self, id: &str, f: impl FnOnce(&mut Project)) -> Result<Project> {
        let updated = {
            let mut db = lock(&self.db);
            let p = db.projects.iter_mut().find(|p| p.id == id).ok_or_else(|| Diagnostic::new("Project not found."))?;
            f(p);
            let p = p.clone();
            self.save_projects(&db);
            p
        };
        self.emit(Event::Projects);
        Ok(updated)
    }

    // --- jobs --------------------------------------------------------------

    pub fn list_jobs(&self) -> Vec<RenderJob> {
        let mut jobs = lock(&self.db).jobs.clone();
        jobs.reverse();
        jobs
    }

    pub(crate) fn update_job(&self, id: &str, f: impl FnOnce(&mut RenderJob)) -> Option<RenderJob> {
        let updated = {
            let mut db = lock(&self.db);
            let j = db.jobs.iter_mut().find(|j| j.id == id)?;
            f(j);
            let j = j.clone();
            self.save_jobs(&db);
            j
        };
        self.emit(Event::Jobs);
        Some(updated)
    }

    /// Environment for project child processes: managed Node first on PATH,
    /// temp files redirected into the workspace, app-owned caches.
    pub(crate) fn child_env(&self, node: &NodeRuntime, temp: &Path) -> Vec<(String, OsString)> {
        let mut path = OsString::from(node.bin_dir());
        if let Some(existing) = std::env::var_os("PATH") {
            path.push(if cfg!(windows) { ";" } else { ":" });
            path.push(existing);
        }
        let mut env = crate::package_manager::cache_env(&self.paths);
        env.extend([
            ("PATH".to_string(), path),
            ("TEMP".to_string(), temp.into()),
            ("TMP".to_string(), temp.into()),
            ("TMPDIR".to_string(), temp.into()),
            ("NO_COLOR".to_string(), "1".into()),
            ("FORCE_COLOR".to_string(), "0".into()),
        ]);
        env
    }

    /// The managed runtime recorded for a prepared project.
    pub(crate) fn node_for(&self, project: &Project) -> Result<NodeRuntime> {
        let rt = project.node_version.as_ref().map(|v| NodeRuntime {
            version: v.clone(),
            dir: self.paths.runtimes().join(format!("node-v{v}")),
        });
        rt.filter(NodeRuntime::is_complete).ok_or_else(|| {
            Diagnostic::new("The Node.js runtime for this project is not installed.").suggest("Use Rebuild environment.")
        })
    }

    /// Stop every child process (app exit).
    pub fn shutdown(&self) {
        lock(&self.import_cancel).cancel();
        for t in lock(&self.ops).values() {
            t.cancel();
        }
        for t in lock(&self.job_tokens).values() {
            t.cancel();
        }
        for e in lock(&self.previews).values() {
            e.token.cancel();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let idle = lock(&self.ops).is_empty()
                && lock(&self.job_tokens).is_empty()
                && lock(&self.previews).values().all(|e| e.info.status == "failed");
            if idle {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
