use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result, bail};

use crate::model::{AppState, STATE_VERSION};

pub fn load(path: &Path) -> Result<AppState> {
    if !path.exists() {
        return Ok(AppState::default());
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    let state: AppState = toml::from_str(&text)
        .with_context(|| format!("{} is not valid godot-nvm state", path.display()))?;
    if state.schema_version > STATE_VERSION {
        bail!(
            "{} uses schema version {}, but this build supports up to {}",
            path.display(),
            state.schema_version,
            STATE_VERSION
        );
    }
    Ok(state)
}

pub fn save(path: &Path, state: &AppState) -> Result<()> {
    let parent = path.parent().context("state path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let text = toml::to_string_pretty(state).context("could not serialize application state")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create a temporary file in {}", parent.display()))?;
    tmp.write_all(text.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not atomically replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let state = AppState::default();
        save(&path, &state).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.schema_version, STATE_VERSION);
        assert!(loaded.settings.create_envrc);
    }

    #[test]
    fn rejects_future_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(&path, "schema_version = 999\n").unwrap();
        assert!(
            load(&path)
                .unwrap_err()
                .to_string()
                .contains("schema version")
        );
    }
}
