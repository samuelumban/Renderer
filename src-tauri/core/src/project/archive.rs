//! Safe ZIP extraction. Archives are untrusted: entry names are validated,
//! symlinks are skipped, and size limits are enforced on bytes actually
//! written (declared sizes are not trusted).

use crate::process::CancelToken;
use crate::{Diagnostic, Result};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct ExtractLimits {
    pub max_zip_bytes: u64,
    pub max_extracted_bytes: u64,
    pub max_entries: u64,
}

#[derive(Debug, Default)]
pub struct ExtractSummary {
    pub files: u64,
    pub bytes: u64,
    pub warnings: Vec<String>,
}

/// Directories that are never extracted from a project ZIP: dependencies are
/// installed from the lockfile instead, and VCS/OS metadata is not needed.
const SKIPPED_DIRS: &[&str] = &["node_modules", ".git", "__MACOSX"];

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Convert an archive entry name to a safe relative path, or explain why not.
pub fn sanitize_entry_name(name: &str) -> std::result::Result<PathBuf, String> {
    if name.contains('\0') {
        return Err("contains a NUL byte".into());
    }
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err("absolute path".into());
    }
    let mut out = PathBuf::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err("path traversal ('..')".into()),
            _ => {}
        }
        if part.chars().any(|c| c.is_control() || "<>:\"|?*".contains(c)) {
            return Err("invalid character in name (possible drive or stream path)".into());
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err("name ends with a dot or space".into());
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if RESERVED.contains(&stem.as_str()) {
            return Err("reserved Windows device name".into());
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        return Err("empty path".into());
    }
    Ok(out)
}

fn is_symlink(mode: Option<u32>) -> bool {
    mode.is_some_and(|m| m & 0o170000 == 0o120000)
}

fn bad_zip(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::new(msg).suggest("Re-export the project as a standard .zip file and import it again.")
}

fn open(zip_path: &Path, limits: &ExtractLimits) -> Result<zip::ZipArchive<File>> {
    let meta = fs::metadata(zip_path).map_err(|e| Diagnostic::io("Cannot read the ZIP file", e))?;
    if meta.len() > limits.max_zip_bytes {
        return Err(Diagnostic::new(format!(
            "The ZIP is {} MB, above the {} MB limit.",
            meta.len() / 1_048_576,
            limits.max_zip_bytes / 1_048_576
        ))
        .suggest("Remove node_modules and build output from the ZIP, or raise the limit in Settings."));
    }
    let file = File::open(zip_path).map_err(|e| Diagnostic::io("Cannot open the ZIP file", e))?;
    let archive = zip::ZipArchive::new(file).map_err(|e| bad_zip(format!("Not a valid ZIP archive: {e}")))?;
    if archive.len() as u64 > limits.max_entries {
        return Err(bad_zip(format!("The ZIP has {} entries, above the limit of {}.", archive.len(), limits.max_entries)));
    }
    Ok(archive)
}

/// Sum of declared uncompressed sizes, used only for a disk-space estimate.
pub fn declared_size(zip_path: &Path, limits: &ExtractLimits) -> Result<u64> {
    let mut archive = open(zip_path, limits)?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        if let Ok(f) = archive.by_index_raw(i) {
            total = total.saturating_add(f.size());
        }
    }
    Ok(total)
}

/// Extract into `dest` (must not already contain files that matter).
/// Any unsafe entry aborts the whole import: nothing is executed, and the
/// caller removes the partially extracted folder.
pub fn extract_zip(
    zip_path: &Path,
    dest: &Path,
    limits: &ExtractLimits,
    skip_project_dirs: bool,
    cancel: &CancelToken,
) -> Result<ExtractSummary> {
    let mut archive = open(zip_path, limits)?;
    fs::create_dir_all(dest)?;
    let mut summary = ExtractSummary::default();
    let mut skipped_dirs: Vec<&str> = Vec::new();
    let mut symlinks = 0u64;

    for i in 0..archive.len() {
        if cancel.is_cancelled() {
            return Err(Diagnostic::new("Import cancelled."));
        }
        let mut entry = archive.by_index(i).map_err(|e| bad_zip(format!("Corrupt ZIP entry: {e}")))?;
        let raw_name = entry.name().to_string();
        let rel = sanitize_entry_name(&raw_name).map_err(|why| {
            Diagnostic::new(format!("Unsafe entry in ZIP: \"{raw_name}\" ({why})."))
                .cause("The archive contains a path that could write outside the project folder.", false)
                .suggest("Do not import this archive. Re-export the project from a trusted source.")
        })?;
        if skip_project_dirs {
            if let Some(dir) = SKIPPED_DIRS.iter().find(|d| rel.components().any(|c| c.as_os_str() == **d)) {
                if !skipped_dirs.contains(dir) {
                    skipped_dirs.push(dir);
                }
                continue;
            }
        }
        if is_symlink(entry.unix_mode()) {
            symlinks += 1;
            continue;
        }
        let target = dest.join(&rel);
        if !target.starts_with(dest) {
            return Err(Diagnostic::new(format!("Unsafe entry in ZIP: \"{raw_name}\".")));
        }
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let remaining = limits.max_extracted_bytes.saturating_sub(summary.bytes);
        let mut out = File::create(&target)?;
        let written = io::copy(&mut (&mut entry).take(remaining.saturating_add(1)), &mut out)
            .map_err(|e| bad_zip(format!("Failed to extract \"{raw_name}\": {e}")))?;
        if written > remaining {
            return Err(Diagnostic::new(format!(
                "The archive expands beyond the {} MB extraction limit.",
                limits.max_extracted_bytes / 1_048_576
            ))
            .cause("The ZIP is very large or is a decompression bomb.", true)
            .suggest("Remove large files from the ZIP, or raise the limit in Settings if you trust it."));
        }
        summary.bytes += written;
        summary.files += 1;
    }
    for d in skipped_dirs {
        summary.warnings.push(format!("Skipped \"{d}\" from the ZIP (not needed; dependencies are installed from the lockfile)."));
    }
    if symlinks > 0 {
        summary.warnings.push(format!("Skipped {symlinks} symbolic link(s) in the ZIP for safety."));
    }
    Ok(summary)
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::io::Write;
    use std::path::Path;
    use zip::write::SimpleFileOptions;

    pub fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in entries {
            if name.ends_with('/') {
                w.add_directory(*name, SimpleFileOptions::default()).unwrap();
            } else {
                w.start_file(*name, SimpleFileOptions::default()).unwrap();
                w.write_all(data).unwrap();
            }
        }
        w.finish().unwrap();
    }

    pub fn make_zip_with_symlink(path: &Path) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        w.start_file("proj/package.json", SimpleFileOptions::default()).unwrap();
        w.write_all(b"{}").unwrap();
        w.add_symlink("proj/evil", "/etc/passwd", SimpleFileOptions::default()).unwrap();
        w.finish().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    const LIMITS: ExtractLimits = ExtractLimits { max_zip_bytes: 10 << 20, max_extracted_bytes: 10 << 20, max_entries: 1000 };

    #[test]
    fn sanitize_rejects_traversal_and_absolute() {
        for bad in [
            "../evil.txt",
            "a/../../evil.txt",
            "..\\evil.txt",
            "a\\..\\..\\evil",
            "/etc/passwd",
            "\\Windows\\system32\\x",
            "C:\\Windows\\x",
            "C:/x",
            "a/file.txt:stream",
            "a/NUL",
            "con.txt",
            "a/trailing./x",
            "a\0b",
            "",
            "./",
        ] {
            assert!(sanitize_entry_name(bad).is_err(), "should reject {bad:?}");
        }
        assert_eq!(sanitize_entry_name("proj/src/index.ts").unwrap(), PathBuf::from("proj/src/index.ts"));
        assert_eq!(sanitize_entry_name("proj\\src\\./a.ts").unwrap(), PathBuf::from("proj/src/a.ts"));
        assert_eq!(sanitize_entry_name("a/..b/c").unwrap(), PathBuf::from("a/..b/c"));
    }

    #[test]
    fn traversal_zip_is_rejected_and_writes_nothing_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("evil.zip");
        make_zip(&zip, &[("ok.txt", b"fine"), ("../escaped.txt", b"bad"), ("sub/../../escaped2.txt", b"bad")]);
        let dest = tmp.path().join("ws").join("source");
        let err = extract_zip(&zip, &dest, &LIMITS, true, &CancelToken::new()).unwrap_err();
        assert!(err.summary.contains("Unsafe entry"), "{err}");
        assert!(!tmp.path().join("ws/escaped.txt").exists());
        assert!(!tmp.path().join("escaped.txt").exists());
        assert!(!tmp.path().join("escaped2.txt").exists());
    }

    #[test]
    fn extracts_and_skips_heavy_dirs_and_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("p.zip");
        make_zip(
            &zip,
            &[
                ("proj/", b""),
                ("proj/package.json", b"{}"),
                ("proj/src/index.ts", b"x"),
                ("proj/node_modules/a/index.js", b"x"),
                ("proj/.git/config", b"x"),
                ("__MACOSX/proj/._package.json", b"x"),
            ],
        );
        let dest = tmp.path().join("out");
        let s = extract_zip(&zip, &dest, &LIMITS, true, &CancelToken::new()).unwrap();
        assert_eq!(s.files, 2);
        assert!(dest.join("proj/src/index.ts").exists());
        assert!(!dest.join("proj/node_modules").exists());
        assert!(!dest.join("proj/.git").exists());
        assert_eq!(s.warnings.len(), 3);

        let zip2 = tmp.path().join("s.zip");
        make_zip_with_symlink(&zip2);
        let dest2 = tmp.path().join("out2");
        let s = extract_zip(&zip2, &dest2, &LIMITS, true, &CancelToken::new()).unwrap();
        assert!(fs::symlink_metadata(dest2.join("proj/evil")).is_err());
        assert!(s.warnings.iter().any(|w| w.contains("symbolic link")));
    }

    #[test]
    fn size_limits_are_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("big.zip");
        let big = vec![0u8; 200_000]; // compresses to almost nothing
        make_zip(&zip, &[("a.bin", &big), ("b.bin", &big)]);
        let limits = ExtractLimits { max_extracted_bytes: 250_000, ..LIMITS };
        let err = extract_zip(&zip, &tmp.path().join("o"), &limits, true, &CancelToken::new()).unwrap_err();
        assert!(err.summary.contains("extraction limit"), "{err}");
        let limits = ExtractLimits { max_zip_bytes: 10, ..LIMITS };
        assert!(extract_zip(&zip, &tmp.path().join("o2"), &limits, true, &CancelToken::new()).is_err());
        let limits = ExtractLimits { max_entries: 1, ..LIMITS };
        assert!(extract_zip(&zip, &tmp.path().join("o3"), &limits, true, &CancelToken::new()).is_err());
    }

    #[test]
    fn cancellation_stops_extraction() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("p.zip");
        make_zip(&zip, &[("a.txt", b"x")]);
        let token = CancelToken::new();
        token.cancel();
        assert!(extract_zip(&zip, &tmp.path().join("o"), &LIMITS, true, &token).is_err());
        assert!(!tmp.path().join("o/a.txt").exists());
    }

    #[test]
    fn not_a_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.zip");
        fs::write(&p, b"hello").unwrap();
        assert!(extract_zip(&p, &tmp.path().join("o"), &LIMITS, true, &CancelToken::new()).is_err());
    }
}
