use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use anyhow::{Context, Result, bail};

use crate::model::ProjectMetadata;

static UID_RESOURCE_CACHE: LazyLock<Mutex<HashMap<(PathBuf, String), PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn inspect(root: &Path) -> Result<ProjectMetadata> {
    let file = root.join("project.godot");
    if !file.is_file() {
        bail!("{} does not contain a project.godot file", root.display());
    }
    let text = fs::read_to_string(&file)
        .with_context(|| format!("{} is not readable UTF-8", file.display()))?;
    let settings = parse_document(&text)?;

    let fallback = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Unnamed project");
    let name = find_setting(&settings, "application", "config/name")
        .and_then(parse_string)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string());
    let icon = find_setting(&settings, "application", "config/icon")
        .and_then(parse_string)
        .and_then(|s| resolve_resource(root, &s));
    let version_hint =
        find_setting(&settings, "application", "config/features").and_then(extract_version_hint);

    Ok(ProjectMetadata {
        name,
        icon,
        version_hint,
    })
}

fn parse_document(text: &str) -> Result<HashMap<(String, String), String>> {
    let mut section = String::new();
    let mut settings = HashMap::new();
    let mut delimiters = Vec::new();
    let mut config_version = None;

    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        if delimiters.is_empty() && line.starts_with('[') {
            if !line.ends_with(']') || line.len() < 3 {
                bail!("malformed section header on line {}", index + 1);
            }
            section = line[1..line.len() - 1].trim().to_string();
            if section.is_empty() {
                bail!("empty section header on line {}", index + 1);
            }
            continue;
        }

        if delimiters.is_empty() {
            let (key, value) = line
                .split_once('=')
                .with_context(|| format!("malformed setting on line {}", index + 1))?;
            let key = key.trim();
            if key.is_empty() {
                bail!("empty setting name on line {}", index + 1);
            }
            let value = value.trim();
            if section.is_empty() && key == "config_version" {
                config_version = value.parse::<u32>().ok().filter(|version| *version > 0);
            }
            settings.insert((section.clone(), key.to_string()), value.to_string());
            update_delimiters(value, index + 1, &mut delimiters)?;
        } else {
            update_delimiters(line, index + 1, &mut delimiters)?;
        }
    }

    if !delimiters.is_empty() {
        bail!("unterminated multiline value at end of project.godot");
    }
    if config_version.is_none() {
        bail!("project.godot has no valid top-level config_version");
    }
    Ok(settings)
}

fn update_delimiters(line: &str, line_number: usize, stack: &mut Vec<char>) -> Result<()> {
    let mut in_string = false;
    let mut escaped = false;
    for character in line.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '(' | '[' | '{' => stack.push(character),
            ')' | ']' | '}' => {
                let expected = match character {
                    ')' => '(',
                    ']' => '[',
                    '}' => '{',
                    _ => unreachable!(),
                };
                if stack.pop() != Some(expected) {
                    bail!("mismatched delimiter on line {line_number}");
                }
            }
            ';' | '#' => break,
            _ => {}
        }
    }
    if in_string {
        bail!("unterminated string on line {line_number}");
    }
    Ok(())
}

fn find_setting<'a>(
    settings: &'a HashMap<(String, String), String>,
    wanted_section: &str,
    wanted_key: &str,
) -> Option<&'a str> {
    settings
        .get(&(wanted_section.to_string(), wanted_key.to_string()))
        .map(String::as_str)
}

fn parse_string(value: &str) -> Option<String> {
    let value = value.trim();
    if !(value.starts_with('"') && value.ends_with('"')) {
        return None;
    }
    serde_json::from_str(value).ok()
}

fn extract_version_hint(value: &str) -> Option<String> {
    let start = value.find('"')? + 1;
    let rest = &value[start..];
    let end = rest.find('"')?;
    let value = &rest[..end];
    let mut pieces = value.split('.');
    let major = pieces.next()?;
    let minor = pieces.next()?;
    if major.chars().all(|c| c.is_ascii_digit()) && minor.chars().all(|c| c.is_ascii_digit()) {
        Some(format!("{major}.{minor}"))
    } else {
        None
    }
}

fn resolve_resource(root: &Path, value: &str) -> Option<PathBuf> {
    if value.starts_with("uid://") {
        return resolve_uid_resource(root, value);
    }

    resolve_res_path(root, value)
}

fn resolve_res_path(root: &Path, value: &str) -> Option<PathBuf> {
    let relative = value.strip_prefix("res://")?;
    let relative = Path::new(relative);
    if relative.components().any(|part| {
        matches!(
            part,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    let path = root.join(relative);
    path.is_file().then_some(path)
}

fn resolve_uid_resource(root: &Path, wanted_uid: &str) -> Option<PathBuf> {
    let cache_key = (root.to_path_buf(), wanted_uid.to_string());
    if let Some(path) = UID_RESOURCE_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.get(&cache_key).cloned())
        .filter(|path| path.is_file())
    {
        return Some(path);
    }

    // Godot 4 may serialize resource settings as uid:// values. Imported source
    // files retain the UID and original res:// path in their adjacent .import
    // metadata, which lets us resolve the icon without launching Godot.
    let path = find_uid_import(root, root, wanted_uid, 0)?;
    if let Ok(mut cache) = UID_RESOURCE_CACHE.lock() {
        cache.insert(cache_key, path.clone());
    }
    Some(path)
}

fn find_uid_import(
    root: &Path,
    directory: &Path,
    wanted_uid: &str,
    depth: usize,
) -> Option<PathBuf> {
    const MAX_DEPTH: usize = 64;
    if depth > MAX_DEPTH {
        return None;
    }

    let mut directories = Vec::new();
    let entries = fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name();
            if !matches!(
                name.to_str(),
                Some(".git" | ".godot" | ".direnv" | "target")
            ) {
                directories.push(path);
            }
            continue;
        }
        if !file_type.is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("import")
        {
            continue;
        }
        if let Some(source) = source_for_uid(root, &path, wanted_uid) {
            return Some(source);
        }
    }

    for child in directories {
        if let Some(source) = find_uid_import(root, &child, wanted_uid, depth + 1) {
            return Some(source);
        }
    }
    None
}

fn source_for_uid(root: &Path, import_file: &Path, wanted_uid: &str) -> Option<PathBuf> {
    let text = fs::read_to_string(import_file).ok()?;
    let mut uid = None;
    let mut source_file = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(value) = line.strip_prefix("uid=") {
            uid = parse_string(value);
        } else if let Some(value) = line.strip_prefix("source_file=") {
            source_file = parse_string(value);
        }
    }
    (uid.as_deref() == Some(wanted_uid))
        .then(|| {
            source_file
                .as_deref()
                .and_then(|path| resolve_res_path(root, path))
        })
        .flatten()
}

pub fn canonical_project_path(path: &Path) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("could not resolve {}", path.display()))?;
    inspect(&canonical)?;
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_minimal_project_and_uses_directory_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("project.godot"), "config_version=5\n").unwrap();
        assert_eq!(
            inspect(dir.path()).unwrap().name,
            dir.path().file_name().unwrap().to_str().unwrap()
        );
    }

    #[test]
    fn extracts_metadata() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("icon.png"), []).unwrap();
        fs::write(
            dir.path().join("project.godot"),
            r#"
config_version=5

[application]
config/name="A \"quoted\" game"
config/features=PackedStringArray("4.7", "GL Compatibility")
config/icon="res://icon.png"
"#,
        )
        .unwrap();
        let metadata = inspect(dir.path()).unwrap();
        assert_eq!(metadata.name, "A \"quoted\" game");
        assert_eq!(metadata.version_hint.as_deref(), Some("4.7"));
        assert_eq!(metadata.icon, Some(dir.path().join("icon.png")));
    }

    #[test]
    fn rejects_icon_traversal() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("project.godot"),
            "config_version=5\n[application]\nconfig/icon=\"res://../secret.png\"\n",
        )
        .unwrap();
        assert!(inspect(dir.path()).unwrap().icon.is_none());
    }

    #[test]
    fn resolves_uid_backed_png_icon_from_import_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().join("assets/branding");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("game-icon.png"), b"png contents").unwrap();
        fs::write(
            assets.join("game-icon.png.import"),
            r#"[remap]

importer="texture"
uid="uid://dummygameicon"

[deps]

source_file="res://assets/branding/game-icon.png"
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("project.godot"),
            r#"config_version=5

[application]
config/name="UID icon"
config/icon="uid://dummygameicon"
"#,
        )
        .unwrap();

        assert_eq!(
            inspect(dir.path()).unwrap().icon,
            Some(assets.join("game-icon.png"))
        );
    }

    #[test]
    fn does_not_resolve_a_different_import_uid() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("icon.png"), b"png contents").unwrap();
        fs::write(
            dir.path().join("icon.png.import"),
            "uid=\"uid://anothericon\"\nsource_file=\"res://icon.png\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("project.godot"),
            "config_version=5\n[application]\nconfig/icon=\"uid://wantedicon\"\n",
        )
        .unwrap();

        assert!(inspect(dir.path()).unwrap().icon.is_none());
    }

    #[test]
    fn accepts_multiline_godot_values() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("project.godot"),
            r#"config_version=5

[application]
config/name="Multiline"
config/features=PackedStringArray("4.7", "Forward Plus")

[file_customization]
folder_colors={
"res://addons/": "gray",
"res://assets/": "yellow"
}

[input]
move={
"deadzone": 0.5,
"events": [Object(InputEventKey,"physical_keycode":87)
, Object(InputEventKey,"physical_keycode":4194320)
]
}
"#,
        )
        .unwrap();

        let metadata = inspect(dir.path()).unwrap();
        assert_eq!(metadata.name, "Multiline");
        assert_eq!(metadata.version_hint.as_deref(), Some("4.7"));
    }

    #[test]
    fn rejects_unbalanced_multiline_values() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("project.godot"),
            "config_version=5\n[input]\nmove={\n\"deadzone\": 0.5\n",
        )
        .unwrap();
        assert!(inspect(dir.path()).is_err());
    }
}
