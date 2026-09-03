use std::{
    path::Path,
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    pub version: String,
}

pub fn current_system() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64-linux"),
        "aarch64" => Ok("aarch64-linux"),
        other => bail!("unsupported Linux architecture: {other}"),
    }
}

pub fn flake_ref(path: &Path, shell: Option<&str>) -> String {
    let mut value = format!("path:{}", path.display());
    if let Some(shell) = shell {
        value.push('#');
        value.push_str(shell);
    }
    value
}

pub fn enumerate_dev_shells(path: &Path) -> Result<Vec<String>> {
    let output = Command::new("nix")
        .args(["flake", "show", "--json", "--no-write-lock-file"])
        .arg(flake_ref(path, None))
        .output()
        .context("could not execute nix flake show")?;
    require_success(output, "nix could not evaluate the flake").and_then(|bytes| {
        let root: Value = serde_json::from_slice(&bytes).context("nix returned malformed JSON")?;
        let mut names = root
            .get("devShells")
            .and_then(|v| v.get(current_system().ok()?))
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        names.sort_by_key(|name| (name != "default", name.clone()));
        Ok(names)
    })
}

pub fn validate_dev_shell(path: &Path, shell: &str) -> Result<Validation> {
    let output = Command::new("nix")
        .args(["develop", "--no-write-lock-file"])
        .arg(flake_ref(path, Some(shell)))
        .args([
            "--command",
            "sh",
            "-c",
            "command -v godot >/dev/null && printf '__GODOT_NVM_VERSION__' && godot --version",
        ])
        .output()
        .context("could not execute nix develop")?;
    let bytes = require_success(
        output,
        "the selected dev shell does not expose a working godot command",
    )?;
    let stdout = String::from_utf8_lossy(&bytes);
    let version = stdout
        .split("__GODOT_NVM_VERSION__")
        .nth(1)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .context("godot --version returned no version")?;
    Ok(Validation {
        version: version.to_string(),
    })
}

pub fn lock_flake(path: &Path) -> Result<()> {
    let output = Command::new("nix")
        .args(["flake", "lock"])
        .arg(flake_ref(path, None))
        .output()
        .context("could not execute nix flake lock")?;
    require_success(output, "nix could not lock the generated flake")?;
    Ok(())
}

pub fn prefetch_file(url: &str) -> Result<String> {
    let output = Command::new("nix")
        .args(["store", "prefetch-file", "--json", url])
        .output()
        .context("could not execute nix store prefetch-file")?;
    let bytes = require_success(output, "nix could not download the selected Godot build")?;
    let value: Value =
        serde_json::from_slice(&bytes).context("nix prefetch returned malformed JSON")?;
    value
        .get("hash")
        .and_then(Value::as_str)
        .map(str::to_string)
        .context("nix prefetch did not return a hash")
}

fn require_success(output: Output, context: &str) -> Result<Vec<u8>> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(context);
    bail!("{context}: {}", truncate(message, 300));
}

fn truncate(value: &str, max: usize) -> &str {
    value.get(..max).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_path_flake_refs_without_shell_parsing() {
        let path = Path::new("/tmp/a project; still data");
        assert_eq!(
            flake_ref(path, Some("tools")),
            "path:/tmp/a project; still data#tools"
        );
    }
}
