//! Static project inspection. Reads files only; never executes project code
//! or package scripts.

use crate::model::{CompositionInfo, FontReport, InspectionReport, Issue, PackageVersion};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MAX_SCAN_FILES: usize = 4000;
const MAX_SCAN_BYTES: u64 = 1_500_000;
const FONT_EXT: &[&str] = &["ttf", "otf", "woff", "woff2"];
const CODE_EXT: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];
const STYLE_EXT: &[&str] = &["css", "scss", "sass", "less", "html"];
const IGNORED_DIRS: &[&str] = &["node_modules", ".git", "dist", "build", "out", ".next", ".remotion"];

fn issue(severity: &str, code: &str, message: impl Into<String>, confirmed: bool) -> Issue {
    Issue { severity: severity.into(), code: code.into(), message: message.into(), confirmed }
}

fn read_package_json(dir: &Path) -> Option<Value> {
    let text = fs::read_to_string(dir.join("package.json")).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn dependencies(pkg: &Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for key in ["optionalDependencies", "devDependencies", "dependencies"] {
        if let Some(map) = pkg.get(key).and_then(Value::as_object) {
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    out.insert(k.clone(), s.to_string());
                }
            }
        }
    }
    out
}

fn is_remotion_pkg(name: &str) -> bool {
    name == "remotion" || name.starts_with("@remotion/")
}

/// Locate the project root inside the extracted tree: the shallowest folder
/// with a package.json, preferring one that depends on Remotion.
pub fn find_project_root(source: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<(bool, usize, PathBuf)> = Vec::new();
    let walker = walkdir::WalkDir::new(source).max_depth(5).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| !IGNORED_DIRS.contains(&e.file_name().to_string_lossy().as_ref())) {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_dir() && entry.path().join("package.json").is_file() {
            let has_remotion = read_package_json(entry.path())
                .map(|p| dependencies(&p).keys().any(|k| is_remotion_pkg(k)))
                .unwrap_or(false);
            candidates.push((!has_remotion, entry.depth(), entry.path().to_path_buf()));
        }
    }
    candidates.sort();
    candidates.into_iter().next().map(|c| c.2)
}

struct Lockfiles {
    npm: Option<&'static str>,
    pnpm: bool,
    yarn: bool,
    bun: bool,
}

fn detect_lockfiles(root: &Path) -> Lockfiles {
    let has = |n: &str| root.join(n).is_file();
    Lockfiles {
        npm: ["package-lock.json", "npm-shrinkwrap.json"].into_iter().find(|n| has(n)),
        pnpm: has("pnpm-lock.yaml"),
        yarn: has("yarn.lock"),
        bun: has("bun.lockb") || has("bun.lock"),
    }
}

fn first_line_value(text: &str, key: &str) -> Option<String> {
    text.lines().take(40).find_map(|l| {
        let l = l.trim();
        l.strip_prefix(key).map(|v| v.trim().trim_matches(['\'', '"']).to_string())
    })
}

fn pnpm_major_for_lockfile(text: &str) -> &'static str {
    match first_line_value(text, "lockfileVersion:").as_deref() {
        Some(v) if v.starts_with("9") => "9",
        Some(v) if v.starts_with("6") => "8",
        Some("5.4") => "7",
        Some(v) if v.starts_with("5") => "6",
        _ => "9",
    }
}

fn yarn_berry_major(text: &str) -> &'static str {
    let version = text
        .lines()
        .skip_while(|l| !l.starts_with("__metadata:"))
        .nth(1)
        .and_then(|l| l.trim().strip_prefix("version:"))
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(8);
    match version {
        0..=4 => "2",
        5..=7 => "3",
        _ => "4",
    }
}

fn locked_versions(root: &Path, names: &[String], lockfile: Option<&str>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(lockfile) = lockfile else { return out };
    let Ok(text) = fs::read_to_string(root.join(lockfile)) else { return out };
    match lockfile {
        "package-lock.json" | "npm-shrinkwrap.json" => {
            let Ok(v) = serde_json::from_str::<Value>(&text) else { return out };
            for name in names {
                let ver = v
                    .pointer(&format!("/packages/node_modules~1{}/version", name.replace('/', "~1")))
                    .or_else(|| v.get("dependencies").and_then(|d| d.get(name)).and_then(|d| d.get("version")))
                    .and_then(Value::as_str);
                if let Some(ver) = ver {
                    out.insert(name.clone(), ver.to_string());
                }
            }
        }
        "yarn.lock" => {
            for name in names {
                let re = Regex::new(&format!(
                    r#"(?m)^"?{}@[^\n]*:\n\s+version:?\s+"?([^"\s]+)"?"#,
                    regex::escape(name)
                ));
                if let Some(c) = re.ok().and_then(|re| re.captures(&text)) {
                    out.insert(name.clone(), c[1].to_string());
                }
            }
        }
        "pnpm-lock.yaml" => {
            for name in names {
                let re = Regex::new(&format!(r#"(?m)^\s+'?/?{}[@/](\d[^:('\s]*)"#, regex::escape(name)));
                if let Some(c) = re.ok().and_then(|re| re.captures(&text)) {
                    out.insert(name.clone(), c[1].to_string());
                }
            }
        }
        _ => {}
    }
    out
}

pub fn installed_version(root: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(root.join("node_modules").join(name).join("package.json")).ok()?;
    serde_json::from_str::<Value>(&text).ok()?.get("version")?.as_str().map(str::to_string)
}

fn detect_entry_point(root: &Path, pkg: &Value) -> Option<String> {
    let exists = |rel: &str| {
        let clean = rel.trim_start_matches("./");
        (!clean.contains("..") && root.join(clean).is_file()).then(|| clean.replace('\\', "/"))
    };
    if let Some(scripts) = pkg.get("scripts").and_then(Value::as_object) {
        for script in scripts.values().filter_map(Value::as_str) {
            let tokens: Vec<&str> = script.split_whitespace().collect();
            for w in tokens.windows(3) {
                let is_cmd = ["studio", "preview", "render", "still", "compositions", "bundle"].contains(&w[1]);
                if w[0].ends_with("remotion") && is_cmd && !w[2].starts_with('-') {
                    if let Some(found) = exists(w[2].trim_matches(['"', '\''])) {
                        return Some(found);
                    }
                }
            }
        }
    }
    for dir in ["src", "remotion", "src/remotion", "."] {
        for ext in ["ts", "tsx", "js", "jsx"] {
            let rel = if dir == "." { format!("index.{ext}") } else { format!("{dir}/index.{ext}") };
            if let Some(found) = exists(&rel) {
                let text = fs::read_to_string(root.join(&found)).unwrap_or_default();
                if text.contains("registerRoot") {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn scan_files(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(root).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| e.depth() == 0 || !IGNORED_DIRS.contains(&e.file_name().to_string_lossy().as_ref())) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = entry.path().extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        out.push((entry.path().to_path_buf(), ext));
        if out.len() >= MAX_SCAN_FILES {
            break;
        }
    }
    out
}

fn read_small(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if meta.len() > MAX_SCAN_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

fn prop_number(block: &str, name: &str) -> Option<f64> {
    let re = Regex::new(&format!(r"\b{name}\s*=\s*\{{\s*([0-9]+(?:\.[0-9]+)?)\s*\}}")).ok()?;
    re.captures(block)?.get(1)?.as_str().parse().ok()
}

/// Best-effort scan for `<Composition id="...">`. Dynamic IDs and computed
/// props cannot be seen statically; the Remotion CLI gives the real list.
pub fn scan_compositions(text: &str) -> Vec<CompositionInfo> {
    static TAG: OnceLock<Regex> = OnceLock::new();
    static ID: OnceLock<Regex> = OnceLock::new();
    let tag = re(&TAG, r"<(?:Composition|Still)\b");
    let id = re(&ID, r#"\bid\s*=\s*\{?\s*["'`]([^"'`${}]+)["'`]"#);
    let mut out = Vec::new();
    for m in tag.find_iter(text) {
        let mut end = (m.end() + 1500).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut block = &text[m.end()..end];
        if let Some(next) = tag.find(block) {
            block = &block[..next.start()];
        }
        let Some(c) = id.captures(block) else { continue };
        out.push(CompositionInfo {
            id: c[1].to_string(),
            width: prop_number(block, "width").map(|v| v as u32),
            height: prop_number(block, "height").map(|v| v as u32),
            fps: prop_number(block, "fps"),
            duration_in_frames: prop_number(block, "durationInFrames").map(|v| v as u32),
            source: "static-scan".into(),
        });
    }
    out
}

const GENERIC_FAMILIES: &[&str] = &[
    "serif", "sans-serif", "monospace", "cursive", "fantasy", "system-ui", "ui-serif", "ui-sans-serif",
    "ui-monospace", "ui-rounded", "math", "emoji", "fangsong", "inherit", "initial", "unset", "revert",
    "-apple-system", "blinkmacsystemfont",
];

/// Fonts that ship with Windows 10/11 (or are aliased by Chromium).
const WINDOWS_FONTS: &[&str] = &[
    "arial", "arial black", "bahnschrift", "calibri", "cambria", "candara", "comic sans ms", "consolas",
    "constantia", "corbel", "courier", "courier new", "franklin gothic medium", "gabriola", "georgia",
    "helvetica", "impact", "lucida console", "lucida sans unicode", "microsoft sans serif", "palatino linotype",
    "segoe ui", "segoe ui emoji", "segoe ui symbol", "segoe print", "segoe script", "sitka", "tahoma", "times",
    "times new roman", "trebuchet ms", "verdana", "webdings", "wingdings", "apple color emoji", "noto color emoji",
    "menlo", "monaco", "sf pro", "helvetica neue", "roboto mono",
];

fn normalize_family(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

fn split_families(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|f| f.trim().trim_matches(['"', '\'', '`']).trim().to_string())
        .filter(|f| {
            !f.is_empty()
                && f.len() < 60
                && !f.contains("var(")
                && !f.contains('$')
                && !f.contains('{')
                && !f.contains('!')
                && f.chars().any(|c| c.is_alphabetic())
        })
        .collect()
}

fn scan_fonts(root: &Path, files: &[(PathBuf, String)]) -> FontReport {
    static FACE: OnceLock<Regex> = OnceLock::new();
    static CSS_FAMILY: OnceLock<Regex> = OnceLock::new();
    static JS_FAMILY: OnceLock<Regex> = OnceLock::new();
    static FONTFACE_API: OnceLock<Regex> = OnceLock::new();
    static LOAD_FAMILY: OnceLock<Regex> = OnceLock::new();
    static GOOGLE_PKG: OnceLock<Regex> = OnceLock::new();
    static EXTERNAL: OnceLock<Regex> = OnceLock::new();
    let face = re(&FACE, r"(?s)@font-face\s*\{(.*?)\}");
    let css_family = re(&CSS_FAMILY, r"font-family\s*:\s*([^;}{\n]+)");
    let js_family = re(&JS_FAMILY, r#"fontFamily\s*[:=]\s*\{?\s*["'`]([^"'`\n]+)["'`]"#);
    let fontface_api = re(&FONTFACE_API, r#"new\s+FontFace\(\s*["'`]([^"'`]+)["'`]"#);
    let load_family = re(&LOAD_FAMILY, r#"\bfamily\s*:\s*["'`]([^"'`\n]+)["'`]"#);
    let google_pkg = re(&GOOGLE_PKG, r#"@remotion/google-fonts/([A-Za-z0-9]+)"#);
    let external = re(&EXTERNAL, r#"https?://[^\s"'`)<>]*(?:fonts\.googleapis\.com|fonts\.gstatic\.com|use\.typekit\.net|fonts\.bunny\.net|\.woff2?|\.ttf|\.otf)[^\s"'`)<>]*"#);

    let mut font_files = BTreeSet::new();
    let mut declared = BTreeSet::new();
    let mut referenced = BTreeSet::new();
    let mut externals = BTreeSet::new();
    let mut google = BTreeSet::new();

    for (path, ext) in files {
        if FONT_EXT.contains(&ext.as_str()) {
            if let Ok(rel) = path.strip_prefix(root) {
                font_files.insert(rel.to_string_lossy().replace('\\', "/"));
            }
            continue;
        }
        let is_code = CODE_EXT.contains(&ext.as_str());
        if !is_code && !STYLE_EXT.contains(&ext.as_str()) {
            continue;
        }
        let Some(text) = read_small(path) else { continue };
        for block in face.captures_iter(&text) {
            for fam in css_family.captures_iter(&block[1]) {
                declared.extend(split_families(&fam[1]));
            }
        }
        let without_faces = face.replace_all(&text, "");
        for fam in css_family.captures_iter(&without_faces) {
            referenced.extend(split_families(&fam[1]));
        }
        for m in external.find_iter(&text) {
            externals.insert(m.as_str().trim_end_matches([';', ',']).to_string());
        }
        if is_code {
            for fam in js_family.captures_iter(&text) {
                referenced.extend(split_families(&fam[1]));
            }
            for fam in fontface_api.captures_iter(&text) {
                declared.insert(fam[1].to_string());
            }
            if text.contains("loadFont") || text.contains("FontFace") {
                for fam in load_family.captures_iter(&text) {
                    declared.insert(fam[1].to_string());
                }
            }
            for g in google_pkg.captures_iter(&text) {
                google.insert(g[1].to_string());
            }
        }
    }

    let file_stems: Vec<String> = font_files
        .iter()
        .filter_map(|f| Path::new(f).file_stem().map(|s| normalize_family(&s.to_string_lossy())))
        .collect();
    let declared_norm: BTreeSet<String> = declared.iter().chain(google.iter()).map(|f| normalize_family(f)).collect();
    let possibly_missing: Vec<String> = referenced
        .iter()
        .filter(|fam| {
            let lower = fam.to_lowercase();
            let norm = normalize_family(fam);
            !GENERIC_FAMILIES.contains(&lower.as_str())
                && !WINDOWS_FONTS.contains(&lower.as_str())
                && !declared_norm.contains(&norm)
                && !file_stems.iter().any(|stem| stem.starts_with(&norm))
        })
        .cloned()
        .collect();

    FontReport {
        files: font_files.into_iter().collect(),
        declared_families: declared.into_iter().collect(),
        referenced_families: referenced.into_iter().collect(),
        external: externals.into_iter().collect(),
        google_fonts: google.into_iter().collect(),
        possibly_missing,
    }
}

/// Build the inspection report for an extracted project.
pub fn inspect(root: &Path, source: &Path) -> InspectionReport {
    let mut r = InspectionReport {
        root_rel: root.strip_prefix(source).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default(),
        inspected_at: crate::now(),
        package_manager: "unknown".into(),
        ..Default::default()
    };
    let Some(pkg) = read_package_json(root) else {
        r.issues.push(issue("error", "missing-package-json", "No readable package.json was found in the project.", true));
        return r;
    };
    r.package_name = pkg.get("name").and_then(Value::as_str).map(str::to_string);
    let deps = dependencies(&pkg);

    // --- package manager and lockfile -------------------------------------
    let locks = detect_lockfiles(root);
    let declared_pm = pkg.get("packageManager").and_then(Value::as_str).map(|s| {
        let s = s.split('+').next().unwrap_or(s);
        let (name, version) = s.split_once('@').unwrap_or((s, ""));
        (name.to_string(), version.to_string())
    });
    let lock_count = locks.npm.is_some() as u8 + locks.pnpm as u8 + locks.yarn as u8;
    let declared_name = declared_pm.as_ref().map(|d| d.0.as_str());
    let (pm, lockfile): (&str, Option<&str>) = match (locks.npm, locks.pnpm, locks.yarn) {
        (Some(l), _, _) if declared_name.is_none_or(|d| d == "npm") || (!locks.pnpm && !locks.yarn) => ("npm", Some(l)),
        (_, true, _) if declared_name != Some("yarn") || !locks.yarn => ("pnpm", Some("pnpm-lock.yaml")),
        (_, _, true) => ("yarn", Some("yarn.lock")),
        (Some(l), _, _) => ("npm", Some(l)),
        _ => (declared_name.filter(|d| ["npm", "pnpm", "yarn"].contains(d)).unwrap_or("npm"), None),
    };
    r.package_manager = pm.to_string();
    r.lockfile = lockfile.map(str::to_string);
    if lock_count > 1 {
        r.issues.push(issue("warning", "multiple-lockfiles", format!("Several lockfiles are present; using {} with {}. Package managers are never mixed.", pm, lockfile.unwrap_or("-")), true));
    }
    if let (Some(d), Some(_)) = (declared_name, lockfile) {
        if d != pm {
            r.issues.push(issue("warning", "package-manager-mismatch", format!("package.json declares \"{d}\" but the lockfile belongs to {pm}. The lockfile is used."), true));
        }
    }
    if lockfile.is_none() {
        if locks.bun {
            r.issues.push(issue("warning", "bun-lockfile", "Only a Bun lockfile was found. Bun is not supported; installing with npm would not be reproducible and needs your permission.", true));
        } else {
            r.issues.push(issue("warning", "missing-lockfile", "No lockfile found. Dependency resolution may not be reproducible; installing needs your permission.", true));
        }
    }
    let lock_text = lockfile.and_then(|l| fs::read_to_string(root.join(l)).ok()).unwrap_or_default();
    if pm == "yarn" {
        let declared_major = declared_pm.as_ref().filter(|d| d.0 == "yarn").and_then(|d| d.1.split('.').next().and_then(|m| m.parse::<u32>().ok()));
        r.yarn_berry = lock_text.contains("__metadata:") || declared_major.is_some_and(|m| m >= 2) || (lockfile.is_none() && root.join(".yarnrc.yml").is_file());
        if let Ok(rc) = fs::read_to_string(root.join(".yarnrc.yml")) {
            r.yarn_path = first_line_value(&rc, "yarnPath:").filter(|p| !p.contains("..") && root.join(p).is_file());
        }
    }
    r.package_manager_version = match (pm, declared_pm.as_ref().filter(|d| d.0 == pm && !d.1.is_empty())) {
        (_, Some(d)) => Some(d.1.clone()),
        ("pnpm", None) => Some(pnpm_major_for_lockfile(&lock_text).to_string()),
        ("yarn", None) if r.yarn_berry => Some(yarn_berry_major(&lock_text).to_string()),
        ("yarn", None) => Some("1.22.22".to_string()),
        _ => None,
    };
    r.install_command = crate::package_manager::describe(&r, false);

    // --- Remotion / React versions ----------------------------------------
    let names: Vec<String> = deps.keys().filter(|k| is_remotion_pkg(k)).cloned().collect();
    let locked = locked_versions(root, &names, lockfile);
    r.remotion_packages = names
        .iter()
        .map(|n| PackageVersion {
            name: n.clone(),
            declared: deps.get(n).cloned(),
            locked: locked.get(n).cloned(),
            installed: installed_version(root, n),
        })
        .collect();
    r.react_version = installed_version(root, "react").or_else(|| deps.get("react").cloned());
    if !deps.contains_key("remotion") && !deps.contains_key("@remotion/cli") {
        r.issues.push(issue("error", "no-remotion", "package.json does not depend on \"remotion\" or \"@remotion/cli\". This does not look like a Remotion project.", true));
    } else if !deps.contains_key("@remotion/cli") {
        r.issues.push(issue("warning", "no-remotion-cli", "\"@remotion/cli\" is not a direct dependency. Preview and render need the project's local Remotion CLI.", false));
    }
    check_version_alignment(&mut r);

    // --- Node requirement --------------------------------------------------
    if let Some(req) = pkg.pointer("/engines/node").and_then(Value::as_str) {
        r.node_requirement = Some(req.to_string());
        r.node_requirement_source = Some("package.json engines.node".into());
    } else {
        for f in [".nvmrc", ".node-version"] {
            if let Ok(text) = fs::read_to_string(root.join(f)) {
                let v = text.trim().trim_start_matches('v');
                if v.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    r.node_requirement = Some(v.to_string());
                    r.node_requirement_source = Some(f.to_string());
                    break;
                }
            }
        }
    }
    if let Some(req) = &r.node_requirement {
        if crate::runtime::parse_range(req).is_none() {
            r.issues.push(issue("warning", "node-requirement-unparsed", format!("Could not understand the Node.js requirement \"{req}\"; the default runtime will be used."), true));
        }
    } else {
        r.issues.push(issue("info", "node-default", "The project does not state a Node.js version; the app's default LTS runtime will be used.", true));
    }

    // --- entry point, compositions, fonts ----------------------------------
    r.entry_point = detect_entry_point(root, &pkg);
    if r.entry_point.is_none() {
        r.issues.push(issue("warning", "no-entry-point", "Could not find the Remotion entry point (a file calling registerRoot). Remotion will try its own defaults.", false));
    }
    let files = scan_files(root);
    let mut seen = BTreeSet::new();
    for (path, ext) in &files {
        if CODE_EXT.contains(&ext.as_str()) {
            if let Some(text) = read_small(path) {
                for c in scan_compositions(&text) {
                    if seen.insert(c.id.clone()) {
                        r.compositions.push(c);
                    }
                }
            }
        }
    }
    r.fonts = scan_fonts(root, &files);
    if !r.fonts.possibly_missing.is_empty() {
        r.issues.push(issue("warning", "fonts-possibly-missing", format!("Font families referenced but not found in the project: {}. Chromium may substitute them silently.", r.fonts.possibly_missing.join(", ")), false));
    }
    if !r.fonts.external.is_empty() || !r.fonts.google_fonts.is_empty() {
        r.issues.push(issue("info", "fonts-external", "The project loads fonts from the network; rendering needs internet access for them.", false));
    }
    r
}

/// Remotion requires `remotion` and every `@remotion/*` package to be the
/// same version. Report mismatches; never rewrite package.json.
pub fn check_version_alignment(r: &mut InspectionReport) {
    r.issues.retain(|i| !i.code.starts_with("remotion-version"));
    let distinct = |get: fn(&PackageVersion) -> Option<&String>| -> BTreeSet<String> {
        r.remotion_packages.iter().filter_map(get).cloned().collect()
    };
    let installed = distinct(|p| p.installed.as_ref());
    let locked = distinct(|p| p.locked.as_ref());
    let declared = distinct(|p| p.declared.as_ref());
    let list = |get: fn(&PackageVersion) -> Option<&String>| {
        r.remotion_packages
            .iter()
            .filter_map(|p| get(p).map(|v| format!("{}@{}", p.name, v)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if installed.len() > 1 {
        let msg = format!("Installed Remotion packages have different versions: {}. Remotion requires them to match.", list(|p| p.installed.as_ref()));
        r.issues.push(issue("warning", "remotion-version-mismatch", msg, true));
    } else if installed.is_empty() && locked.len() > 1 {
        let msg = format!("The lockfile pins different Remotion package versions: {}. Remotion requires them to match.", list(|p| p.locked.as_ref()));
        r.issues.push(issue("warning", "remotion-version-mismatch", msg, true));
    } else if installed.is_empty() && locked.is_empty() && declared.len() > 1 {
        let msg = format!("package.json declares different Remotion package versions: {}. They may resolve to mismatched versions.", list(|p| p.declared.as_ref()));
        r.issues.push(issue("warning", "remotion-version-declared-mismatch", msg, false));
    }
}

/// Best known Remotion version: installed, else locked, else declared.
pub fn remotion_version(r: &InspectionReport) -> Option<String> {
    let pick = |name: &str| {
        r.remotion_packages
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.installed.clone().or(p.locked.clone()).or(p.declared.clone()))
    };
    pick("remotion").or_else(|| pick("@remotion/cli"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for (name, content) in files {
            let p = tmp.path().join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        tmp
    }

    const PKG: &str = r#"{
      "name": "my-video",
      "scripts": {"dev": "remotion studio src/index.ts", "build": "remotion render src/index.ts Main out.mp4"},
      "dependencies": {"remotion": "4.0.100", "@remotion/cli": "4.0.100", "react": "18.2.0"},
      "engines": {"node": ">=18"}
    }"#;

    #[test]
    fn finds_root_in_enclosing_folder_and_prefers_remotion() {
        let tmp = project(&[
            ("wrap/tools/package.json", r#"{"name":"tools"}"#),
            ("wrap/video/package.json", PKG),
            ("wrap/video/node_modules/x/package.json", "{}"),
        ]);
        assert_eq!(find_project_root(tmp.path()).unwrap(), tmp.path().join("wrap/video"));
        let flat = project(&[("package.json", PKG)]);
        assert_eq!(find_project_root(flat.path()).unwrap(), flat.path());
        let none = project(&[("readme.md", "x")]);
        assert!(find_project_root(none.path()).is_none());
    }

    #[test]
    fn npm_project_report() {
        let lock = r#"{"lockfileVersion":3,"packages":{"":{},"node_modules/remotion":{"version":"4.0.100"},"node_modules/@remotion/cli":{"version":"4.0.100"}}}"#;
        let tmp = project(&[
            ("package.json", PKG),
            ("package-lock.json", lock),
            ("src/index.ts", "import {registerRoot} from 'remotion'; registerRoot(Root);"),
            ("src/Root.tsx", r#"<Composition id="Main" component={A} width={1920} height={1080} fps={30} durationInFrames={150} /> <Composition id='Second' component={B} />"#),
        ]);
        let r = inspect(tmp.path(), tmp.path());
        assert_eq!(r.package_manager, "npm");
        assert_eq!(r.lockfile.as_deref(), Some("package-lock.json"));
        assert!(r.install_command.contains("npm ci"), "{}", r.install_command);
        assert_eq!(remotion_version(&r).as_deref(), Some("4.0.100"));
        assert_eq!(r.node_requirement.as_deref(), Some(">=18"));
        assert_eq!(r.entry_point.as_deref(), Some("src/index.ts"));
        assert_eq!(r.compositions.len(), 2);
        assert_eq!(r.compositions[0].width, Some(1920));
        assert_eq!(r.compositions[0].duration_in_frames, Some(150));
        assert!(!r.has_blocking());
        assert!(!r.issues.iter().any(|i| i.code.contains("mismatch")));
    }

    #[test]
    fn lockfile_detection_per_manager() {
        let pnpm = project(&[("package.json", PKG), ("pnpm-lock.yaml", "lockfileVersion: '6.0'\n\npackages:\n\n  /remotion@4.0.100:\n    resolution: {}\n")]);
        let r = inspect(pnpm.path(), pnpm.path());
        assert_eq!((r.package_manager.as_str(), r.package_manager_version.as_deref()), ("pnpm", Some("8")));
        assert!(r.install_command.contains("--frozen-lockfile"));
        assert_eq!(r.remotion_packages.iter().find(|p| p.name == "remotion").unwrap().locked.as_deref(), Some("4.0.100"));

        let yarn1 = project(&[("package.json", PKG), ("yarn.lock", "# yarn lockfile v1\n\nremotion@4.0.100:\n  version \"4.0.100\"\n")]);
        let r = inspect(yarn1.path(), yarn1.path());
        assert_eq!(r.package_manager, "yarn");
        assert!(!r.yarn_berry);
        assert!(r.install_command.contains("--frozen-lockfile"));
        assert_eq!(r.remotion_packages.iter().find(|p| p.name == "remotion").unwrap().locked.as_deref(), Some("4.0.100"));

        let berry = project(&[("package.json", PKG), ("yarn.lock", "__metadata:\n  version: 8\n\n\"remotion@npm:4.0.100\":\n  version: 4.0.100\n")]);
        let r = inspect(berry.path(), berry.path());
        assert!(r.yarn_berry);
        assert_eq!(r.package_manager_version.as_deref(), Some("4"));
        assert!(r.install_command.contains("--immutable"));

        let none = project(&[("package.json", PKG)]);
        let r = inspect(none.path(), none.path());
        assert_eq!(r.package_manager, "npm");
        assert!(r.lockfile.is_none());
        assert!(r.issues.iter().any(|i| i.code == "missing-lockfile"));

        let mixed = project(&[
            ("package.json", r#"{"packageManager":"pnpm@9.1.0+sha512.abc","dependencies":{"remotion":"4.0.1","@remotion/cli":"4.0.1"}}"#),
            ("package-lock.json", "{}"),
            ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
        ]);
        let r = inspect(mixed.path(), mixed.path());
        assert_eq!((r.package_manager.as_str(), r.package_manager_version.as_deref()), ("pnpm", Some("9.1.0")));
        assert!(r.issues.iter().any(|i| i.code == "multiple-lockfiles"));

        let mismatch = project(&[("package.json", r#"{"packageManager":"yarn@1.22.0","dependencies":{"remotion":"4.0.1","@remotion/cli":"4.0.1"}}"#), ("package-lock.json", "{}")]);
        let r = inspect(mismatch.path(), mismatch.path());
        assert_eq!(r.package_manager, "npm");
        assert!(r.issues.iter().any(|i| i.code == "package-manager-mismatch"));
    }

    #[test]
    fn version_mismatch_and_non_remotion() {
        let tmp = project(&[("package.json", r#"{"dependencies":{"remotion":"4.0.100","@remotion/cli":"4.0.50","@remotion/player":"^4.0.0"}}"#)]);
        let r = inspect(tmp.path(), tmp.path());
        let i = r.issues.iter().find(|i| i.code == "remotion-version-declared-mismatch").unwrap();
        assert!(!i.confirmed);

        let lock = r#"{"packages":{"node_modules/remotion":{"version":"4.0.100"},"node_modules/@remotion/cli":{"version":"4.0.50"}}}"#;
        let tmp = project(&[("package.json", r#"{"dependencies":{"remotion":"^4.0.0","@remotion/cli":"^4.0.0"}}"#), ("package-lock.json", lock)]);
        let r = inspect(tmp.path(), tmp.path());
        assert!(r.issues.iter().any(|i| i.code == "remotion-version-mismatch" && i.confirmed));

        let plain = project(&[("package.json", r#"{"dependencies":{"react":"18.0.0"}}"#)]);
        let r = inspect(plain.path(), plain.path());
        assert!(r.has_blocking());
        assert!(r.issues.iter().any(|i| i.code == "no-remotion"));
    }

    #[test]
    fn installed_version_wins() {
        let tmp = project(&[
            ("package.json", r#"{"dependencies":{"remotion":"^3.3.0","@remotion/cli":"^3.3.0"}}"#),
            ("node_modules/remotion/package.json", r#"{"version":"3.3.102"}"#),
            ("node_modules/@remotion/cli/package.json", r#"{"version":"3.3.102"}"#),
        ]);
        let r = inspect(tmp.path(), tmp.path());
        assert_eq!(remotion_version(&r).as_deref(), Some("3.3.102"));
    }

    #[test]
    fn font_scan_separates_declared_from_possibly_missing() {
        let tmp = project(&[
            ("package.json", PKG),
            ("public/fonts/Brand-Bold.woff2", "x"),
            ("src/style.css", "@font-face { font-family: 'Brand'; src: url(/fonts/Brand-Bold.woff2); }\n.a { font-family: Brand, 'Mystery Sans', Arial, sans-serif; }\n@import url('https://fonts.googleapis.com/css2?family=Lato');"),
            ("src/A.tsx", "import {loadFont} from '@remotion/google-fonts/Inter';\nconst s = {fontFamily: 'Inter'}; const t = {fontFamily: \"Ghost Font\"};"),
        ]);
        let r = inspect(tmp.path(), tmp.path());
        assert_eq!(r.fonts.files, vec!["public/fonts/Brand-Bold.woff2"]);
        assert!(r.fonts.declared_families.contains(&"Brand".to_string()));
        assert_eq!(r.fonts.google_fonts, vec!["Inter"]);
        assert!(r.fonts.external.iter().any(|u| u.contains("fonts.googleapis.com")));
        assert_eq!(r.fonts.possibly_missing, vec!["Ghost Font", "Mystery Sans"]);
        let i = r.issues.iter().find(|i| i.code == "fonts-possibly-missing").unwrap();
        assert_eq!((i.severity.as_str(), i.confirmed), ("warning", false));
    }
}
