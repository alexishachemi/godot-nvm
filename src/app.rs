use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Child,
    sync::mpsc::{self, Receiver, Sender},
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use ratatui::layout::Size;
use ratatui_image::{
    Resize,
    picker::{Picker, ProtocolType},
    protocol::Protocol,
};
use uuid::Uuid;

use crate::{
    config, generator, icon, launcher,
    model::{AppState, NewProjectSpec, ProjectMetadata, ProjectRecord, Renderer},
    nix,
    paths::AppPaths,
    project, release,
};

pub const EXIT_CLOSE_SHELL: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathPurpose {
    Add,
    Scan,
}

#[derive(Debug, Clone)]
pub struct ScanCandidate {
    pub path: PathBuf,
    pub metadata: ProjectMetadata,
    pub has_flake: bool,
    pub selected: bool,
    pub registered: bool,
}

#[derive(Debug, Clone)]
pub struct ExistingForm {
    pub path: PathBuf,
    pub version: String,
    pub tools: String,
    pub field: usize,
    pub replace_existing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationPurpose {
    Import,
    Revalidate,
}

#[derive(Debug, Clone)]
pub struct CreateForm {
    pub name: String,
    pub parent: String,
    pub version: String,
    pub renderer_index: usize,
    pub git_metadata: bool,
    pub tools: String,
    pub field: usize,
}

impl CreateForm {
    pub fn renderer(&self) -> Renderer {
        let choices = Renderer::choices_for(&self.version);
        choices[self.renderer_index.min(choices.len().saturating_sub(1))]
    }
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    GenerateExisting {
        path: PathBuf,
        version: String,
        tools: Vec<String>,
        replace_existing: bool,
    },
    Create(NewProjectSpec),
    Unregister {
        id: Uuid,
    },
}

#[derive(Debug, Clone)]
pub enum Screen {
    Dashboard,
    AddMenu {
        selected: usize,
    },
    PathInput {
        purpose: PathPurpose,
        input: String,
    },
    ScanResults {
        root: PathBuf,
        candidates: Vec<ScanCandidate>,
        cursor: usize,
    },
    ShellChoice {
        path: PathBuf,
        shells: Vec<String>,
        selected: usize,
    },
    InvalidFlake {
        path: PathBuf,
        diagnostic: String,
        selected: usize,
    },
    ExistingForm(ExistingForm),
    CreateForm(CreateForm),
    Settings {
        directory: String,
        field: usize,
        create_envrc: bool,
    },
    Confirm {
        action: ConfirmAction,
        summary: String,
    },
    Help,
    Busy {
        message: String,
    },
}

enum WorkerResult {
    Releases(Result<Vec<String>>),
    Shells {
        path: PathBuf,
        result: Result<Vec<String>>,
    },
    Validated {
        path: PathBuf,
        shell: String,
        purpose: ValidationPurpose,
        result: Result<nix::Validation>,
    },
    GeneratedExisting {
        path: PathBuf,
        result: Result<(nix::Validation, Option<PathBuf>)>,
    },
    Created {
        result: Result<(PathBuf, nix::Validation)>,
    },
}

pub struct App {
    pub paths: AppPaths,
    pub state: AppState,
    pub screen: Screen,
    pub selected: usize,
    pub filter: String,
    pub filtering: bool,
    pub status: String,
    pub should_quit: bool,
    pub exit_code: i32,
    pub releases: Vec<String>,
    pub release_error: Option<String>,
    pending_imports: VecDeque<PathBuf>,
    worker_tx: Sender<WorkerResult>,
    worker_rx: Receiver<WorkerResult>,
    pub launches: Vec<(Uuid, Child)>,
    picker: Option<Picker>,
    pub images: HashMap<Uuid, Protocol>,
}

impl App {
    pub fn load(paths: AppPaths) -> Result<Self> {
        let state = config::load(&paths.config_file)?;
        let (worker_tx, worker_rx) = mpsc::channel();
        let app = Self {
            paths,
            state,
            screen: Screen::Dashboard,
            selected: 0,
            filter: String::new(),
            filtering: false,
            status: "Ready".into(),
            should_quit: false,
            exit_code: 0,
            releases: Vec::new(),
            release_error: None,
            pending_imports: VecDeque::new(),
            worker_tx,
            worker_rx,
            launches: Vec::new(),
            picker: None,
            images: HashMap::new(),
        };
        app.refresh_releases(false);
        Ok(app)
    }

    pub fn set_picker(&mut self, picker: Option<Picker>) {
        self.picker = picker.filter(|picker| picker.protocol_type() != ProtocolType::Halfblocks);
        self.rebuild_images();
    }

    pub fn image_protocol_name(&self) -> &'static str {
        match self.picker.as_ref().map(Picker::protocol_type) {
            Some(ProtocolType::Kitty) => "Kitty",
            Some(ProtocolType::Sixel) => "Sixel",
            Some(ProtocolType::Iterm2) => "iTerm2",
            _ => "off",
        }
    }

    pub fn rebuild_images(&mut self) {
        self.images.clear();
        let Some(picker) = &self.picker else {
            return;
        };
        for record in &self.state.projects {
            let Ok(metadata) = project::inspect(&record.path) else {
                continue;
            };
            let Some(path) = metadata.icon else {
                continue;
            };
            let Ok(image) = icon::load(&path) else {
                continue;
            };
            if let Ok(protocol) = picker.new_protocol(image, Size::new(8, 3), Resize::Fit(None)) {
                self.images.insert(record.id, protocol);
            }
        }
    }

    pub fn visible_indices(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        let mut indices = self
            .state
            .projects
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                if needle.is_empty() {
                    return true;
                }
                let metadata = project::inspect(&record.path).unwrap_or_default();
                metadata.name.to_lowercase().contains(&needle)
                    || record
                        .path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&needle)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        indices.sort_by(|a, b| {
            let left = &self.state.projects[*a];
            let right = &self.state.projects[*b];
            right
                .last_opened_at
                .cmp(&left.last_opened_at)
                .then_with(|| {
                    let ln = project::inspect(&left.path).unwrap_or_default().name;
                    let rn = project::inspect(&right.path).unwrap_or_default().name;
                    ln.to_lowercase().cmp(&rn.to_lowercase())
                })
        });
        indices
    }

    pub fn save(&self) -> Result<()> {
        config::save(&self.paths.config_file, &self.state)
    }

    pub fn refresh_releases(&self, force: bool) {
        let tx = self.worker_tx.clone();
        let cache = self.paths.cache_dir.clone();
        std::thread::spawn(move || {
            let _ = tx.send(WorkerResult::Releases(release::stable_versions(
                &cache, force,
            )));
        });
    }

    pub fn poll_workers(&mut self) {
        while let Ok(message) = self.worker_rx.try_recv() {
            match message {
                WorkerResult::Releases(result) => match result {
                    Ok(versions) => {
                        self.releases = versions;
                        self.release_error = None;
                        self.fill_release_defaults();
                    }
                    Err(error) => self.release_error = Some(format!("{error:#}")),
                },
                WorkerResult::Shells { path, result } => match result {
                    Ok(shells) if !shells.is_empty() => {
                        self.screen = Screen::ShellChoice {
                            path,
                            shells,
                            selected: 0,
                        };
                    }
                    Ok(_) => self.prompt_invalid_flake(
                        path,
                        "flake exposes no dev shell for this system".into(),
                    ),
                    Err(error) => self.prompt_invalid_flake(path, format!("{error:#}")),
                },
                WorkerResult::Validated {
                    path,
                    shell,
                    purpose,
                    result,
                } => match result {
                    Ok(validation) => self.register_valid(path, shell, validation),
                    Err(error) if purpose == ValidationPurpose::Import => {
                        self.prompt_invalid_flake(path, format!("{error:#}"))
                    }
                    Err(error) => self.register_broken(path, shell, format!("{error:#}")),
                },
                WorkerResult::GeneratedExisting { path, result } => match result {
                    Ok((validation, backup)) => {
                        self.register_valid(path, "default".into(), validation);
                        if let Some(backup) = backup {
                            self.status = format!(
                                "Registered project; previous flake backed up to {}",
                                backup.display()
                            );
                        }
                    }
                    Err(error) => {
                        self.status =
                            format!("Could not generate flake for {}: {error:#}", path.display());
                        self.continue_imports();
                    }
                },
                WorkerResult::Created { result } => match result {
                    Ok((path, validation)) => {
                        self.register_valid(path, "default".into(), validation);
                        self.status = "Project created and registered".into();
                    }
                    Err(error) => {
                        self.status = format!("Project creation failed: {error:#}");
                        self.screen = Screen::Dashboard;
                    }
                },
            }
        }
        self.poll_launches();
    }

    fn poll_launches(&mut self) {
        let mut failures = Vec::new();
        self.launches
            .retain_mut(|(id, child)| match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        failures.push((*id, status));
                    }
                    false
                }
                Ok(None) => true,
                Err(_) => false,
            });
        if let Some((_, status)) = failures.last() {
            self.status = format!("A Godot launch exited early with {status}; see its log");
        }
    }

    fn fill_release_defaults(&mut self) {
        match &mut self.screen {
            Screen::CreateForm(form) if form.version.is_empty() => {
                if let Some(version) = self.releases.first() {
                    form.version = version.clone();
                }
            }
            Screen::ExistingForm(form) if !form.version.ends_with("-stable") => {
                if let Some(version) = best_version(&self.releases, &form.version) {
                    form.version = version;
                }
            }
            _ => {}
        }
    }

    pub fn open_add_menu(&mut self) {
        self.screen = Screen::AddMenu { selected: 0 };
    }

    pub fn open_new_form(&mut self) {
        let parent = self
            .state
            .settings
            .default_projects_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        self.screen = Screen::CreateForm(CreateForm {
            name: String::new(),
            parent: parent.display().to_string(),
            version: self.releases.first().cloned().unwrap_or_default(),
            renderer_index: 0,
            git_metadata: true,
            tools: String::new(),
            field: 0,
        });
    }

    pub fn open_settings(&mut self) {
        self.screen = Screen::Settings {
            directory: self
                .state
                .settings
                .default_projects_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            field: 0,
            create_envrc: self.state.settings.create_envrc,
        };
    }

    pub fn path_input(&mut self, purpose: PathPurpose) {
        let path = self
            .state
            .settings
            .default_projects_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        self.screen = Screen::PathInput {
            purpose,
            input: path.display().to_string(),
        };
    }

    pub fn scan(&mut self, root: PathBuf) -> Result<()> {
        let root = root
            .canonicalize()
            .with_context(|| format!("could not resolve {}", root.display()))?;
        let candidates = discover_projects(&root, &self.state.projects)?;
        self.screen = Screen::ScanResults {
            root,
            candidates,
            cursor: 0,
        };
        Ok(())
    }

    pub fn queue_imports(&mut self, paths: Vec<PathBuf>) {
        self.pending_imports.extend(paths);
        self.continue_imports();
    }

    pub fn cancel_workflow(&mut self) {
        self.pending_imports.clear();
        self.screen = Screen::Dashboard;
        self.status = "Cancelled".into();
    }

    fn continue_imports(&mut self) {
        let Some(path) = self.pending_imports.pop_front() else {
            self.screen = Screen::Dashboard;
            self.rebuild_images();
            return;
        };
        let path = match project::canonical_project_path(&path) {
            Ok(path) => path,
            Err(error) => {
                self.status = format!("Skipped {}: {error:#}", path.display());
                return self.continue_imports();
            }
        };
        if self.state.projects.iter().any(|record| record.path == path) {
            self.status = format!("{} is already registered", path.display());
            return self.continue_imports();
        }
        if path.join("flake.nix").is_file() {
            self.screen = Screen::Busy {
                message: format!("Inspecting Nix dev shells in {}…", path.display()),
            };
            let tx = self.worker_tx.clone();
            std::thread::spawn(move || {
                let result = nix::enumerate_dev_shells(&path);
                let _ = tx.send(WorkerResult::Shells { path, result });
            });
        } else {
            let hint = project::inspect(&path)
                .ok()
                .and_then(|metadata| metadata.version_hint)
                .unwrap_or_default();
            let version = best_version(&self.releases, &hint).unwrap_or(hint);
            self.screen = Screen::ExistingForm(ExistingForm {
                path,
                version,
                tools: String::new(),
                field: 0,
                replace_existing: false,
            });
        }
    }

    pub fn validate_shell(&mut self, path: PathBuf, shell: String) {
        self.start_shell_validation(path, shell, ValidationPurpose::Import);
    }

    fn start_shell_validation(&mut self, path: PathBuf, shell: String, purpose: ValidationPurpose) {
        self.screen = Screen::Busy {
            message: format!("Validating {shell} in {}…", path.display()),
        };
        let tx = self.worker_tx.clone();
        std::thread::spawn(move || {
            let result = nix::validate_dev_shell(&path, &shell);
            let _ = tx.send(WorkerResult::Validated {
                path,
                shell,
                purpose,
                result,
            });
        });
    }

    fn prompt_invalid_flake(&mut self, path: PathBuf, diagnostic: String) {
        self.screen = Screen::InvalidFlake {
            path,
            diagnostic,
            selected: 0,
        };
    }

    pub fn resolve_invalid_flake(&mut self, path: PathBuf, overwrite: bool) {
        if !overwrite {
            self.skip_import(path);
            return;
        }
        let hint = project::inspect(&path)
            .ok()
            .and_then(|metadata| metadata.version_hint)
            .unwrap_or_default();
        let version = best_version(&self.releases, &hint).unwrap_or(hint);
        self.screen = Screen::ExistingForm(ExistingForm {
            path,
            version,
            tools: String::new(),
            field: 0,
            replace_existing: true,
        });
    }

    pub fn skip_import(&mut self, path: PathBuf) {
        self.status = format!("Skipped {}", path.display());
        self.continue_imports();
    }

    fn register_valid(&mut self, path: PathBuf, shell: String, validation: nix::Validation) {
        let mut record = ProjectRecord::new(path.clone());
        record.dev_shell = shell;
        record.verified_version = Some(validation.version);
        self.upsert(record);
        self.status = format!("Registered {}", path.display());
        self.continue_imports();
    }

    fn register_broken(&mut self, path: PathBuf, shell: String, diagnostic: String) {
        let mut record = ProjectRecord::new(path.clone());
        record.dev_shell = shell;
        record.diagnostic = Some(diagnostic);
        self.upsert(record);
        self.status = format!("Registered {} as broken", path.display());
        self.continue_imports();
    }

    fn upsert(&mut self, record: ProjectRecord) {
        if let Some(existing) = self
            .state
            .projects
            .iter_mut()
            .find(|existing| existing.path == record.path)
        {
            let mut record = record;
            record.id = existing.id;
            record.last_opened_at = existing.last_opened_at;
            *existing = record;
        } else {
            self.state.projects.push(record);
        }
        if let Err(error) = self.save() {
            self.status = format!("Could not save registry: {error:#}");
        }
    }

    pub fn confirm_existing(&mut self, form: ExistingForm) -> Result<()> {
        validate_version(&form.version)?;
        let tools = parse_tools(&form.tools)?;
        let file_action = if form.replace_existing {
            "The existing flake.nix and flake.lock will be moved to a timestamped backup."
        } else {
            "No existing file will be overwritten."
        };
        let summary = format!(
            "{} a generated flake for {}\nGodot: {}\nExtra tools: {}\n.envrc: {}\n\n{}",
            if form.replace_existing {
                "Replace"
            } else {
                "Add"
            },
            form.path.display(),
            form.version,
            if tools.is_empty() {
                "none".into()
            } else {
                tools.join(", ")
            },
            if self.state.settings.create_envrc {
                "create when absent"
            } else {
                "disabled"
            },
            file_action,
        );
        self.screen = Screen::Confirm {
            action: ConfirmAction::GenerateExisting {
                path: form.path,
                version: form.version,
                tools,
                replace_existing: form.replace_existing,
            },
            summary,
        };
        Ok(())
    }

    pub fn confirm_create(&mut self, form: CreateForm) -> Result<()> {
        if form.name.trim().is_empty() {
            bail!("project name is required");
        }
        let parent = PathBuf::from(form.parent.trim())
            .canonicalize()
            .context("project parent directory does not exist")?;
        validate_version(&form.version)?;
        let tools = parse_tools(&form.tools)?;
        let renderer = form.renderer();
        let spec = NewProjectSpec {
            name: form.name.trim().into(),
            parent,
            godot_version: form.version,
            renderer,
            git_metadata: form.git_metadata,
            extra_tools: tools,
            create_envrc: self.state.settings.create_envrc,
        };
        let summary = format!(
            "Create {}\nLocation: {}\nGodot: {}\nRenderer: {}\nGit metadata: {}\n.envrc: {}",
            spec.name,
            spec.parent.display(),
            spec.godot_version,
            spec.renderer.label(),
            spec.git_metadata,
            spec.create_envrc
        );
        self.screen = Screen::Confirm {
            action: ConfirmAction::Create(spec),
            summary,
        };
        Ok(())
    }

    pub fn execute_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::GenerateExisting {
                path,
                version,
                tools,
                replace_existing,
            } => {
                self.screen = Screen::Busy {
                    message: format!("Downloading Godot {version} and validating generated flake…"),
                };
                let tx = self.worker_tx.clone();
                let envrc = self.state.settings.create_envrc;
                std::thread::spawn(move || {
                    let result = (|| {
                        let asset = release::resolve_asset(&version)?;
                        let hash = nix::prefetch_file(&asset.url)?;
                        let backup = if replace_existing {
                            Some(generator::replace_existing_flake(
                                &path, &asset, &hash, &tools, envrc,
                            )?)
                        } else {
                            generator::add_missing_flake(&path, &asset, &hash, &tools, envrc)?;
                            None
                        };
                        let validation = nix::validate_dev_shell(&path, "default")?;
                        Ok((validation, backup))
                    })();
                    let _ = tx.send(WorkerResult::GeneratedExisting { path, result });
                });
            }
            ConfirmAction::Create(spec) => {
                self.screen = Screen::Busy {
                    message: format!("Creating {} with Godot {}…", spec.name, spec.godot_version),
                };
                let tx = self.worker_tx.clone();
                std::thread::spawn(move || {
                    let result = (|| {
                        let asset = release::resolve_asset(&spec.godot_version)?;
                        let hash = nix::prefetch_file(&asset.url)?;
                        let path = generator::create_project(&spec, &asset, &hash)?;
                        let validation = nix::validate_dev_shell(&path, "default")?;
                        Ok((path, validation))
                    })();
                    let _ = tx.send(WorkerResult::Created { result });
                });
            }
            ConfirmAction::Unregister { id } => {
                if let Some(index) = self
                    .state
                    .projects
                    .iter()
                    .position(|record| record.id == id)
                {
                    let removed = self.state.projects.remove(index);
                    self.images.remove(&removed.id);
                    self.selected = self.selected.saturating_sub(1);
                    if let Err(error) = self.save() {
                        self.status = format!("Could not save registry: {error:#}");
                    } else {
                        self.status = format!(
                            "Unregistered {} (project files were not touched)",
                            removed.path.display()
                        );
                    }
                }
                self.screen = Screen::Dashboard;
            }
        }
    }

    pub fn launch_selected(&mut self, close_shell: bool) -> Result<()> {
        let indices = self.visible_indices();
        let index = *indices.get(self.selected).context("no project selected")?;
        let record = &mut self.state.projects[index];
        if !record.is_ready() {
            bail!("project is not ready; revalidate it first");
        }
        let launch = launcher::launch(
            record.id,
            &record.path,
            &record.dev_shell,
            &self.paths.log_dir,
        )?;
        self.status = format!(
            "Launching {} (log: {})",
            record.path.display(),
            launch.log_path.display()
        );
        record.last_opened_at = Some(Utc::now());
        let id = record.id;
        self.launches.push((id, launch.child));
        self.save()?;
        if close_shell {
            self.should_quit = true;
            self.exit_code = EXIT_CLOSE_SHELL;
        }
        Ok(())
    }

    pub fn revalidate_selected(&mut self) -> Result<()> {
        let indices = self.visible_indices();
        let index = *indices.get(self.selected).context("no project selected")?;
        let record = &self.state.projects[index];
        self.pending_imports.clear();
        let path = record.path.clone();
        let shell = record.dev_shell.clone();
        self.start_shell_validation(path, shell, ValidationPurpose::Revalidate);
        Ok(())
    }

    pub fn cancel_confirm(&mut self, action: &ConfirmAction) {
        match action {
            ConfirmAction::GenerateExisting { path, .. } => {
                self.status = format!("Skipped {}", path.display());
                self.continue_imports();
            }
            ConfirmAction::Create(_) | ConfirmAction::Unregister { .. } => {
                self.cancel_workflow();
            }
        }
    }

    pub fn confirm_unregister_selected(&mut self) -> Result<()> {
        let indices = self.visible_indices();
        let index = *indices.get(self.selected).context("no project selected")?;
        let record = &self.state.projects[index];
        self.screen = Screen::Confirm {
            action: ConfirmAction::Unregister { id: record.id },
            summary: format!(
                "Unregister {}?\n\nNo project files will be deleted.",
                record.path.display()
            ),
        };
        Ok(())
    }

    pub fn save_settings(&mut self, directory: String, create_envrc: bool) -> Result<()> {
        let path = if directory.trim().is_empty() {
            None
        } else {
            let path = PathBuf::from(directory.trim())
                .canonicalize()
                .context("default directory does not exist")?;
            if !path.is_dir() {
                bail!("default projects path is not a directory");
            }
            Some(path)
        };
        self.state.settings.default_projects_dir = path;
        self.state.settings.create_envrc = create_envrc;
        self.save()?;
        self.screen = Screen::Dashboard;
        self.status = "Settings saved".into();
        Ok(())
    }
}

fn discover_projects(
    root: &Path,
    registered_projects: &[ProjectRecord],
) -> Result<Vec<ScanCandidate>> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("could not scan {}", root.display()))? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        let Ok(metadata) = project::inspect(&path) else {
            continue;
        };
        let registered = registered_projects.iter().any(|record| record.path == path);
        candidates.push(ScanCandidate {
            has_flake: path.join("flake.nix").is_file(),
            path,
            metadata,
            selected: !registered,
            registered,
        });
    }
    candidates.sort_by_key(|item| item.metadata.name.to_lowercase());
    Ok(candidates)
}

fn best_version(releases: &[String], hint: &str) -> Option<String> {
    if hint.ends_with("-stable") && releases.iter().any(|release| release == hint) {
        return Some(hint.into());
    }
    releases
        .iter()
        .find(|release| {
            release
                .strip_suffix("-stable")
                .is_some_and(|v| v == hint || v.starts_with(&format!("{hint}.")))
        })
        .cloned()
}

fn validate_version(value: &str) -> Result<()> {
    if value.ends_with("-stable")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        Ok(())
    } else {
        Err(anyhow!(
            "enter an exact official stable tag, for example 4.7.1-stable"
        ))
    }
}

fn parse_tools(value: &str) -> Result<Vec<String>> {
    let tools = value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if tools
        .iter()
        .all(|tool| generator::validate_package_attr(tool))
    {
        Ok(tools)
    } else {
        bail!("extra tools must be nixpkgs attribute paths separated by spaces or commas")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_hint_chooses_newest_matching_stable() {
        let releases = vec![
            "4.7.2-stable".into(),
            "4.7.1-stable".into(),
            "4.6-stable".into(),
        ];
        assert_eq!(
            best_version(&releases, "4.7").as_deref(),
            Some("4.7.2-stable")
        );
    }

    #[test]
    fn scanner_includes_projects_with_and_without_flakes() {
        let root = tempfile::tempdir().unwrap();
        let with_flake = root.path().join("with-flake");
        let without_flake = root.path().join("without-flake");
        fs::create_dir(&with_flake).unwrap();
        fs::create_dir(&without_flake).unwrap();
        let project = "config_version=5\n[application]\nconfig/name=\"Game\"\n";
        fs::write(with_flake.join("project.godot"), project).unwrap();
        fs::write(with_flake.join("flake.nix"), "{}").unwrap();
        fs::write(without_flake.join("project.godot"), project).unwrap();

        let candidates = discover_projects(root.path(), &[]).unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|candidate| candidate.has_flake));
        assert!(candidates.iter().any(|candidate| !candidate.has_flake));
        assert!(candidates.iter().all(|candidate| candidate.selected));
    }
}
