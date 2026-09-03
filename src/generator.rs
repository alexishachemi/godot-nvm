use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::{
    model::{NewProjectSpec, Renderer},
    nix,
    release::ReleaseAsset,
};

pub fn validate_package_attr(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|segment| {
            let mut chars = segment.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '\''))
        })
}

pub fn render_flake(asset: &ReleaseAsset, hash: &str, extra_tools: &[String]) -> Result<String> {
    if !extra_tools.iter().all(|tool| validate_package_attr(tool)) {
        bail!("extra tools must be dot-separated nixpkgs attribute paths");
    }
    let system = nix::current_system()?;
    let binary = asset
        .filename
        .strip_suffix(".zip")
        .context("Godot asset is not a zip archive")?;
    let tools = if extra_tools.is_empty() {
        String::new()
    } else {
        format!(
            "\n                {}",
            extra_tools.join("\n                ")
        )
    };
    Ok(format!(
        r#"{{
  description = "Godot {version} development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {{ self, nixpkgs }}:
    let
      system = "{system}";
      pkgs = import nixpkgs {{ inherit system; }};

      godot-unwrapped = pkgs.stdenv.mkDerivation {{
        pname = "godot-unwrapped";
        version = "{version}";
        src = pkgs.fetchurl {{
          url = "{url}";
          hash = "{hash}";
        }};
        nativeBuildInputs = [ pkgs.unzip ];
        unpackPhase = ''
          mkdir source
          unzip "$src" -d source
        '';
        installPhase = ''
          mkdir -p "$out/bin"
          cp "source/{binary}" "$out/bin/godot"
          chmod +x "$out/bin/godot"
        '';
        dontStrip = true;
      }};

      godot = pkgs.buildFHSEnv {{
        name = "godot";
        targetPkgs = pkgs: with pkgs; [
          godot-unwrapped
          alsa-lib libpulseaudio
          dbus fontconfig libxkbcommon mesa udev
          libGL vulkan-loader wayland
          xorg.libX11 xorg.libXcursor xorg.libXext xorg.libXfixes
          xorg.libXi xorg.libXinerama xorg.libXrandr xorg.libXrender
          speechd
        ];
        runScript = "godot";
      }};

      extraTools = with pkgs; [{tools}
      ];
    in {{
      packages.${{system}}.default = godot;
      devShells.${{system}}.default = pkgs.mkShell {{
        packages = [ godot ] ++ extraTools;
      }};
    }};
}}
"#,
        version = asset.version,
        system = system,
        url = asset.url,
        hash = hash,
        binary = binary,
        tools = tools,
    ))
}

pub fn create_project(spec: &NewProjectSpec, asset: &ReleaseAsset, hash: &str) -> Result<PathBuf> {
    validate_project_name(&spec.name)?;
    fs::create_dir_all(&spec.parent)
        .with_context(|| format!("could not create {}", spec.parent.display()))?;
    let destination = spec.parent.join(spec.name.trim());
    if destination.exists() {
        bail!("{} already exists", destination.display());
    }
    let staging = tempfile::Builder::new()
        .prefix(".godot-nvm-create-")
        .tempdir_in(&spec.parent)?;
    write_project_files(staging.path(), spec, asset, hash)?;
    nix::lock_flake(staging.path())?;
    nix::validate_dev_shell(staging.path(), "default")?;
    let path = staging.keep();
    fs::rename(&path, &destination)
        .with_context(|| format!("could not commit {}", destination.display()))?;
    Ok(destination)
}

pub fn add_missing_flake(
    root: &Path,
    asset: &ReleaseAsset,
    hash: &str,
    extra_tools: &[String],
    envrc: bool,
) -> Result<()> {
    let flake = root.join("flake.nix");
    if flake.exists() {
        bail!(
            "{} already exists and will not be overwritten",
            flake.display()
        );
    }
    if root.join("flake.lock").exists() {
        bail!(
            "{} exists without a flake.nix and will not be overwritten",
            root.join("flake.lock").display()
        );
    }
    let staging = tempfile::Builder::new()
        .prefix(".godot-nvm-flake-")
        .tempdir_in(root)?;
    fs::write(
        staging.path().join("flake.nix"),
        render_flake(asset, hash, extra_tools)?,
    )?;
    if envrc {
        fs::write(staging.path().join(".envrc"), "use flake\n")?;
    }
    nix::lock_flake(staging.path())?;
    nix::validate_dev_shell(staging.path(), "default")?;

    let envrc_target = root.join(".envrc");
    fs::rename(staging.path().join("flake.nix"), &flake)?;
    if let Err(error) = fs::rename(staging.path().join("flake.lock"), root.join("flake.lock")) {
        let _ = fs::remove_file(&flake);
        return Err(error).context("could not install flake.lock");
    }
    if envrc && !envrc_target.exists() {
        fs::rename(staging.path().join(".envrc"), envrc_target)?;
    }
    Ok(())
}

pub fn replace_existing_flake(
    root: &Path,
    asset: &ReleaseAsset,
    hash: &str,
    extra_tools: &[String],
    envrc: bool,
) -> Result<PathBuf> {
    let flake = root.join("flake.nix");
    if !flake.exists() {
        bail!(
            "{} does not exist; use the missing-flake workflow",
            flake.display()
        );
    }

    let staging = tempfile::Builder::new()
        .prefix(".godot-nvm-replacement-")
        .tempdir_in(root)?;
    fs::write(
        staging.path().join("flake.nix"),
        render_flake(asset, hash, extra_tools)?,
    )?;
    if envrc && !root.join(".envrc").exists() {
        fs::write(staging.path().join(".envrc"), "use flake\n")?;
    }
    nix::lock_flake(staging.path())?;
    nix::validate_dev_shell(staging.path(), "default")?;

    commit_replacement(root, staging.path(), envrc)
}

fn commit_replacement(root: &Path, staged: &Path, install_envrc: bool) -> Result<PathBuf> {
    let flake = root.join("flake.nix");
    let lock = root.join("flake.lock");
    let envrc = root.join(".envrc");
    let should_install_envrc = install_envrc && !envrc.exists() && staged.join(".envrc").exists();
    let backup = next_backup_path(root);
    fs::create_dir(&backup)
        .with_context(|| format!("could not create backup {}", backup.display()))?;

    if let Err(error) = fs::rename(&flake, backup.join("flake.nix")) {
        let _ = fs::remove_dir(&backup);
        return Err(error).context("could not back up the existing flake.nix");
    }
    let had_lock = lock.exists();
    if had_lock && let Err(error) = fs::rename(&lock, backup.join("flake.lock")) {
        let _ = fs::rename(backup.join("flake.nix"), &flake);
        let _ = fs::remove_dir(&backup);
        return Err(error).context("could not back up the existing flake.lock");
    }

    let install_result = (|| -> Result<()> {
        fs::rename(staged.join("flake.nix"), &flake)
            .context("could not install replacement flake.nix")?;
        fs::rename(staged.join("flake.lock"), &lock)
            .context("could not install replacement flake.lock")?;
        if should_install_envrc {
            fs::rename(staged.join(".envrc"), &envrc).context("could not install .envrc")?;
        }
        Ok(())
    })();

    if let Err(error) = install_result {
        let _ = fs::remove_file(&flake);
        let _ = fs::remove_file(&lock);
        if should_install_envrc {
            let _ = fs::remove_file(&envrc);
        }
        let _ = fs::rename(backup.join("flake.nix"), &flake);
        if had_lock {
            let _ = fs::rename(backup.join("flake.lock"), &lock);
        }
        let _ = fs::remove_dir(&backup);
        return Err(error).context("replacement was rolled back");
    }

    Ok(backup)
}

fn next_backup_path(root: &Path) -> PathBuf {
    let stem = format!(".godot-nvm-backup-{}", Utc::now().format("%Y%m%dT%H%M%SZ"));
    for suffix in 0.. {
        let name = if suffix == 0 {
            stem.clone()
        } else {
            format!("{stem}-{suffix}")
        };
        let candidate = root.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn write_project_files(
    root: &Path,
    spec: &NewProjectSpec,
    asset: &ReleaseAsset,
    hash: &str,
) -> Result<()> {
    fs::write(root.join("project.godot"), render_project_godot(spec))?;
    fs::write(root.join("icon.svg"), DEFAULT_ICON)?;
    fs::write(
        root.join("flake.nix"),
        render_flake(asset, hash, &spec.extra_tools)?,
    )?;
    if spec.create_envrc {
        fs::write(root.join(".envrc"), "use flake\n")?;
    }
    if spec.git_metadata {
        let ignored = if spec.godot_version.starts_with('3') {
            ".import/\n"
        } else {
            ".godot/\n"
        };
        fs::write(
            root.join(".gitignore"),
            format!("# Godot generated files\n{ignored}"),
        )?;
    }
    Ok(())
}

pub fn render_project_godot(spec: &NewProjectSpec) -> String {
    let escaped_name =
        serde_json::to_string(spec.name.trim()).unwrap_or_else(|_| "\"Godot project\"".into());
    let branch = spec
        .godot_version
        .split('-')
        .next()
        .unwrap_or(&spec.godot_version);
    let mut pieces = branch.split('.');
    let feature = format!(
        "{}.{}",
        pieces.next().unwrap_or("4"),
        pieces.next().unwrap_or("0")
    );
    if spec.godot_version.starts_with('3') {
        let driver = if spec.renderer == Renderer::Gles2 {
            "GLES2"
        } else {
            "GLES3"
        };
        format!(
            r#"; Engine configuration file.
; Generated by godot-nvm.
config_version=4

[application]
config/name={escaped_name}
config/icon="res://icon.svg"

[rendering]
quality/driver/driver_name="{driver}"
"#
        )
    } else {
        let (method, label) = match spec.renderer {
            Renderer::Mobile => ("mobile", "Mobile"),
            Renderer::Compatibility => ("gl_compatibility", "GL Compatibility"),
            _ => ("gl_compatibility", ""),
        };
        let method = if spec.renderer == Renderer::ForwardPlus {
            "gl_compatibility"
        } else {
            method
        };
        let rendering = if spec.renderer == Renderer::ForwardPlus {
            String::new()
        } else {
            format!(
                "\n[rendering]\nrenderer/rendering_method=\"{method}\"\nrenderer/rendering_method.mobile=\"{method}\"\n"
            )
        };
        let features = if label.is_empty() {
            format!("PackedStringArray(\"{feature}\")")
        } else {
            format!("PackedStringArray(\"{feature}\", \"{label}\")")
        };
        format!(
            r#"; Engine configuration file.
; Generated by godot-nvm.
config_version=5

[application]
config/name={escaped_name}
config/features={features}
config/icon="res://icon.svg"
{rendering}"#
        )
    }
}

fn validate_project_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || matches!(name, "." | "..") || name.contains('/') || name.contains('\0') {
        bail!("project name must be a non-empty directory name without slashes");
    }
    Ok(())
}

const DEFAULT_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">
<rect x="8" y="8" width="112" height="112" rx="24" fill="#478cbf"/>
<path d="M35 78V50l29-17 29 17v28L64 95z" fill="#fff" opacity=".95"/>
<circle cx="51" cy="62" r="5" fill="#478cbf"/><circle cx="77" cy="62" r="5" fill="#478cbf"/>
<path d="M47 76q17 13 34 0" fill="none" stroke="#478cbf" stroke-width="6" stroke-linecap="round"/>
</svg>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_attrs_are_data_not_nix_expressions() {
        assert!(validate_package_attr("python3Packages.pygame"));
        assert!(validate_package_attr("pkg-config"));
        assert!(!validate_package_attr("git ]; builtins.abort \"oops\""));
        assert!(!validate_package_attr("a..b"));
    }

    #[test]
    fn flake_contains_pinned_asset_data() {
        let asset = ReleaseAsset {
            version: "4.7.1-stable".into(),
            url: "https://example.invalid/godot.zip".into(),
            filename: "Godot_v4.7.1-stable_linux.x86_64.zip".into(),
        };
        let flake = render_flake(&asset, "sha256-example", &["git".into()]).unwrap();
        assert!(flake.contains("Godot_v4.7.1-stable_linux.x86_64"));
        assert!(flake.contains("sha256-example"));
        assert!(flake.contains("git"));
    }

    #[test]
    fn replacement_keeps_recoverable_backups() {
        let root = tempfile::tempdir().unwrap();
        let staged = tempfile::tempdir_in(root.path()).unwrap();
        fs::write(root.path().join("flake.nix"), "old flake").unwrap();
        fs::write(root.path().join("flake.lock"), "old lock").unwrap();
        fs::write(staged.path().join("flake.nix"), "new flake").unwrap();
        fs::write(staged.path().join("flake.lock"), "new lock").unwrap();
        fs::write(staged.path().join(".envrc"), "use flake\n").unwrap();

        let backup = commit_replacement(root.path(), staged.path(), true).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("flake.nix")).unwrap(),
            "new flake"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("flake.lock")).unwrap(),
            "new lock"
        );
        assert_eq!(
            fs::read_to_string(root.path().join(".envrc")).unwrap(),
            "use flake\n"
        );
        assert_eq!(
            fs::read_to_string(backup.join("flake.nix")).unwrap(),
            "old flake"
        );
        assert_eq!(
            fs::read_to_string(backup.join("flake.lock")).unwrap(),
            "old lock"
        );
    }
}
