//! Locating and invoking the project's own Remotion CLI.

use crate::model::{CompositionInfo, RenderRequest};
use crate::{Diagnostic, Result};
use serde_json::Value;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Resolve the CLI script from the installed `@remotion/cli` package's own
/// `bin` field, so no path is hard-coded across Remotion versions.
pub fn resolve_cli(project_root: &Path) -> Result<PathBuf> {
    let pkg_dir = project_root.join("node_modules").join("@remotion").join("cli");
    let missing = || {
        Diagnostic::new("The project's local Remotion CLI (@remotion/cli) is not installed.")
            .cause("Dependencies are not installed, or the project does not depend on @remotion/cli.", true)
            .suggest("Use Rebuild environment. If it persists, add @remotion/cli (same version as remotion) to the source project.")
    };
    let text = std::fs::read_to_string(pkg_dir.join("package.json")).map_err(|_| missing())?;
    let pkg: Value = serde_json::from_str(&text).map_err(|_| missing())?;
    let bin = match pkg.get("bin") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Object(map)) => map
            .get("remotion")
            .or_else(|| map.values().next())
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
    .ok_or_else(missing)?;
    if bin.contains("..") {
        return Err(missing());
    }
    let script = pkg_dir.join(bin);
    if script.is_file() { Ok(script) } else { Err(missing()) }
}

pub fn major(version: Option<&str>) -> u32 {
    version
        .and_then(|v| v.trim_start_matches(['^', '~', '=', 'v', '>', '<', ' ']).split('.').next().map(str::to_string))
        .and_then(|m| m.parse().ok())
        .unwrap_or(4)
}

/// IDs become process arguments; they can never be options or contain spaces.
pub fn validate_composition_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 200 || id.starts_with('-') || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Diagnostic::new(format!("\"{id}\" is not a valid composition ID."))
            .suggest("Composition IDs contain letters, numbers and hyphens. Pick one from the detected list."));
    }
    Ok(())
}

pub fn studio_args(cli: &Path, remotion_major: u32, entry: Option<&str>, port: u16) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![cli.into()];
    // `studio` was introduced in v4; earlier versions call it `preview`.
    args.push(if remotion_major >= 4 { "studio" } else { "preview" }.into());
    if let Some(e) = entry {
        args.push(e.into());
    }
    args.push(format!("--port={port}").into());
    args.push("--no-open".into());
    args
}

pub fn compositions_args(cli: &Path, entry: Option<&str>, browser: Option<&str>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![cli.into(), "compositions".into()];
    if let Some(e) = entry {
        args.push(e.into());
    }
    if let Some(b) = browser {
        args.push(format!("--browser-executable={b}").into());
    }
    args
}

pub const CODECS: &[(&str, &str)] = &[("h264", "mp4"), ("h265", "mp4"), ("vp8", "webm"), ("vp9", "webm"), ("prores", "mov"), ("gif", "gif")];

pub fn extension_for(codec: &str) -> Option<&'static str> {
    CODECS.iter().find(|(c, _)| *c == codec).map(|(_, e)| *e)
}

pub fn validate_request(req: &RenderRequest) -> Result<()> {
    validate_composition_id(&req.composition_id)?;
    if extension_for(&req.codec).is_none() {
        return Err(Diagnostic::new(format!("Unsupported codec \"{}\".", req.codec)));
    }
    if let Some(s) = req.scale {
        if !(s > 0.0 && s <= 16.0) {
            return Err(Diagnostic::new("Scale must be greater than 0 and at most 16."));
        }
    }
    if req.concurrency == Some(0) {
        return Err(Diagnostic::new("Concurrency must be at least 1."));
    }
    if let (Some(a), Some(b)) = (req.frame_start, req.frame_end) {
        if b < a {
            return Err(Diagnostic::new("The frame range end must not be before its start."));
        }
    }
    if req.frame_end.is_some() && req.frame_start.is_none() {
        return Err(Diagnostic::new("A frame range needs a start frame."));
    }
    Ok(())
}

pub fn render_args(cli: &Path, entry: Option<&str>, req: &RenderRequest, output: &Path, browser: Option<&str>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![cli.into(), "render".into()];
    if let Some(e) = entry {
        args.push(e.into());
    }
    args.push(req.composition_id.as_str().into());
    args.push(output.into());
    args.push(format!("--codec={}", req.codec).into());
    if let Some(c) = req.concurrency {
        args.push(format!("--concurrency={c}").into());
    }
    if let Some(s) = req.scale.filter(|s| (*s - 1.0).abs() > f64::EPSILON) {
        args.push(format!("--scale={s}").into());
    }
    match (req.frame_start, req.frame_end) {
        (Some(a), Some(b)) => args.push(format!("--frames={a}-{b}").into()),
        (Some(a), None) => args.push(format!("--frames={a}").into()),
        _ => {}
    }
    if let Some(b) = browser {
        args.push(format!("--browser-executable={b}").into());
    }
    args
}

/// Parse `remotion compositions` output: `<id>  <fps>  <width>x<height>  <frames> (...)`.
pub fn parse_compositions(output: &str) -> Vec<CompositionInfo> {
    let mut out: Vec<CompositionInfo> = Vec::new();
    for line in output.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 4 {
            continue;
        }
        let (Ok(fps), Some((w, h)), Ok(frames)) = (t[1].parse::<f64>(), t[2].split_once('x'), t[3].parse::<u32>()) else { continue };
        let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) else { continue };
        if out.iter().any(|c| c.id == t[0]) {
            continue;
        }
        out.push(CompositionInfo {
            id: t[0].to_string(),
            width: Some(w),
            height: Some(h),
            fps: Some(fps),
            duration_in_frames: Some(frames),
            source: "remotion-cli".into(),
        });
    }
    out
}

#[derive(Debug, PartialEq)]
pub struct Progress {
    pub frame: u32,
    pub total: u32,
    pub phase: &'static str,
}

/// Extract "N/M" progress from a CLI output line, across Remotion versions.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let lower = line.to_lowercase();
    let phase = if lower.contains("encod") || lower.contains("stitch") || lower.contains("mux") {
        "encoding"
    } else if lower.contains("render") {
        "rendering"
    } else {
        return None;
    };
    let bytes = line.as_bytes();
    // Scan from the end of the line. Older CLIs print a step counter such as
    // "(2/3) Rendering frames 10/150"; parenthesised pairs are skipped.
    for slash in (1..bytes.len()).rev() {
        if bytes[slash] != b'/' || !bytes[slash - 1].is_ascii_digit() || !bytes.get(slash + 1).is_some_and(|b| b.is_ascii_digit()) {
            continue;
        }
        let start = (0..slash).rev().take_while(|&i| bytes[i].is_ascii_digit()).last()?;
        let end = (slash + 1..bytes.len()).take_while(|&i| bytes[i].is_ascii_digit()).last()? + 1;
        if start > 0 && bytes[start - 1] == b'(' && bytes.get(end) == Some(&b')') {
            continue;
        }
        let frame: u32 = line[start..slash].parse().ok()?;
        let total: u32 = line[slash + 1..end].parse().ok()?;
        return (total > 0 && frame <= total).then_some(Progress { frame, total, phase });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_cli_from_bin_field() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(resolve_cli(tmp.path()).is_err());
        let dir = tmp.path().join("node_modules/@remotion/cli");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"version":"4.0.1","bin":{"remotion":"remotion-cli.js","remotionb":"remotionb-cli.js"}}"#).unwrap();
        assert!(resolve_cli(tmp.path()).is_err()); // script missing
        std::fs::write(dir.join("remotion-cli.js"), "").unwrap();
        assert_eq!(resolve_cli(tmp.path()).unwrap(), dir.join("remotion-cli.js"));
        std::fs::write(dir.join("package.json"), r#"{"bin":"../../../evil.js"}"#).unwrap();
        assert!(resolve_cli(tmp.path()).is_err());
    }

    #[test]
    fn studio_command_depends_on_version() {
        let a = studio_args(Path::new("cli.js"), 4, Some("src/index.ts"), 3123);
        assert_eq!(a[1], "studio");
        assert!(a.contains(&OsString::from("--port=3123")));
        assert_eq!(studio_args(Path::new("cli.js"), 3, None, 1)[1], "preview");
        assert_eq!(major(Some("^3.3.102")), 3);
        assert_eq!(major(Some("4.0.100")), 4);
        assert_eq!(major(None), 4);
    }

    #[test]
    fn render_args_and_validation() {
        let mut req = RenderRequest { composition_id: "Main".into(), codec: "h264".into(), scale: Some(0.5), concurrency: Some(2), frame_start: Some(0), frame_end: Some(29), ..Default::default() };
        validate_request(&req).unwrap();
        let args = render_args(Path::new("cli.js"), Some("src/index.ts"), &req, Path::new("out.mp4"), None);
        let s: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(s, ["cli.js", "render", "src/index.ts", "Main", "out.mp4", "--codec=h264", "--concurrency=2", "--scale=0.5", "--frames=0-29"]);
        req.composition_id = "--props=/etc/passwd".into();
        assert!(validate_request(&req).is_err());
        req.composition_id = "a b".into();
        assert!(validate_request(&req).is_err());
        req.composition_id = "Main".into();
        req.codec = "exe".into();
        assert!(validate_request(&req).is_err());
        req.codec = "vp9".into();
        req.frame_end = Some(0);
        req.frame_start = Some(5);
        assert!(validate_request(&req).is_err());
        assert_eq!(extension_for("prores"), Some("mov"));
    }

    #[test]
    fn parses_compositions_table() {
        let out = "Bundling 100%\nThe following compositions are available:\n\nMain            30      1920x1080      150 (5.00 sec)\nshort-clip      29.97   1080x1920      60 (2.00 sec)\nnot a row here\n";
        let c = parse_compositions(out);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].id.as_str(), c[0].width, c[0].duration_in_frames), ("Main", Some(1920), Some(150)));
        assert_eq!(c[1].fps, Some(29.97));
    }

    #[test]
    fn parses_progress_lines() {
        assert_eq!(parse_progress("Rendered 30/150, time remaining: 4s"), Some(Progress { frame: 30, total: 150, phase: "rendering" }));
        assert_eq!(parse_progress("Encoded 150/150"), Some(Progress { frame: 150, total: 150, phase: "encoding" }));
        assert_eq!(parse_progress("Rendering frames (8x)  ━━━━━━  12/90"), Some(Progress { frame: 12, total: 90, phase: "rendering" }));
        assert_eq!(parse_progress("(2/3)        Rendering frames (2x) 7/20"), Some(Progress { frame: 7, total: 20, phase: "rendering" }));
        assert_eq!(parse_progress("Rendering frames (2x) 19/20 (3/3) ━━"), Some(Progress { frame: 19, total: 20, phase: "rendering" }));
        assert_eq!(parse_progress("(1/3) Rendering"), None);
        assert_eq!(parse_progress("Downloaded 3/10"), None);
        assert_eq!(parse_progress("Rendering to /a/b/c.mp4"), None);
    }
}
