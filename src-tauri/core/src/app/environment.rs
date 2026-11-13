//! Environment preparation: managed Node runtime + lockfile install.

use super::{lock, Core};
use crate::diagnostics::{diagnose, Stage};
use crate::logs::LogSink;
use crate::model::EnvStatus;
use crate::process::{self, CancelToken, RunSpec};
use crate::project::{archive, inspect};
use crate::{fsutil, package_manager, remotion, runtime, Diagnostic, Result};
use serde::Deserialize;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrepareOptions {
    /// Run package lifecycle scripts. false = safer install (`--ignore-scripts`).
    pub allow_scripts: bool,
    /// Permission to install without a lockfile (non-reproducible).
    pub allow_generate_lockfile: bool,
    /// Delete node_modules first (Rebuild environment).
    pub clean: bool,
}

const MIN_FREE_FOR_INSTALL: u64 = 1024 * 1024 * 1024;

impl Core {
    /// Start preparing in the background. Calling this is the user's explicit
    /// approval to run the install for this project.
    pub fn prepare(self: &Arc<Self>, id: &str, opts: PrepareOptions) -> Result<()> {
        self.project(id)?;
        let cancel = CancelToken::new();
        {
            let mut ops = lock(&self.ops);
            if ops.contains_key(id) {
                return Err(Diagnostic::new("The environment is already being prepared."));
            }
            if lock(&self.db).jobs.iter().any(|j| j.project_id == id && j.status.is_active()) {
                return Err(Diagnostic::new("A render is running for this project.").suggest("Wait for it or cancel it first."));
            }
            ops.insert(id.to_string(), cancel.clone());
        }
        self.update_project(id, |p| {
            p.env_status = EnvStatus::Preparing;
            p.install_approved = true;
            p.safe_install = !opts.allow_scripts;
            p.last_error = None;
        })?;
        let core = self.clone();
        let id = id.to_string();
        std::thread::spawn(move || {
            let _active = core.begin_active();
            let mut log = LogSink::create(core.paths.project_logs(&id).join("environment.log"), format!("env:{id}"), core.emitter());
            let result = core.prepare_blocking(&id, opts, &cancel, &mut log);
            let cancelled = cancel.is_cancelled();
            match &result {
                Ok(()) => log.line("Environment ready."),
                Err(_) if cancelled => log.line("Cancelled."),
                Err(e) => log.line(&format!("FAILED: {}", e.summary)),
            }
            lock(&core.ops).remove(&id);
            let _ = core.update_project(&id, |p| match result {
                Ok(()) => {
                    p.env_status = EnvStatus::Ready;
                    p.last_error = None;
                }
                Err(_) if cancelled => p.env_status = EnvStatus::NotPrepared,
                Err(e) => {
                    p.env_status = EnvStatus::Failed;
                    p.last_error = Some(e);
                }
            });
            drop(_active);
            core.enforce_cache_limit();
        });
        Ok(())
    }

    pub fn cancel_prepare(&self, id: &str) {
        if let Some(t) = lock(&self.ops).get(id) {
            t.cancel();
        }
    }

    /// Re-extract the source from the retained ZIP (Save Space reopen).
    fn restore_source(&self, id: &str, cancel: &CancelToken, log: &mut LogSink) -> Result<()> {
        let p = self.project(id)?;
        if PathBuf::from(&p.project_root).join("package.json").is_file() {
            return Ok(());
        }
        let zip = self.restorable_zip(&p).ok_or_else(|| {
            Diagnostic::new("The project source was removed and its ZIP can no longer be found.")
                .cause(format!("Expected the ZIP at {}", p.original_zip_path), false)
                .suggest("Import the ZIP again.")
        })?;
        log.line(&format!("Restoring project source from {}", zip.display()));
        let source = self.paths.source(id);
        fsutil::safe_remove(&self.paths.workspaces(), &source)?;
        archive::extract_zip(&zip, &source, &self.limits(), true, cancel)?;
        let root = inspect::find_project_root(&source).ok_or_else(|| Diagnostic::new("No package.json was found in the ZIP."))?;
        self.update_project(id, |p| p.project_root = root.to_string_lossy().into_owned())?;
        self.reinspect(id)?;
        Ok(())
    }

    fn prepare_blocking(&self, id: &str, opts: PrepareOptions, cancel: &CancelToken, log: &mut LogSink) -> Result<()> {
        let check = || if cancel.is_cancelled() { Err(Diagnostic::new("Cancelled.")) } else { Ok(()) };
        self.restore_source(id, cancel, log)?;
        let project = self.project(id)?;
        let root = PathBuf::from(&project.project_root);
        let settings = self.settings();
        if let Some(blocking) = project.report.issues.iter().find(|i| i.severity == "error") {
            return Err(Diagnostic::new(blocking.message.clone()).suggest("Fix the source project and import it again."));
        }
        if opts.clean {
            log.line("Removing node_modules (rebuild requested)...");
            fsutil::safe_remove(&self.paths.workspaces(), &root.join("node_modules"))?;
        }

        // 1. Node.js runtime (app-managed; system Node is never used or changed).
        let version = runtime::resolve(&self.paths, &settings, project.report.node_requirement.as_deref(), &mut |l| log.line(l))?;
        check()?;
        let node = runtime::ensure(&self.paths, &settings, &version, &mut |l| log.line(l), cancel)?;
        log.line(&format!("Node runtime: v{} ({})", node.version, node.node_exe().display()));
        self.update_project(id, |p| {
            p.node_version = Some(node.version.clone());
            p.selected_node_runtime = Some(node.node_exe().to_string_lossy().into_owned());
        })?;

        // 2. Disk space.
        if let Some(free) = fsutil::available_space(&root) {
            log.line(&format!("Free disk space: {} MB", free / 1_048_576));
            if free < MIN_FREE_FOR_INSTALL {
                return Err(Diagnostic::new(format!("Only {} MB of disk space is free; installing dependencies needs at least 1 GB.", free / 1_048_576))
                    .cause("Insufficient disk space.", false)
                    .suggest("Free space in Storage (clean temporary files, clear the package cache) and retry."));
            }
        }

        // 3. Install plan from the lockfile.
        let plan = package_manager::plan(&project.report, !opts.allow_scripts);
        if plan.generates_lockfile && !opts.allow_generate_lockfile {
            return Err(Diagnostic::new("This project has no lockfile, so the install is not reproducible.")
                .cause("Without a lockfile the package manager picks the newest versions allowed by package.json.", false)
                .suggest("Confirm \"Install without lockfile\" to proceed, or add a lockfile to the source project."));
        }
        let temp = self.paths.temp(id).join("install");
        std::fs::create_dir_all(&temp)?;
        let mut env = self.child_env(&node, &temp);
        let tool = package_manager::ensure_tool(&self.paths, &node, &plan.tool, &root, &env, &mut |l| log.line(l), cancel)?;
        check()?;
        env.extend(plan.env.iter().map(|(k, v)| (k.clone(), OsString::from(v))));

        // 4. Install.
        let mut args: Vec<OsString> = vec![tool.into()];
        args.extend(plan.args.iter().map(OsString::from));
        let spec = RunSpec {
            program: node.node_exe(),
            args,
            cwd: root.clone(),
            env,
            timeout: Some(Duration::from_secs(settings.install_timeout_minutes * 60)),
        };
        log.line(&format!("Install command: {}", plan.display));
        log.line(&format!("$ {}", spec.display()));
        if opts.allow_scripts {
            log.line("Lifecycle scripts: enabled (approved by user).");
        } else {
            log.line("Lifecycle scripts: disabled (safe install). Some projects may not work in this mode.");
        }
        let out = process::run(&spec, cancel, |l| log.line(l))?;
        let _ = fsutil::safe_remove(&self.paths.workspaces(), &temp);
        log.line(&format!("Exit code: {:?} after {:.1}s", out.exit_code, out.duration.as_secs_f64()));
        if out.cancelled {
            return Err(Diagnostic::new("Cancelled."));
        }
        if out.timed_out {
            let mut d = Diagnostic::new(format!("Dependency installation timed out after {} minutes.", settings.install_timeout_minutes))
                .cause("A slow network or a hanging lifecycle script.", true)
                .suggest("Retry, or raise the install timeout in Settings.");
            d.command = Some(out.command);
            d.excerpt = Some(crate::diagnostics::excerpt(&out.tail, 25));
            return Err(d);
        }
        if !out.success() {
            return Err(diagnose(Stage::Install, &plan.display, out.exit_code, &out.tail));
        }

        // 5. Verify the local Remotion CLI and refresh the report with
        //    installed versions.
        let cli = remotion::resolve_cli(&root)?;
        log.line(&format!("Local Remotion CLI: {}", cli.display()));
        let p = self.reinspect(id)?;
        log.line(&format!("Remotion version (installed): {}", p.remotion_version.as_deref().unwrap_or("unknown")));
        Ok(())
    }
}
