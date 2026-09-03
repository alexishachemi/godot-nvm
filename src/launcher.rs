use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use anyhow::{Context, Result};
use chrono::Utc;
use uuid::Uuid;

use crate::nix;

pub struct Launch {
    pub child: Child,
    pub log_path: PathBuf,
}

pub fn launch(project_id: Uuid, path: &Path, shell: &str, log_root: &Path) -> Result<Launch> {
    let dir = log_root.join(project_id.to_string());
    fs::create_dir_all(&dir)?;
    rotate(&dir, 5)?;
    let log_path = dir.join(format!("{}.log", Utc::now().format("%Y%m%dT%H%M%SZ")));
    let stdout = File::create(&log_path)?;
    let stderr = stdout.try_clone()?;
    let child = Command::new("setsid")
        .arg("nix")
        .args(["develop", "--no-write-lock-file"])
        .arg(nix::flake_ref(path, Some(shell)))
        .args(["--command", "godot", "--editor", "--path"])
        .arg(path)
        .current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("could not start the detached Godot process")?;
    Ok(Launch { child, log_path })
}

fn rotate(dir: &Path, keep: usize) -> Result<()> {
    let mut files = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
        .collect::<Vec<_>>();
    files.sort_by_key(|entry| entry.file_name());
    let remove = files.len().saturating_sub(keep.saturating_sub(1));
    for entry in files.into_iter().take(remove) {
        fs::remove_file(entry.path())?;
    }
    Ok(())
}
