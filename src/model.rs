use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    pub schema_version: u32,
    pub settings: Settings,
    pub projects: Vec<ProjectRecord>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            schema_version: STATE_VERSION,
            settings: Settings::default(),
            projects: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub default_projects_dir: Option<PathBuf>,
    pub create_envrc: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_projects_dir: None,
            create_envrc: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub id: Uuid,
    pub path: PathBuf,
    #[serde(default = "default_shell")]
    pub dev_shell: String,
    #[serde(default)]
    pub verified_version: Option<String>,
    #[serde(default)]
    pub diagnostic: Option<String>,
    #[serde(default)]
    pub last_opened_at: Option<DateTime<Utc>>,
}

fn default_shell() -> String {
    "default".into()
}

impl ProjectRecord {
    pub fn new(path: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            path,
            dev_shell: default_shell(),
            verified_version: None,
            diagnostic: None,
            last_opened_at: None,
        }
    }

    pub fn is_ready(&self) -> bool {
        self.verified_version.is_some() && self.diagnostic.is_none() && self.path.exists()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectMetadata {
    pub name: String,
    pub icon: Option<PathBuf>,
    pub version_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    ForwardPlus,
    Mobile,
    Compatibility,
    Gles3,
    Gles2,
}

impl Renderer {
    pub fn choices_for(version: &str) -> &'static [Renderer] {
        if version.trim_start().starts_with('3') {
            &[Renderer::Gles3, Renderer::Gles2]
        } else {
            &[
                Renderer::ForwardPlus,
                Renderer::Mobile,
                Renderer::Compatibility,
            ]
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ForwardPlus => "Forward+",
            Self::Mobile => "Mobile",
            Self::Compatibility => "Compatibility",
            Self::Gles3 => "GLES3",
            Self::Gles2 => "GLES2",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewProjectSpec {
    pub name: String,
    pub parent: PathBuf,
    pub godot_version: String,
    pub renderer: Renderer,
    pub git_metadata: bool,
    pub extra_tools: Vec<String>,
    pub create_envrc: bool,
}
