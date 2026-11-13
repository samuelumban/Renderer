//! Render queue. Jobs run through the project's local Remotion CLI, write to
//! a `.partial-` file and are renamed into place only after verification.

use super::{lock, Core};
use crate::diagnostics::{diagnose, Stage};
use crate::logs::{self, LogSink};
use crate::model::{EnvStatus, Event, JobStatus, RenderJob, RenderRequest, StorageMode};
use crate::process::{self, CancelToken, RunSpec};
use crate::{fsutil, remotion, Diagnostic, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_FINISHED_JOBS: usize = 100;
const MIN_FREE_FOR_RENDER: u64 = 200 * 1024 * 1024;

struct Rendered {
    output: PathBuf,
    bytes: u64,
}

impl Core {
    pub fn enqueue_render(self: &Arc<Self>, mut req: RenderRequest) -> Result<RenderJob> {
        remotion::validate_request(&req)?;
        let project = self.project(&req.project_id)?;
        let ext = remotion::extension_for(&req.codec).unwrap_or("mp4");
        let out_dir = PathBuf::from(req.output_dir.trim());
        if !out_dir.is_absolute() {
            return Err(Diagnostic::new("Choose an output folder."));
        }
        let mut name = fsutil::sanitize_file_name(&req.file_name);
        if !name.to_lowercase().ends_with(&format!(".{ext}")) {
            name = format!("{}.{ext}", Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(name.clone()));
        }
        req.file_name = name.clone();
        req.output_dir = out_dir.to_string_lossy().into_owned();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let job = RenderJob {
            id: id.clone(),
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            composition_id: req.composition_id.clone(),
            output_path: out_dir.join(&name).to_string_lossy().into_owned(),
            status: JobStatus::Waiting,
            created_at: crate::now(),
            log_path: self.paths.project_logs(&project.id).join(format!("job-{id}.log")).to_string_lossy().into_owned(),
            request: req,
            phase: "waiting".into(),
            ..Default::default()
        };
        {
            let mut db = lock(&self.db);
            db.jobs.push(job.clone());
            let finished = db.jobs.iter().filter(|j| j.status.is_finished()).count();
            if finished > MAX_FINISHED_JOBS {
                let mut drop_n = finished - MAX_FINISHED_JOBS;
                db.jobs.retain(|j| {
                    if drop_n > 0 && j.status.is_finished() {
                        drop_n -= 1;
                        false
                    } else {
                        true
                    }
                });
            }
            self.save_jobs(&db);
        }
        self.emit(Event::Jobs);
        self.pump_queue();
        Ok(job)
    }

    pub fn cancel_job(self: &Arc<Self>, id: &str) {
        if let Some(t) = lock(&self.job_tokens).get(id) {
            t.cancel();
        }
        self.update_job(id, |j| {
            if j.status == JobStatus::Waiting {
                j.status = JobStatus::Cancelled;
                j.finished_at = Some(crate::now());
                j.phase = "cancelled".into();
            }
        });
    }

    pub fn clear_finished_jobs(&self) {
        {
            let mut db = lock(&self.db);
            db.jobs.retain(|j| !j.status.is_finished());
            self.save_jobs(&db);
        }
        self.emit(Event::Jobs);
    }

    /// Start waiting jobs while capacity allows: at most
    /// `max_concurrent_renders` overall and one per project workspace.
    pub(crate) fn pump_queue(self: &Arc<Self>) {
        loop {
            let next = {
                let preparing: Vec<String> = lock(&self.ops).keys().cloned().collect();
                let mut db = lock(&self.db);
                let max = db.settings.max_concurrent_renders.max(1) as usize;
                let active: Vec<String> = db.jobs.iter().filter(|j| j.status.is_active()).map(|j| j.project_id.clone()).collect();
                if active.len() >= max {
                    None
                } else {
                    let found = db.jobs.iter_mut().find(|j| {
                        j.status == JobStatus::Waiting && !active.contains(&j.project_id) && !preparing.contains(&j.project_id)
                    });
                    let job = found.map(|j| {
                        j.status = JobStatus::Preparing;
                        j.phase = "preparing".into();
                        j.started_at = Some(crate::now());
                        j.clone()
                    });
                    if job.is_some() {
                        self.save_jobs(&db);
                    }
                    job
                }
            };
            let Some(job) = next else { break };
            let token = CancelToken::new();
            lock(&self.job_tokens).insert(job.id.clone(), token.clone());
            let core = self.clone();
            std::thread::spawn(move || core.run_job(job, token));
        }
        self.emit(Event::Jobs);
    }

    fn run_job(self: &Arc<Self>, job: RenderJob, token: CancelToken) {
        let active = self.begin_active();
        let mut log = LogSink::create(PathBuf::from(&job.log_path), format!("job:{}", job.id), self.emitter());
        let mut exit_code = None;
        let result = self.execute_job(&job, &token, &mut log, &mut exit_code);

        // Cleanup always runs: success, failure or cancel. Children are
        // already gone (process::run kills the whole tree before returning).
        let registered = lock(&self.db).jobs.iter().find(|j| j.id == job.id).map(|j| j.temporary_paths.clone()).unwrap_or_default();
        for p in &registered {
            self.remove_registered_temp(Path::new(p));
        }
        let cancelled = token.is_cancelled();
        let completed = result.is_ok();
        match &result {
            Ok(r) => log.line(&format!("Completed: {} ({} bytes). Temporary files removed.", r.output.display(), r.bytes)),
            Err(_) if cancelled => log.line("Cancelled. Child processes stopped and temporary files removed."),
            Err(e) => log.line(&format!("FAILED: {}", e.summary)),
        }
        self.update_job(&job.id, |j| {
            j.finished_at = Some(crate::now());
            j.exit_code = exit_code;
            j.temporary_paths.clear();
            match result {
                Ok(r) => {
                    j.status = JobStatus::Completed;
                    j.phase = "completed".into();
                    j.output_path = r.output.to_string_lossy().into_owned();
                    j.output_bytes = Some(r.bytes);
                    if j.total_frames > 0 {
                        j.frame = j.total_frames;
                    }
                }
                Err(_) if cancelled => {
                    j.status = JobStatus::Cancelled;
                    j.phase = "cancelled".into();
                }
                Err(e) => {
                    j.status = JobStatus::Failed;
                    j.phase = "failed".into();
                    j.error = Some(e);
                }
            }
        });
        lock(&self.job_tokens).remove(&job.id);
        if completed {
            let _ = self.update_project(&job.project_id, |p| p.last_render_at = Some(crate::now()));
            self.apply_save_space(&job.project_id);
        }
        logs::prune(&self.paths.project_logs(&job.project_id), std::time::SystemTime::now());
        drop(active);
        self.pump_queue();
    }

    fn execute_job(&self, job: &RenderJob, token: &CancelToken, log: &mut LogSink, exit_code: &mut Option<i32>) -> Result<Rendered> {
        let project = self.project(&job.project_id)?;
        match project.env_status {
            EnvStatus::Ready => {}
            EnvStatus::Removed => {
                return Err(Diagnostic::new("The project workspace was removed by Save Space mode.")
                    .suggest("Open the project and prepare the environment again, then re-queue the render."))
            }
            _ => return Err(Diagnostic::new("The project environment is not ready.").suggest("Prepare the environment first.")),
        }
        let root = PathBuf::from(&project.project_root);
        let node = self.node_for(&project)?;
        let cli = remotion::resolve_cli(&root)?;
        let settings = self.settings();
        let req = &job.request;

        // Output folder must exist and be writable.
        let out_dir = PathBuf::from(&req.output_dir);
        let not_writable = |e: std::io::Error| {
            Diagnostic::new(format!("The output folder is not writable: {}", out_dir.display()))
                .cause(e.to_string(), false)
                .suggest("Choose a different output folder.")
        };
        fs::create_dir_all(&out_dir).map_err(not_writable)?;
        let probe = out_dir.join(format!(".urs-write-test-{}", &job.id[..8]));
        fs::write(&probe, b"").map_err(not_writable)?;
        let _ = fs::remove_file(&probe);
        if let Some(free) = fsutil::available_space(&out_dir) {
            if free < MIN_FREE_FOR_RENDER {
                return Err(Diagnostic::new(format!("Only {} MB free on the output drive.", free / 1_048_576))
                    .cause("Insufficient disk space.", false)
                    .suggest("Free disk space or choose another output folder."));
            }
        }

        // App-created temporary paths, registered before anything is written.
        let final_path = out_dir.join(&req.file_name);
        let stem = final_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = remotion::extension_for(&req.codec).unwrap_or("mp4");
        let partial = out_dir.join(format!("{stem}.partial-{}.{ext}", &job.id[..8]));
        let temp = self.paths.temp(&project.id).join(format!("job-{}", job.id));
        fs::create_dir_all(&temp)?;
        self.update_job(&job.id, |j| {
            j.temporary_paths = vec![temp.to_string_lossy().into_owned(), partial.to_string_lossy().into_owned()];
            j.status = JobStatus::Rendering;
            j.phase = "starting".into();
        });

        let spec = RunSpec {
            program: node.node_exe(),
            args: remotion::render_args(&cli, project.report.entry_point.as_deref(), req, &partial, settings.browser_executable.as_deref()),
            cwd: root,
            env: self.child_env(&node, &temp),
            timeout: None,
        };
        log.line(&format!("Remotion {} / Node v{}", project.remotion_version.as_deref().unwrap_or("?"), node.version));
        log.line(&format!("$ {}", spec.display()));

        let (mut frame, mut total, mut phase) = (0u32, 0u32, "starting");
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let out = process::run(&spec, token, |line| {
            log.line(line);
            let lower = line.to_lowercase();
            let update = match remotion::parse_progress(line) {
                Some(p) => {
                    (frame, total, phase) = (p.frame, p.total, p.phase);
                    true
                }
                None if phase == "starting" && lower.contains("bundl") => {
                    phase = "bundling";
                    true
                }
                None => false,
            };
            if update && last_emit.elapsed() > Duration::from_millis(150) {
                last_emit = Instant::now();
                self.emit(Event::Progress { job_id: job.id.clone(), frame, total, phase: phase.to_string() });
            }
        })?;
        *exit_code = out.exit_code;
        self.update_job(&job.id, |j| {
            j.frame = frame;
            j.total_frames = total;
        });
        log.line(&format!("Exit code: {:?} after {:.1}s", out.exit_code, out.duration.as_secs_f64()));
        if out.cancelled || token.is_cancelled() {
            return Err(Diagnostic::new("Cancelled."));
        }
        if !out.success() {
            return Err(diagnose(Stage::Render, &out.command, out.exit_code, &out.tail));
        }

        // Verify before the file gets its final name.
        let bytes = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
        if bytes == 0 {
            let mut d = Diagnostic::new("Remotion reported success but the output file is missing or empty.")
                .cause("The encoder did not write the file.", true)
                .suggest("Check the log, then retry with the h264 codec.");
            d.command = Some(out.command);
            d.excerpt = Some(crate::diagnostics::excerpt(&out.tail, 25));
            return Err(d);
        }
        let output = if req.overwrite { final_path } else { fsutil::unique_path(&final_path) };
        if req.overwrite && output.is_file() {
            fs::remove_file(&output).map_err(|e| Diagnostic::io("Could not replace the existing output file", e))?;
        }
        fsutil::rename_retry(&partial, &output).map_err(|e| Diagnostic::io("Could not move the render into place", e))?;
        Ok(Rendered { output, bytes })
    }

    /// Delete a path the app registered as temporary. Inside the app folder
    /// anything goes through `safe_remove`; outside it, only a single regular
    /// file carrying the app's `.partial-` marker may be removed.
    pub(crate) fn remove_registered_temp(&self, path: &Path) {
        if path.starts_with(&self.paths.base) {
            let _ = fsutil::safe_remove(&self.paths.workspaces(), path);
        } else if path.file_name().is_some_and(|n| n.to_string_lossy().contains(".partial-"))
            && fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        {
            let _ = fs::remove_file(path);
        }
    }

    /// Save Space: after a successful render, delete the extracted workspace
    /// (including node_modules) — but only if the ZIP still exists, so the
    /// only copy of the source is never deleted.
    fn apply_save_space(&self, project_id: &str) {
        let Ok(p) = self.project(project_id) else { return };
        if p.storage_mode != StorageMode::SaveSpace {
            return;
        }
        let pending = lock(&self.db).jobs.iter().any(|j| j.project_id == project_id && !j.status.is_finished());
        if pending || lock(&self.ops).contains_key(project_id) {
            return;
        }
        if self.restorable_zip(&p).is_none() {
            let _ = self.update_project(project_id, |p| {
                p.last_error = Some(
                    Diagnostic::new("Save Space skipped: the original ZIP could not be found, so the workspace was kept.")
                        .cause("Deleting the workspace would remove the only copy of the project source.", false)
                        .suggest("Restore the ZIP to its original location, or switch the project to Fast Reopen."),
                )
            });
            return;
        }
        self.stop_preview_and_wait(project_id);
        let ws = self.paths.workspaces();
        let removed = fsutil::safe_remove(&ws, &self.paths.source(project_id)).is_ok();
        let _ = fsutil::safe_remove(&ws, &self.paths.temp(project_id));
        if removed {
            let _ = self.update_project(project_id, |p| p.env_status = EnvStatus::Removed);
        }
    }
}
