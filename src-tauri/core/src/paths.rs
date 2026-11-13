//! All app-managed locations, derived from one base directory.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub base: PathBuf,
}

impl Paths {
    /// `%LOCALAPPDATA%\UniversalRemotionStudio`, overridable with `URS_DATA_DIR`.
    pub fn from_env() -> Paths {
        let base = std::env::var_os("URS_DATA_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                dirs::data_local_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("UniversalRemotionStudio")
            });
        Paths { base }
    }

    pub fn new(base: impl Into<PathBuf>) -> Paths {
        Paths { base: base.into() }
    }

    pub fn runtimes(&self) -> PathBuf {
        self.base.join("runtimes")
    }
    /// App-managed pnpm / Yarn installs (never global).
    pub fn tools(&self) -> PathBuf {
        self.runtimes().join("tools")
    }
    pub fn workspaces(&self) -> PathBuf {
        self.base.join("workspaces")
    }
    pub fn workspace(&self, id: &str) -> PathBuf {
        self.workspaces().join(id)
    }
    pub fn source(&self, id: &str) -> PathBuf {
        self.workspace(id).join("source")
    }
    pub fn temp(&self, id: &str) -> PathBuf {
        self.workspace(id).join("temp")
    }
    pub fn zip_copy(&self, id: &str) -> PathBuf {
        self.workspace(id).join("original.zip")
    }
    pub fn package_cache(&self) -> PathBuf {
        self.base.join("cache").join("package-manager")
    }
    pub fn browser_cache(&self) -> PathBuf {
        self.base.join("cache").join("browser")
    }
    pub fn logs(&self) -> PathBuf {
        self.base.join("logs")
    }
    pub fn project_logs(&self, id: &str) -> PathBuf {
        self.logs().join(id)
    }
    /// Files the app itself needs at run time (never project data).
    pub fn app_dir(&self) -> PathBuf {
        self.base.join("app")
    }
    pub fn settings_dir(&self) -> PathBuf {
        self.base.join("settings")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [
            self.app_dir(),
            self.runtimes(),
            self.workspaces(),
            self.package_cache(),
            self.browser_cache(),
            self.logs(),
            self.settings_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

pub fn default_output_dir() -> PathBuf {
    dirs::video_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| Path::new(".").to_path_buf())
        .join("Remotion Renders")
}
