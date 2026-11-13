//! Import, removal and per-project maintenance.

use super::{lock, Core};
use crate::model::{EnvStatus, Event, JobStatus, LogFile, Project, StorageMode};
use crate::process::CancelToken;
use crate::project::archive::{self, ExtractLimits};
use crate::project::inspect;
use crate::{fsutil, logs, Diagnostic, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

impl Core {
    pub(crate) fn limits(&self) -> ExtractLimits {
        let s = self.settings();
        ExtractLimits { max_zip_bytes: s.max_zip_bytes, max_extracted_bytes: s.max_extracted_bytes, max_entries: s.max_zip_entries }
    }

    pub fn cancel_import(&self) {
        lock(&self.import_cancel).cancel();
    }

    /// Validate, extract and inspect a ZIP. Nothing from the archive is run.
    pub fn import_zip(&self, zip_path: &Path, name: Option<String>) -> Result<Project> {
        let cancel = CancelToken::new();
        *lock(&self.import_cancel) = cancel.clone();
        if !zip_path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
            return Err(Diagnostic::new("Only .zip files can be imported."));
        }
        let limits = self.limits();
        let need = archive::declared_size(zip_path, &limits)?.saturating_add(100 * 1024 * 1024);
        if let Some(free) = fsutil::available_space(&self.paths.workspaces()) {
            if free < need {
                return Err(Diagnostic::new(format!(
                    "Not enough disk space to extract this project: about {} MB needed, {} MB free.",
                    need / 1_048_576,
                    free / 1_048_576
                ))
                .suggest("Free space in Storage, then import again."));
            }
        }

        // Short ids keep workspace paths well under Windows path limits.
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let ws = self.paths.workspace(&id);
        let source = self.paths.source(&id);
        let fail = |e: Diagnostic| {
            let _ = fsutil::safe_remove(&self.paths.workspaces(), &ws);
            e
        };
        let summary = archive::extract_zip(zip_path, &source, &limits, true, &cancel).map_err(fail)?;
        let root = inspect::find_project_root(&source).ok_or_else(|| {
            fail(Diagnostic::new("No package.json was found in the ZIP.")
                .cause("The archive is not a Node.js/Remotion project, or the project is nested too deeply.", true)
                .suggest("Zip the folder that contains package.json and import that."))
        })?;
        let mut report = inspect::inspect(&root, &source);
        for w in summary.warnings {
            report.issues.push(crate::model::Issue { severity: "info".into(), code: "zip-skipped".into(), message: w, confirmed: true });
        }

        let settings = self.settings();
        let zip_copy = if settings.keep_zip_copy {
            let dest = self.paths.zip_copy(&id);
            fs::copy(zip_path, &dest).map_err(|e| fail(Diagnostic::io("Could not keep a copy of the ZIP", e)))?;
            Some(dest.to_string_lossy().into_owned())
        } else {
            None
        };
        let name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .or_else(|| report.package_name.clone())
            .or_else(|| zip_path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Untitled project".into());
        let project = Project {
            id: id.clone(),
            name,
            original_zip_path: zip_path.to_string_lossy().into_owned(),
            zip_copy_path: zip_copy,
            workspace_path: ws.to_string_lossy().into_owned(),
            project_root: root.to_string_lossy().into_owned(),
            package_manager: report.package_manager.clone(),
            lockfile_path: report.lockfile.as_ref().map(|l| root.join(l).to_string_lossy().into_owned()),
            remotion_version: inspect::remotion_version(&report),
            node_requirement: report.node_requirement.clone(),
            storage_mode: settings.default_storage_mode,
            env_status: EnvStatus::NotPrepared,
            report,
            created_at: crate::now(),
            last_opened_at: Some(crate::now()),
            ..Default::default()
        };
        {
            let mut db = lock(&self.db);
            db.projects.push(project.clone());
            self.save_projects(&db);
        }
        self.emit(Event::Projects);
        Ok(project)
    }

    /// The ZIP to restore a Save Space project from, if one still exists.
    pub(crate) fn restorable_zip(&self, p: &Project) -> Option<PathBuf> {
        p.zip_copy_path
            .iter()
            .chain(std::iter::once(&p.original_zip_path))
            .map(PathBuf::from)
            .find(|z| z.is_file())
    }

    /// Re-run static inspection (after install, font changes, or restore).
    pub fn reinspect(&self, id: &str) -> Result<Project> {
        let p = self.project(id)?;
        let root = PathBuf::from(&p.project_root);
        if !root.is_dir() {
            return Ok(p);
        }
        let mut report = inspect::inspect(&root, &self.paths.source(id));
        // Keep the authoritative composition list from the CLI if we had one.
        if p.report.compositions.iter().any(|c| c.source == "remotion-cli") {
            report.compositions = p.report.compositions.clone();
        }
        self.update_project(id, |p| {
            p.remotion_version = inspect::remotion_version(&report);
            p.node_requirement = report.node_requirement.clone();
            p.package_manager = report.package_manager.clone();
            p.lockfile_path = report.lockfile.as_ref().map(|l| root.join(l).to_string_lossy().into_owned());
            p.report = report;
        })
    }

    pub fn open_project(&self, id: &str) -> Result<Project> {
        self.update_project(id, |p| p.last_opened_at = Some(crate::now()))
    }

    pub fn rename_project(&self, id: &str, name: &str) -> Result<Project> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(Diagnostic::new("The project name cannot be empty."));
        }
        self.update_project(id, |p| p.name = name)
    }

    pub fn set_storage_mode(&self, id: &str, mode: StorageMode) -> Result<Project> {
        self.update_project(id, |p| p.storage_mode = mode)
    }

    pub(crate) fn project_has_active_work(&self, id: &str) -> bool {
        lock(&self.ops).contains_key(id)
            || lock(&self.db).jobs.iter().any(|j| j.project_id == id && j.status.is_active())
    }

    /// Remove a project: workspace, app-owned ZIP copy and logs. The user's
    /// original ZIP and rendered outputs are never touched.
    pub fn remove_project(&self, id: &str) -> Result<()> {
        self.project(id)?;
        if self.project_has_active_work(id) {
            return Err(Diagnostic::new("This project is installing or rendering.").suggest("Cancel the running job first."));
        }
        self.stop_preview_and_wait(id);
        {
            let mut db = lock(&self.db);
            for j in db.jobs.iter_mut().filter(|j| j.project_id == id && j.status == JobStatus::Waiting) {
                j.status = JobStatus::Cancelled;
                j.finished_at = Some(crate::now());
            }
            self.save_jobs(&db);
        }
        fsutil::safe_remove(&self.paths.workspaces(), &self.paths.workspace(id))?;
        let _ = fsutil::safe_remove(&self.paths.logs(), &self.paths.project_logs(id));
        {
            let mut db = lock(&self.db);
            db.projects.retain(|p| p.id != id);
            self.save_projects(&db);
        }
        self.emit(Event::Projects);
        self.emit(Event::Jobs);
        Ok(())
    }

    /// Copy user-chosen font files into the project's `public/fonts` folder.
    /// Existing files are never overwritten.
    pub fn add_fonts(&self, id: &str, files: &[PathBuf]) -> Result<Project> {
        let p = self.project(id)?;
        let root = PathBuf::from(&p.project_root);
        if !root.is_dir() {
            return Err(Diagnostic::new("The project workspace is not extracted.").suggest("Prepare the environment first."));
        }
        let dir = root.join("public").join("fonts");
        fs::create_dir_all(&dir)?;
        for f in files {
            let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !["ttf", "otf", "woff", "woff2"].contains(&ext.as_str()) {
                return Err(Diagnostic::new(format!("{} is not a font file (.ttf, .otf, .woff, .woff2).", f.display())));
            }
            let name = fsutil::sanitize_file_name(&f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            let dest = dir.join(name);
            if !dest.exists() {
                fs::copy(f, &dest).map_err(|e| Diagnostic::io("Could not copy the font", e))?;
            }
        }
        self.reinspect(id)
    }

    /// Approximate disk use per project workspace.
    pub fn project_sizes(&self) -> HashMap<String, u64> {
        self.list_projects()
            .into_iter()
            .map(|p| {
                let size = fsutil::dir_size(&self.paths.workspace(&p.id));
                (p.id, size)
            })
            .collect()
    }

    // --- logs --------------------------------------------------------------

    pub fn list_logs(&self, id: &str) -> Vec<LogFile> {
        logs::list(&self.paths.project_logs(id))
    }

    pub fn read_log(&self, id: &str, name: &str) -> Result<String> {
        if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") || id.contains(['/', '\\', '.']) {
            return Err(Diagnostic::new("Invalid log name."));
        }
        Ok(logs::read_tail(&self.paths.project_logs(id).join(name), 256 * 1024))
    }

    pub fn delete_logs(&self, id: Option<&str>) -> Result<u64> {
        let logs_root = self.paths.logs();
        match id {
            Some(id) if !id.contains(['/', '\\', '.']) => fsutil::clear_dir(&logs_root, &self.paths.project_logs(id)),
            Some(_) => Err(Diagnostic::new("Invalid project id.")),
            None => fsutil::clear_dir(&logs_root, &logs_root),
        }
    }
}
