use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_file: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let dirs = ProjectDirs::from("dev", "godot-nvm", "godot-nvm")
            .context("could not determine XDG application directories")?;
        Ok(Self {
            config_file: dirs.config_dir().join("state.toml"),
            cache_dir: dirs.cache_dir().to_path_buf(),
            log_dir: dirs
                .state_dir()
                .unwrap_or_else(|| dirs.data_local_dir())
                .join("logs"),
        })
    }
}
