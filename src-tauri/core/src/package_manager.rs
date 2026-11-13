//! Lockfile-driven install plans. One package manager per project, frozen
//! installs whenever a lockfile exists, and app-managed pnpm/Yarn binaries
//! (never global installs).

use crate::model::InspectionReport;
use crate::paths::Paths;
use crate::process::{self, CancelToken, RunSpec};
use crate::runtime::NodeRuntime;
use crate::{Diagnostic, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    Npm,
    Pnpm(String),
    YarnClassic(String),
    YarnBerry { version: String, yarn_path: Option<String> },
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub tool: Tool,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub display: String,
    /// True when no lockfile exists, so the install resolves versions freshly
    /// and writes a new lockfile. Requires explicit user permission.
    pub generates_lockfile: bool,
}

pub fn plan(report: &InspectionReport, safe_mode: bool) -> Plan {
    let locked = report.lockfile.is_some();
    let version = report.package_manager_version.clone();
    let mut env = Vec::new();
    let (tool, name, mut args): (Tool, &str, Vec<&str>) = match report.package_manager.as_str() {
        "pnpm" => (
            Tool::Pnpm(version.unwrap_or_else(|| "9".into())),
            "pnpm",
            if locked { vec!["install", "--frozen-lockfile"] } else { vec!["install", "--no-frozen-lockfile"] },
        ),
        "yarn" if report.yarn_berry => (
            Tool::YarnBerry { version: version.unwrap_or_else(|| "4".into()), yarn_path: report.yarn_path.clone() },
            "yarn",
            if locked { vec!["install", "--immutable"] } else { vec!["install"] },
        ),
        "yarn" => (
            Tool::YarnClassic(version.unwrap_or_else(|| "1.22.22".into())),
            "yarn",
            if locked { vec!["install", "--frozen-lockfile", "--non-interactive"] } else { vec!["install", "--non-interactive"] },
        ),
        _ => (Tool::Npm, "npm", if locked { vec!["ci"] } else { vec!["install"] }),
    };
    if safe_mode {
        if matches!(tool, Tool::YarnBerry { .. }) {
            env.push(("YARN_ENABLE_SCRIPTS".to_string(), "false".to_string()));
        } else {
            args.push("--ignore-scripts");
        }
    }
    let mut display = format!("{name} {}", args.join(" "));
    if safe_mode && matches!(tool, Tool::YarnBerry { .. }) {
        display.push_str(" (YARN_ENABLE_SCRIPTS=false)");
    }
    if tool == Tool::Npm {
        args.extend(["--no-audit", "--no-fund"]);
    }
    Plan { tool, args: args.into_iter().map(String::from).collect(), env, display, generates_lockfile: !locked }
}

pub fn describe(report: &InspectionReport, safe_mode: bool) -> String {
    plan(report, safe_mode).display
}

fn version_ok(v: &str) -> bool {
    !v.is_empty() && v.len() < 40 && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-^~".contains(c))
}

/// Environment shared by installs: app-owned caches, quiet npm.
pub fn cache_env(paths: &Paths) -> Vec<(String, OsString)> {
    let cache = paths.package_cache();
    vec![
        ("npm_config_cache".into(), cache.join("npm").into()),
        ("npm_config_store_dir".into(), cache.join("pnpm-store").into()),
        ("YARN_CACHE_FOLDER".into(), cache.join("yarn").into()),
        ("npm_config_update_notifier".into(), "false".into()),
        ("npm_config_fund".into(), "false".into()),
        ("npm_config_audit".into(), "false".into()),
        ("COREPACK_ENABLE_DOWNLOAD_PROMPT".into(), "0".into()),
    ]
}

/// Return the JavaScript entry file of the package manager, installing pnpm
/// or Yarn into the app's tools folder when necessary.
pub fn ensure_tool(
    paths: &Paths,
    node: &NodeRuntime,
    tool: &Tool,
    project_root: &Path,
    base_env: &[(String, OsString)],
    log: &mut dyn FnMut(&str),
    cancel: &CancelToken,
) -> Result<PathBuf> {
    let (pkg, version, bin): (&str, &str, &[&str]) = match tool {
        Tool::Npm => return Ok(node.npm_cli()),
        Tool::YarnBerry { yarn_path: Some(p), .. } => return Ok(project_root.join(p)),
        Tool::Pnpm(v) => ("pnpm", v, &["pnpm", "bin", "pnpm.cjs"]),
        Tool::YarnClassic(v) => ("yarn", v, &["yarn", "bin", "yarn.js"]),
        Tool::YarnBerry { version, .. } => ("@yarnpkg/cli-dist", version, &["@yarnpkg", "cli-dist", "bin", "yarn.js"]),
    };
    if !version_ok(version) {
        return Err(Diagnostic::new(format!("Unsupported {pkg} version string \"{version}\" in package.json.")));
    }
    let dir = paths.tools().join(format!("{}-{}", pkg.replace(['@', '/'], ""), version.replace(['^', '~'], "")));
    let entry = bin.iter().fold(dir.join("node_modules"), |p, part| p.join(part));
    if entry.is_file() {
        return Ok(entry);
    }
    std::fs::create_dir_all(&dir)?;
    log(&format!("Installing {pkg}@{version} into the app's tools folder (not global)..."));
    let spec = RunSpec {
        program: node.node_exe(),
        args: vec![
            node.npm_cli().into(),
            "install".into(),
            "--prefix".into(),
            dir.clone().into(),
            "--no-audit".into(),
            "--no-fund".into(),
            "--ignore-scripts".into(),
            "--no-package-lock".into(),
            format!("{pkg}@{version}").into(),
        ],
        cwd: dir.clone(),
        env: base_env.to_vec(),
        timeout: Some(Duration::from_secs(600)),
    };
    let out = process::run(&spec, cancel, |l| log(l))?;
    if out.cancelled {
        return Err(Diagnostic::new("Cancelled."));
    }
    if !out.success() || !entry.is_file() {
        let mut d = crate::diagnostics::diagnose(crate::diagnostics::Stage::Install, &out.command, out.exit_code, &out.tail);
        d.summary = format!("Could not install {pkg}@{version} for this project.");
        return Err(d);
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(pm: &str, lock: Option<&str>, berry: bool) -> InspectionReport {
        InspectionReport { package_manager: pm.into(), lockfile: lock.map(String::from), yarn_berry: berry, ..Default::default() }
    }

    #[test]
    fn frozen_installs_when_lockfile_exists() {
        let p = plan(&report("npm", Some("package-lock.json"), false), false);
        assert_eq!(p.args[0], "ci");
        assert!(!p.generates_lockfile);
        assert_eq!(p.display, "npm ci");

        let p = plan(&report("pnpm", Some("pnpm-lock.yaml"), false), false);
        assert_eq!(p.args, ["install", "--frozen-lockfile"]);

        let p = plan(&report("yarn", Some("yarn.lock"), false), false);
        assert!(p.args.contains(&"--frozen-lockfile".to_string()));

        let p = plan(&report("yarn", Some("yarn.lock"), true), false);
        assert_eq!(p.args, ["install", "--immutable"]);
    }

    #[test]
    fn never_npm_install_with_lockfile_and_flags_missing_lockfile() {
        let with = plan(&report("npm", Some("package-lock.json"), false), true);
        assert!(!with.args.contains(&"install".to_string()));
        assert!(with.args.contains(&"--ignore-scripts".to_string()));
        let without = plan(&report("npm", None, false), false);
        assert_eq!(without.args[0], "install");
        assert!(without.generates_lockfile);
    }

    #[test]
    fn safe_mode_for_berry_uses_env() {
        let p = plan(&report("yarn", Some("yarn.lock"), true), true);
        assert!(p.env.iter().any(|(k, v)| k == "YARN_ENABLE_SCRIPTS" && v == "false"));
        assert!(!p.args.contains(&"--ignore-scripts".to_string()));
    }

    #[test]
    fn rejects_odd_version_strings() {
        assert!(version_ok("9.1.0"));
        assert!(version_ok("1.22.22"));
        assert!(!version_ok("9 && calc"));
        assert!(!version_ok(""));
    }
}
