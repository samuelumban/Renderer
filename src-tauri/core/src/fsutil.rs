//! File helpers. `safe_remove` is the only function in the app that deletes
//! directories recursively, and it refuses anything outside an allowed root.

use crate::{Diagnostic, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?)?;
    fs::rename(&tmp, path)
}

/// Size of a tree in bytes. Symlinks are not followed.
pub fn dir_size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Free bytes on the volume containing `path` (or its nearest existing ancestor).
pub fn available_space(path: &Path) -> Option<u64> {
    let mut p = path;
    loop {
        if p.exists() {
            return fs4::available_space(p).ok();
        }
        p = p.parent()?;
    }
}

fn refuse(target: &Path, why: &str) -> Diagnostic {
    Diagnostic::new(format!("Refused to delete {}: {why}.", target.display()))
        .cause("Cleanup is restricted to app-created paths.", false)
}

/// Resolve `target` and check it lies strictly inside `allowed_root`.
/// The final component is not followed if it is a symlink, so a link that
/// points outside the root can be removed without touching what it points at.
pub fn resolve_inside(allowed_root: &Path, target: &Path) -> Result<PathBuf> {
    if !target.is_absolute() {
        return Err(refuse(target, "path is not absolute"));
    }
    if target.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(refuse(target, "path contains '..'"));
    }
    let root = fs::canonicalize(allowed_root).map_err(|_| refuse(target, "allowed root does not exist"))?;
    let (parent, name) = match (target.parent(), target.file_name()) {
        (Some(p), Some(n)) => (p, n),
        _ => return Err(refuse(target, "path has no parent")),
    };
    let parent = fs::canonicalize(parent).map_err(|_| refuse(target, "parent does not exist"))?;
    let resolved = parent.join(name);
    if !resolved.starts_with(&root) || resolved == root {
        return Err(refuse(target, "path is outside the app workspace"));
    }
    Ok(resolved)
}

/// Delete a file, symlink or directory tree, only if it is strictly inside
/// `allowed_root`. A missing target is not an error.
pub fn safe_remove(allowed_root: &Path, target: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(target) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    let resolved = resolve_inside(allowed_root, target)?;
    let mut last = None;
    for attempt in 0..4 {
        let r = if meta.file_type().is_symlink() || meta.is_file() {
            fs::remove_file(&resolved).or_else(|_| fs::remove_dir(&resolved))
        } else {
            remove_tree(&resolved)
        };
        match r {
            Ok(()) => return Ok(()),
            Err(_) if fs::symlink_metadata(&resolved).is_err() => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(150 * (attempt + 1)));
    }
    Err(Diagnostic::new(format!(
        "Could not delete {}: {}",
        target.display(),
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
    .cause("A file is still in use by another program.", true)
    .suggest("Close programs using the folder and run Clean now again."))
}

fn remove_tree(path: &Path) -> std::io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(first) => {
            // Read-only files (common inside node_modules on Windows) block
            // deletion; clear the flag and retry once.
            for e in walkdir::WalkDir::new(path).follow_links(false).into_iter().filter_map(|e| e.ok()) {
                if let Ok(m) = e.metadata() {
                    let mut p = m.permissions();
                    if p.readonly() {
                        #[allow(clippy::permissions_set_readonly_false)]
                        p.set_readonly(false);
                        let _ = fs::set_permissions(e.path(), p);
                    }
                }
            }
            fs::remove_dir_all(path).map_err(|_| first)
        }
    }
}

/// Rename with a few retries: on Windows a freshly written tree can be
/// briefly locked by antivirus or the search indexer.
pub fn rename_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = None;
    for attempt in 0..6 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(200 * (attempt + 1)));
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

/// Delete the children of `dir` (not `dir` itself). Returns bytes freed.
pub fn clear_dir(allowed_root: &Path, dir: &Path) -> Result<u64> {
    let before = dir_size(dir);
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            safe_remove(allowed_root, &e.path())?;
        }
    }
    Ok(before.saturating_sub(dir_size(dir)))
}

/// `name.ext`, or `name (2).ext` ... if it already exists.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new("."));
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{stem}-{}{ext}", uuid::Uuid::new_v4().simple())))
}

/// Strip characters that are invalid in Windows file names.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() { "render".into() } else { cleaned }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_inside_root_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("app");
        let outside = tmp.path().join("user-data");
        fs::create_dir_all(root.join("ws/a")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "x").unwrap();
        fs::write(root.join("ws/a/f.txt"), "x").unwrap();

        safe_remove(&root, &root.join("ws/a")).unwrap();
        assert!(!root.join("ws/a").exists());

        assert!(safe_remove(&root, &outside).is_err());
        assert!(safe_remove(&root, &root).is_err());
        assert!(safe_remove(&root, &root.join("ws").join("..").join("..").join("user-data")).is_err());
        assert!(safe_remove(&root, Path::new("relative/path")).is_ok()); // missing => no-op
        assert!(outside.join("keep.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("app");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        // Deleting through the link is refused: the parent resolves outside.
        assert!(safe_remove(&root, &root.join("link").join("keep.txt")).is_err());
        // Deleting the link itself removes only the link.
        safe_remove(&root, &root.join("link")).unwrap();
        assert!(outside.join("keep.txt").exists());
        assert!(fs::symlink_metadata(root.join("link")).is_err());
    }

    #[test]
    fn unique_and_sanitized_names() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("out.mp4");
        assert_eq!(unique_path(&p), p);
        fs::write(&p, "x").unwrap();
        assert_eq!(unique_path(&p), tmp.path().join("out (2).mp4"));
        assert_eq!(sanitize_file_name("a/b:c?.mp4 "), "a_b_c_.mp4");
        assert_eq!(sanitize_file_name("..."), "render");
    }
}
