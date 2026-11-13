//! Remotion Studio preview: the project's own CLI, a free localhost port,
//! and a tracked process tree that is killed on stop or app exit.

use super::{lock, Core};
use crate::diagnostics::{diagnose, Stage};
use crate::logs::LogSink;
use crate::model::{EnvStatus, Event, PreviewInfo};
use crate::process::{self, CancelToken, RunSpec};
use crate::{fsutil, remotion, Diagnostic, Result};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Remotion Studio listens on every network interface and has no option to
/// change that. This preload script runs inside the preview's Node process
/// (via NODE_OPTIONS, without touching project files) and pins every
/// wildcard `listen()` to the loopback interface.
const LOCALHOST_ONLY_SHIM: &str = r#"'use strict';
const net = require('net');
const original = net.Server.prototype.listen;
const wildcard = (h) => h === undefined || h === null || h === '' || h === '0.0.0.0' || h === '::' || h === '::0';
net.Server.prototype.listen = function (...args) {
  const first = args[0];
  if (first !== null && typeof first === 'object') {
    if (!('path' in first) && !('fd' in first) && !('_handle' in first) && wildcard(first.host)) {
      args[0] = Object.assign({}, first, {host: '127.0.0.1'});
    }
  } else if (typeof first === 'number' || (typeof first === 'string' && /^\d+$/.test(first))) {
    if (typeof args[1] === 'string') {
      if (wildcard(args[1])) args[1] = '127.0.0.1';
    } else {
      args.splice(1, 0, '127.0.0.1');
    }
  }
  return original.apply(this, args);
};
"#;

pub(crate) struct Entry {
    pub token: CancelToken,
    pub info: PreviewInfo,
}

fn free_port() -> Result<u16> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| Diagnostic::io("Could not allocate a local port", e))
}

fn port_open(port: u16) -> bool {
    let addrs = [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))];
    addrs.iter().any(|a| TcpStream::connect_timeout(a, Duration::from_millis(300)).is_ok())
}

impl Core {
    pub fn list_previews(&self) -> Vec<PreviewInfo> {
        lock(&self.previews).values().map(|e| e.info.clone()).collect()
    }

    fn set_preview(&self, id: &str, f: impl FnOnce(&mut PreviewInfo)) {
        if let Some(e) = lock(&self.previews).get_mut(id) {
            f(&mut e.info);
        }
        self.emit(Event::Previews);
    }

    pub fn start_preview(self: &Arc<Self>, id: &str) -> Result<PreviewInfo> {
        let project = self.project(id)?;
        if project.env_status != EnvStatus::Ready {
            return Err(Diagnostic::new("The environment is not ready.").suggest("Prepare the environment first."));
        }
        if let Some(e) = lock(&self.previews).get(id) {
            if e.info.status != "failed" {
                return Ok(e.info.clone());
            }
        }
        let root = PathBuf::from(&project.project_root);
        let node = self.node_for(&project)?;
        let cli = remotion::resolve_cli(&root)?;
        let port = free_port()?;
        let token = CancelToken::new();
        let info = PreviewInfo {
            project_id: id.to_string(),
            port,
            // The app only ever opens the loopback address.
            url: format!("http://localhost:{port}"),
            status: "starting".into(),
            error: None,
        };
        lock(&self.previews).insert(id.to_string(), Entry { token: token.clone(), info: info.clone() });
        self.emit(Event::Previews);

        let temp = self.paths.temp(id).join("preview");
        let _ = std::fs::create_dir_all(&temp);
        let mut env = self.child_env(&node, &temp);
        let shim = self.paths.app_dir().join("localhost-only.cjs");
        std::fs::write(&shim, LOCALHOST_ONLY_SHIM).map_err(|e| Diagnostic::io("Could not prepare the preview sandbox", e))?;
        let mut node_options = std::env::var("NODE_OPTIONS").unwrap_or_default();
        node_options.push_str(&format!(" --require \"{}\"", shim.to_string_lossy().replace('\\', "/")));
        env.push(("NODE_OPTIONS".to_string(), node_options.trim().into()));
        let spec = RunSpec {
            program: node.node_exe(),
            args: remotion::studio_args(&cli, remotion::major(project.remotion_version.as_deref()), project.report.entry_point.as_deref(), port),
            cwd: root,
            env,
            timeout: None,
        };

        // Readiness probe.
        let (core, pid, probe_token) = (self.clone(), id.to_string(), token.clone());
        std::thread::spawn(move || {
            let started = Instant::now();
            while !probe_token.is_cancelled() && started.elapsed() < Duration::from_secs(300) {
                if lock(&core.previews).get(&pid).is_none_or(|e| e.info.port != port || e.info.status != "starting") {
                    return;
                }
                if port_open(port) {
                    core.set_preview(&pid, |i| i.status = "running".into());
                    return;
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        });

        let (core, pid) = (self.clone(), id.to_string());
        std::thread::spawn(move || {
            let mut log = LogSink::create(core.paths.project_logs(&pid).join("preview.log"), format!("preview:{pid}"), core.emitter());
            log.line(&format!("$ {}", spec.display()));
            let result = process::run(&spec, &token, |l| log.line(l));
            let _ = fsutil::safe_remove(&core.paths.workspaces(), &temp);
            let failure = match result {
                Ok(out) if token.is_cancelled() || out.cancelled => None,
                Ok(out) => Some(diagnose(Stage::Preview, &out.command, out.exit_code, &out.tail)),
                Err(e) => Some(e),
            };
            {
                let mut previews = lock(&core.previews);
                // Only touch our own entry (a restart may have replaced it).
                if previews.get(&pid).is_some_and(|e| e.info.port == port) {
                    match failure {
                        None => {
                            previews.remove(&pid);
                        }
                        Some(d) => {
                            if let Some(e) = previews.get_mut(&pid) {
                                e.info.status = "failed".into();
                                e.info.error = Some(d);
                            }
                        }
                    }
                }
            }
            log.line("Preview server stopped.");
            core.emit(Event::Previews);
        });
        Ok(info)
    }

    pub fn stop_preview(&self, id: &str) {
        let mut previews = lock(&self.previews);
        match previews.get(id) {
            Some(e) if e.info.status == "failed" => {
                previews.remove(id);
            }
            Some(e) => e.token.cancel(),
            None => {}
        }
        drop(previews);
        self.emit(Event::Previews);
    }

    pub(crate) fn stop_preview_and_wait(&self, id: &str) {
        self.stop_preview(id);
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&self.previews).contains_key(id) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Core {
    /// Ask the project's Remotion CLI for the authoritative composition list
    /// (bundles the project, so it can take a while).
    pub fn detect_compositions(&self, id: &str) -> Result<Vec<crate::model::CompositionInfo>> {
        let project = self.project(id)?;
        if project.env_status != EnvStatus::Ready {
            return Err(Diagnostic::new("The environment is not ready.").suggest("Prepare the environment first."));
        }
        let root = PathBuf::from(&project.project_root);
        let node = self.node_for(&project)?;
        let cli = remotion::resolve_cli(&root)?;
        let temp = self.paths.temp(id).join(format!("compositions-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&temp)?;
        let spec = RunSpec {
            program: node.node_exe(),
            args: remotion::compositions_args(&cli, project.report.entry_point.as_deref(), self.settings().browser_executable.as_deref()),
            cwd: root,
            env: self.child_env(&node, &temp),
            timeout: Some(Duration::from_secs(600)),
        };
        let mut log = LogSink::create(self.paths.project_logs(id).join("compositions.log"), format!("env:{id}"), self.emitter());
        log.line(&format!("$ {}", spec.display()));
        let mut all = String::new();
        let active = self.begin_active();
        let result = process::run(&spec, &CancelToken::new(), |l| {
            log.line(l);
            all.push_str(l);
            all.push('\n');
        });
        drop(active);
        let _ = fsutil::safe_remove(&self.paths.workspaces(), &temp);
        let out = result?;
        if !out.success() {
            return Err(diagnose(Stage::Compositions, &out.command, out.exit_code, &out.tail));
        }
        let comps = remotion::parse_compositions(&all);
        if comps.is_empty() {
            let mut d = Diagnostic::new("Remotion listed no compositions.")
                .cause("The project registers no compositions, or the CLI output format was not recognised.", true)
                .suggest("Type the composition ID manually in the Render tab.");
            d.excerpt = Some(crate::diagnostics::excerpt(&out.tail, 15));
            return Err(d);
        }
        let stored = comps.clone();
        self.update_project(id, |p| p.report.compositions = stored)?;
        Ok(comps)
    }
}
