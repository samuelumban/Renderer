//! Serializable data model shared with the UI (camelCase JSON).

use crate::diagnostics::Diagnostic;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StorageMode {
    #[default]
    FastReopen,
    SaveSpace,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EnvStatus {
    #[default]
    NotPrepared,
    Preparing,
    Ready,
    Failed,
    /// Workspace was deleted by Save Space mode; ZIP and metadata remain.
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub original_zip_path: String,
    /// App-owned copy of the ZIP (when "keep ZIP copy" is enabled).
    pub zip_copy_path: Option<String>,
    pub workspace_path: String,
    pub project_root: String,
    pub package_manager: String,
    pub lockfile_path: Option<String>,
    pub remotion_version: Option<String>,
    pub node_requirement: Option<String>,
    pub selected_node_runtime: Option<String>,
    pub node_version: Option<String>,
    pub storage_mode: StorageMode,
    pub env_status: EnvStatus,
    pub install_approved: bool,
    pub safe_install: bool,
    pub last_error: Option<Diagnostic>,
    pub report: InspectionReport,
    pub created_at: String,
    pub last_opened_at: Option<String>,
    pub last_render_at: Option<String>,
    /// Filled in when listing; not authoritative.
    pub storage_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PackageVersion {
    pub name: String,
    pub declared: Option<String>,
    pub locked: Option<String>,
    pub installed: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CompositionInfo {
    pub id: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub duration_in_frames: Option<u32>,
    /// "static-scan" (best effort) or "remotion-cli" (authoritative).
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FontReport {
    /// Font files bundled in the project (relative paths).
    pub files: Vec<String>,
    /// Families declared via @font-face / FontFace / loadFont.
    pub declared_families: Vec<String>,
    /// Families referenced in CSS or inline styles.
    pub referenced_families: Vec<String>,
    /// External font URLs (need network at render time).
    pub external: Vec<String>,
    /// Families loaded through @remotion/google-fonts.
    pub google_fonts: Vec<String>,
    /// Referenced but not found bundled. A warning, not a confirmed failure.
    pub possibly_missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Issue {
    /// "error" (blocking) | "warning" | "info"
    pub severity: String,
    pub code: String,
    pub message: String,
    /// true = verified fact, false = possible issue found by static scanning.
    pub confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct InspectionReport {
    pub root_rel: String,
    pub package_name: Option<String>,
    pub remotion_packages: Vec<PackageVersion>,
    pub react_version: Option<String>,
    pub node_requirement: Option<String>,
    pub node_requirement_source: Option<String>,
    pub package_manager: String,
    pub package_manager_version: Option<String>,
    pub yarn_berry: bool,
    /// Project-pinned Yarn release (`yarnPath`), relative to the root.
    pub yarn_path: Option<String>,
    pub lockfile: Option<String>,
    pub install_command: String,
    pub entry_point: Option<String>,
    pub compositions: Vec<CompositionInfo>,
    pub fonts: FontReport,
    pub issues: Vec<Issue>,
    pub inspected_at: String,
}

impl InspectionReport {
    pub fn has_blocking(&self) -> bool {
        self.issues.iter().any(|i| i.severity == "error")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum JobStatus {
    #[default]
    Waiting,
    Preparing,
    Rendering,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_active(self) -> bool {
        matches!(self, JobStatus::Preparing | JobStatus::Rendering)
    }
    pub fn is_finished(self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderRequest {
    pub project_id: String,
    pub composition_id: String,
    pub output_dir: String,
    pub file_name: String,
    /// h264 | h265 | vp8 | vp9 | prores | gif
    pub codec: String,
    pub scale: Option<f64>,
    pub concurrency: Option<u32>,
    pub frame_start: Option<u32>,
    pub frame_end: Option<u32>,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderJob {
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub composition_id: String,
    pub output_path: String,
    pub status: JobStatus,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub log_path: String,
    /// Only ever written by the app. Never read from project files.
    pub temporary_paths: Vec<String>,
    pub request: RenderRequest,
    pub frame: u32,
    pub total_frames: u32,
    pub phase: String,
    pub output_bytes: Option<u64>,
    pub error: Option<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub output_dir: String,
    pub default_storage_mode: StorageMode,
    pub keep_zip_copy: bool,
    pub cache_limit_bytes: u64,
    pub max_zip_bytes: u64,
    pub max_extracted_bytes: u64,
    pub max_zip_entries: u64,
    pub install_timeout_minutes: u64,
    pub max_concurrent_renders: u32,
    pub default_node_major: u32,
    pub node_mirror: String,
    /// Optional browser override passed to Remotion (`--browser-executable`).
    pub browser_executable: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            output_dir: crate::paths::default_output_dir().to_string_lossy().into_owned(),
            default_storage_mode: StorageMode::FastReopen,
            keep_zip_copy: true,
            cache_limit_bytes: 5 * 1024 * 1024 * 1024,
            max_zip_bytes: 1024 * 1024 * 1024,
            max_extracted_bytes: 4 * 1024 * 1024 * 1024,
            max_zip_entries: 200_000,
            install_timeout_minutes: 30,
            max_concurrent_renders: 1,
            default_node_major: 22,
            node_mirror: "https://nodejs.org/dist".into(),
            browser_executable: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    pub workspaces: u64,
    pub runtimes: u64,
    pub package_cache: u64,
    pub browser_cache: u64,
    pub temp: u64,
    pub logs: u64,
    pub free_bytes: Option<u64>,
    pub cache_limit_bytes: u64,
    pub base_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub project_id: String,
    pub port: u16,
    pub url: String,
    /// starting | running | stopped | failed
    pub status: String,
    pub error: Option<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LogFile {
    pub name: String,
    pub bytes: u64,
    pub modified: String,
}

/// Notifications pushed to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Event {
    Log { key: String, line: String },
    Projects,
    Jobs,
    Previews,
    Progress { job_id: String, frame: u32, total: u32, phase: String },
}
