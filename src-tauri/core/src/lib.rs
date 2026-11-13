//! Core engine for Universal Remotion Render Studio.
//!
//! This crate has no UI dependencies so that every safety-critical piece
//! (ZIP extraction, project inspection, process control, cleanup boundaries)
//! can be tested without a desktop shell.

pub mod app;
pub mod diagnostics;
pub mod fsutil;
pub mod logs;
pub mod model;
pub mod package_manager;
pub mod paths;
pub mod process;
pub mod project;
pub mod remotion;
pub mod runtime;

pub use app::Core;
pub use diagnostics::Diagnostic;

pub type Result<T> = std::result::Result<T, Diagnostic>;

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
