use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use ratatui_image::Image;

use crate::{
    app::{App, ConfirmAction, CreateForm, ExistingForm, PathPurpose, Screen},
    model::Renderer,
    project,
};

const ACCENT: Color = Color::Rgb(71, 140, 191);
const CARD_HEIGHT: u16 = 5;

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if key.kind != crossterm::event::KeyEventKind::Press {
        return;
    }
    let screen = app.screen.clone();
    let result = match screen {
        Screen::Dashboard => handle_dashboard(app, key),
        Screen::AddMenu { selected } => handle_add_menu(app, key, selected),
        Screen::PathInput { purpose, input } => handle_path(app, key, purpose, input),
        Screen::ScanResults {
            root,
            candidates,
            cursor,
        } => handle_scan(app, key, root, candidates, cursor),
        Screen::ShellChoice {
            path,
            shells,
            selected,
        } => handle_shells(app, key, path, shells, selected),
        Screen::InvalidFlake {
            path,
            diagnostic,
            selected,
        } => handle_invalid_flake(app, key, path, diagnostic, selected),
        Screen::ExistingForm(form) => handle_existing_form(app, key, form),
        Screen::CreateForm(form) => handle_create_form(app, key, form),
        Screen::Settings {
            directory,
            field,
            create_envrc,
        } => handle_settings(app, key, directory, field, create_envrc),
        Screen::Confirm { action, .. } => handle_confirm(app, key, action),
        Screen::Help => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
            ) {
                app.screen = Screen::Dashboard;
            }
            Ok(())
        }
        Screen::Busy { .. } => Ok(()),
    };
    if let Err(error) = result {
        app.status = format!("{error:#}");
    }
}

fn handle_dashboard(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.filtering {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.filtering = false,
            KeyCode::Backspace => {
                app.filter.pop();
                app.selected = 0;
            }
            KeyCode::Char(c) => {
                app.filter.push(c);
                app.selected = 0;
            }
            _ => {}
        }
        return Ok(());
    }
    let count = app.visible_indices().len();
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Down | KeyCode::Char('j') => {
            app.selected = (app.selected + 1).min(count.saturating_sub(1))
        }
        KeyCode::Up | KeyCode::Char('k') => app.selected = app.selected.saturating_sub(1),
        KeyCode::Home => app.selected = 0,
        KeyCode::End => app.selected = count.saturating_sub(1),
        KeyCode::Enter | KeyCode::Char('o') => app.launch_selected(false)?,
        KeyCode::Char('x') => app.launch_selected(true)?,
        KeyCode::Char('a') => app.open_add_menu(),
        KeyCode::Char('n') => app.open_new_form(),
        KeyCode::Char('/') => app.filtering = true,
        KeyCode::Char('r') => app.revalidate_selected()?,
        KeyCode::Char('d') => app.confirm_unregister_selected()?,
        KeyCode::Char(',') => app.open_settings(),
        KeyCode::Char('?') => app.screen = Screen::Help,
        _ => {}
    }
    Ok(())
}

fn handle_add_menu(app: &mut App, key: KeyEvent, mut selected: usize) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.screen = Screen::Dashboard,
        KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(1),
        KeyCode::Enter => app.path_input(if selected == 0 {
            PathPurpose::Add
        } else {
            PathPurpose::Scan
        }),
        _ => {}
    }
    if matches!(app.screen, Screen::AddMenu { .. }) {
        app.screen = Screen::AddMenu { selected };
    }
    Ok(())
}

fn handle_path(
    app: &mut App,
    key: KeyEvent,
    purpose: PathPurpose,
    mut input: String,
) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.screen = Screen::Dashboard,
        KeyCode::Enter => {
            let path = expand_tilde(&input);
            match purpose {
                PathPurpose::Add => app.queue_imports(vec![path]),
                PathPurpose::Scan => app.scan(path)?,
            }
        }
        _ => edit_text(&mut input, key),
    }
    if matches!(app.screen, Screen::PathInput { .. }) {
        app.screen = Screen::PathInput { purpose, input };
    }
    Ok(())
}

fn handle_scan(
    app: &mut App,
    key: KeyEvent,
    root: PathBuf,
    mut candidates: Vec<crate::app::ScanCandidate>,
    mut cursor: usize,
) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.cancel_workflow(),
        KeyCode::Up | KeyCode::Char('k') => cursor = cursor.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            cursor = (cursor + 1).min(candidates.len().saturating_sub(1))
        }
        KeyCode::Char(' ') => {
            if let Some(item) = candidates.get_mut(cursor)
                && !item.registered
            {
                item.selected = !item.selected;
            }
        }
        KeyCode::Char('a') => {
            for item in &mut candidates {
                if !item.registered {
                    item.selected = true;
                }
            }
        }
        KeyCode::Char('c') => {
            for item in &mut candidates {
                item.selected = false;
            }
        }
        KeyCode::Enter => {
            let paths = candidates
                .iter()
                .filter(|item| item.selected && !item.registered)
                .map(|item| item.path.clone())
                .collect();
            app.queue_imports(paths);
        }
        _ => {}
    }
    if matches!(app.screen, Screen::ScanResults { .. }) {
        app.screen = Screen::ScanResults {
            root,
            candidates,
            cursor,
        };
    }
    Ok(())
}

fn handle_shells(
    app: &mut App,
    key: KeyEvent,
    path: PathBuf,
    shells: Vec<String>,
    mut selected: usize,
) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.skip_import(path.clone()),
        KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            selected = (selected + 1).min(shells.len().saturating_sub(1))
        }
        KeyCode::Enter => {
            if let Some(shell) = shells.get(selected) {
                app.validate_shell(path.clone(), shell.clone());
            }
        }
        _ => {}
    }
    if matches!(app.screen, Screen::ShellChoice { .. }) {
        app.screen = Screen::ShellChoice {
            path,
            shells,
            selected,
        };
    }
    Ok(())
}

fn handle_invalid_flake(
    app: &mut App,
    key: KeyEvent,
    path: PathBuf,
    diagnostic: String,
    mut selected: usize,
) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.resolve_invalid_flake(path.clone(), false),
        KeyCode::Left | KeyCode::Up | KeyCode::BackTab => selected = selected.saturating_sub(1),
        KeyCode::Right | KeyCode::Down | KeyCode::Tab => selected = (selected + 1).min(1),
        KeyCode::Enter => app.resolve_invalid_flake(path.clone(), selected == 1),
        _ => {}
    }
    if matches!(app.screen, Screen::InvalidFlake { .. }) {
        app.screen = Screen::InvalidFlake {
            path,
            diagnostic,
            selected,
        };
    }
    Ok(())
}

fn handle_existing_form(app: &mut App, key: KeyEvent, mut form: ExistingForm) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.skip_import(form.path.clone()),
        KeyCode::Tab | KeyCode::Down => form.field = (form.field + 1) % 2,
        KeyCode::BackTab | KeyCode::Up => form.field = (form.field + 1) % 2,
        KeyCode::Enter if form.field == 1 => return app.confirm_existing(form),
        KeyCode::Enter => form.field = 1,
        _ => {
            if form.field == 0 {
                edit_text(&mut form.version, key)
            } else {
                edit_text(&mut form.tools, key)
            }
        }
    }
    if matches!(app.screen, Screen::ExistingForm(_)) {
        app.screen = Screen::ExistingForm(form);
    }
    Ok(())
}

fn handle_create_form(app: &mut App, key: KeyEvent, mut form: CreateForm) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.screen = Screen::Dashboard,
        KeyCode::Tab | KeyCode::Down => form.field = (form.field + 1) % 6,
        KeyCode::BackTab | KeyCode::Up => form.field = (form.field + 5) % 6,
        KeyCode::Left if form.field == 3 => {
            form.renderer_index = form.renderer_index.saturating_sub(1)
        }
        KeyCode::Right if form.field == 3 => {
            form.renderer_index = (form.renderer_index + 1)
                .min(Renderer::choices_for(&form.version).len().saturating_sub(1));
        }
        KeyCode::Char(' ') if form.field == 4 => form.git_metadata = !form.git_metadata,
        KeyCode::Enter if form.field == 5 => return app.confirm_create(form),
        KeyCode::Enter => form.field = (form.field + 1).min(5),
        _ => match form.field {
            0 => edit_text(&mut form.name, key),
            1 => edit_text(&mut form.parent, key),
            2 => {
                edit_text(&mut form.version, key);
                form.renderer_index = 0;
            }
            5 => edit_text(&mut form.tools, key),
            _ => {}
        },
    }
    if matches!(app.screen, Screen::CreateForm(_)) {
        app.screen = Screen::CreateForm(form);
    }
    Ok(())
}

fn handle_settings(
    app: &mut App,
    key: KeyEvent,
    mut directory: String,
    mut field: usize,
    mut create_envrc: bool,
) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.screen = Screen::Dashboard,
        KeyCode::Tab | KeyCode::Down | KeyCode::Up | KeyCode::BackTab => field = (field + 1) % 2,
        KeyCode::Char(' ') if field == 1 => create_envrc = !create_envrc,
        KeyCode::Enter if field == 1 => return app.save_settings(directory, create_envrc),
        KeyCode::Enter => field = 1,
        _ if field == 0 => edit_text(&mut directory, key),
        _ => {}
    }
    if matches!(app.screen, Screen::Settings { .. }) {
        app.screen = Screen::Settings {
            directory,
            field,
            create_envrc,
        };
    }
    Ok(())
}

fn handle_confirm(app: &mut App, key: KeyEvent, action: ConfirmAction) -> Result<()> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => app.execute_confirm(action),
        KeyCode::Char('n') | KeyCode::Esc => app.cancel_confirm(&action),
        _ => {}
    }
    Ok(())
}

fn edit_text(value: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Backspace => {
            value.pop();
        }
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            value.push(c)
        }
        _ => {}
    }
}

fn expand_tilde(value: &str) -> PathBuf {
    if (value == "~" || value.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(value.trim_start_matches("~/"));
    }
    PathBuf::from(value)
}

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    draw_dashboard(frame, app);
    match &app.screen {
        Screen::Dashboard => {}
        Screen::AddMenu { selected } => modal_list(
            frame,
            "Add projects",
            &["Add an individual project", "Scan a directory"],
            *selected,
            "Enter choose · Esc cancel",
        ),
        Screen::PathInput { purpose, input } => modal_text(
            frame,
            match purpose {
                PathPurpose::Add => "Project directory",
                PathPurpose::Scan => "Directory to scan",
            },
            vec![
                Line::from("Enter an absolute path (~/ is supported):"),
                Line::from(Span::styled(input, input_style(true))),
                Line::from(""),
                Line::from("Enter continue · Esc cancel"),
            ],
        ),
        Screen::ScanResults {
            root,
            candidates,
            cursor,
        } => draw_scan(frame, root, candidates, *cursor),
        Screen::ShellChoice {
            path,
            shells,
            selected,
        } => {
            let owned = shells.iter().map(|s| s.as_str()).collect::<Vec<_>>();
            modal_list(
                frame,
                &format!("Dev shell — {}", path.display()),
                &owned,
                *selected,
                "Enter validate · Esc cancel",
            );
        }
        Screen::InvalidFlake {
            path,
            diagnostic,
            selected,
        } => draw_invalid_flake(frame, path, diagnostic, *selected),
        Screen::ExistingForm(form) => draw_existing_form(frame, app, form),
        Screen::CreateForm(form) => draw_create_form(frame, app, form),
        Screen::Settings {
            directory,
            field,
            create_envrc,
        } => draw_settings(frame, directory, *field, *create_envrc),
        Screen::Confirm { summary, .. } => modal_text(
            frame,
            "Confirm changes",
            vec![
                Line::from(summary.as_str()),
                Line::from(""),
                Line::from("y/Enter confirm · n/Esc cancel"),
            ],
        ),
        Screen::Help => draw_help(frame),
        Screen::Busy { message } => modal_text(
            frame,
            "Working",
            vec![
                Line::from(message.as_str()),
                Line::from(""),
                Line::from("Nix downloads and validation can take several minutes."),
            ],
        ),
    }
}

fn draw_dashboard(frame: &mut Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(frame.area());
    let filter = if app.filtering {
        format!("  Filter: {}█", app.filter)
    } else if app.filter.is_empty() {
        String::new()
    } else {
        format!("  Filter: {}", app.filter)
    };
    let title = format!(
        " Godot Nix Project Manager  ·  {} projects  ·  images: {}{} ",
        app.visible_indices().len(),
        app.image_protocol_name(),
        filter
    );
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(ACCENT)),
        chunks[0],
    );

    let body = if chunks[1].width >= 105 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(chunks[1])
    } else {
        std::rc::Rc::from(vec![chunks[1]])
    };
    draw_cards(frame, app, body[0]);
    if body.len() > 1 {
        draw_details(frame, app, body[1]);
    }

    let footer = Paragraph::new(vec![
        Line::from(
            " o/Enter open  x open+close  a add  n new  r revalidate  d unregister  , settings  / filter  ? help  q quit",
        ),
        Line::from(Span::styled(
            &app.status,
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    frame.render_widget(footer, chunks[2]);
}

fn draw_cards(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let indices = app.visible_indices();
    if indices.is_empty() {
        frame.render_widget(
            Paragraph::new("No projects registered. Press a to add one or n to create one.")
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::ALL).title(" Projects ")),
            area,
        );
        return;
    }
    let slots = (area.height / CARD_HEIGHT).max(1) as usize;
    let start = if app.selected >= slots {
        app.selected + 1 - slots
    } else {
        0
    };
    for (slot, visible_index) in (start..indices.len()).take(slots).enumerate() {
        let record = &app.state.projects[indices[visible_index]];
        let y = area.y + slot as u16 * CARD_HEIGHT;
        let rect = Rect::new(
            area.x,
            y,
            area.width,
            CARD_HEIGHT.min(area.bottom().saturating_sub(y)),
        );
        let selected = visible_index == app.selected;
        let border = if selected {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        frame.render_widget(
            Block::default().borders(Borders::ALL).border_style(border),
            rect,
        );
        let metadata =
            project::inspect(&record.path).unwrap_or_else(|_| crate::model::ProjectMetadata {
                name: record
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Missing project")
                    .into(),
                ..Default::default()
            });
        let image_width = if app.images.contains_key(&record.id) && rect.width > 30 {
            10
        } else {
            0
        };
        if let Some(protocol) = app.images.get(&record.id)
            && image_width > 0
        {
            frame.render_widget(
                Image::new(protocol).allow_clipping(true),
                Rect::new(rect.x + 1, rect.y + 1, 8, 3),
            );
        }
        let text_area = Rect::new(
            rect.x + 1 + image_width,
            rect.y + 1,
            rect.width.saturating_sub(2 + image_width),
            rect.height.saturating_sub(2),
        );
        let (status, color) = if !record.path.exists() {
            ("MISSING", Color::Red)
        } else if record.is_ready() {
            ("READY", Color::Green)
        } else {
            ("BROKEN", Color::Yellow)
        };
        let version = record.verified_version.as_deref().unwrap_or("unverified");
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(metadata.name, Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw("  "),
                    Span::styled(status, Style::default().fg(color)),
                ]),
                Line::from(format!(
                    "Godot {version}  ·  devShell: {}",
                    record.dev_shell
                )),
                Line::from(record.path.display().to_string()).dark_gray(),
            ]),
            text_area,
        );
    }
}

fn draw_details(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let indices = app.visible_indices();
    let lines = indices
        .get(app.selected)
        .map(|index| {
            let record = &app.state.projects[*index];
            let metadata = project::inspect(&record.path).unwrap_or_default();
            vec![
                Line::from(metadata.name).bold(),
                Line::from(""),
                Line::from("Path").fg(ACCENT),
                Line::from(record.path.display().to_string()),
                Line::from(""),
                Line::from("Dev shell").fg(ACCENT),
                Line::from(record.dev_shell.as_str()),
                Line::from(""),
                Line::from("Last opened").fg(ACCENT),
                Line::from(
                    record
                        .last_opened_at
                        .map(|v| v.to_rfc3339())
                        .unwrap_or_else(|| "Never".into()),
                ),
                Line::from(""),
                Line::from("Diagnostic").fg(ACCENT),
                Line::from(record.diagnostic.as_deref().unwrap_or("None")),
            ]
        })
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Details ")),
        area,
    );
}

fn draw_scan(
    frame: &mut Frame<'_>,
    root: &Path,
    candidates: &[crate::app::ScanCandidate],
    cursor: usize,
) {
    let area = centered(85, 80, frame.area());
    frame.render_widget(Clear, area);
    let items = candidates
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let check = if item.registered {
                "—"
            } else if item.selected {
                "✓"
            } else {
                " "
            };
            let flake = if item.has_flake {
                "flake"
            } else {
                "needs flake"
            };
            let line = format!(
                "[{check}] {}  ·  {flake}\n    {}",
                item.metadata.name,
                item.path.display()
            );
            let style = if index == cursor {
                Style::default().bg(ACCENT).fg(Color::Black)
            } else if item.registered {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(cursor));
    frame.render_stateful_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(format!(
            " Scan {} — {} projects ",
            root.display(),
            candidates.len()
        ))),
        area,
        &mut state,
    );
    let hint = Rect::new(
        area.x + 2,
        area.bottom().saturating_sub(2),
        area.width.saturating_sub(4),
        1,
    );
    frame.render_widget(
        Paragraph::new("Space toggle · a all · c clear · Enter add selected · Esc cancel"),
        hint,
    );
}

fn draw_existing_form(frame: &mut Frame<'_>, app: &App, form: &ExistingForm) {
    let suggestions = app
        .releases
        .iter()
        .take(4)
        .cloned()
        .collect::<Vec<_>>()
        .join("  ");
    modal_text(
        frame,
        if form.replace_existing {
            "Replace unusable flake"
        } else {
            "Generate required flake"
        },
        vec![
            Line::from(format!("Project: {}", form.path.display())),
            Line::from(""),
            field_line("Exact stable version", &form.version, form.field == 0),
            field_line("Extra nixpkgs tools", &form.tools, form.field == 1),
            Line::from(""),
            Line::from(format!("Recent stable releases: {suggestions}")).dark_gray(),
            Line::from("Tab move · Enter continue/confirm · Esc cancel"),
        ],
    );
}

fn draw_invalid_flake(frame: &mut Frame<'_>, path: &Path, diagnostic: &str, selected: usize) {
    let cancel_style = if selected == 0 {
        Style::default().bg(ACCENT).fg(Color::Black)
    } else {
        Style::default()
    };
    let overwrite_style = if selected == 1 {
        Style::default().bg(ACCENT).fg(Color::Black)
    } else {
        Style::default()
    };
    modal_text(
        frame,
        "Unusable project flake",
        vec![
            Line::from(format!("Project: {}", path.display())),
            Line::from(""),
            Line::from("The existing flake cannot provide a working Godot dev shell.")
                .fg(Color::Yellow),
            Line::from(diagnostic),
            Line::from(""),
            Line::from(Span::styled(
                "  Cancel adding this project (default)",
                cancel_style,
            )),
            Line::from(Span::styled(
                "  Overwrite with a generated flake",
                overwrite_style,
            )),
            Line::from(""),
            Line::from("↑/↓ or Tab choose · Enter confirm · Esc cancel"),
        ],
    );
}

fn draw_create_form(frame: &mut Frame<'_>, app: &App, form: &CreateForm) {
    let suggestions = app
        .releases
        .iter()
        .take(4)
        .cloned()
        .collect::<Vec<_>>()
        .join("  ");
    modal_text(
        frame,
        "Create Godot project",
        vec![
            field_line("Name", &form.name, form.field == 0),
            field_line("Parent directory", &form.parent, form.field == 1),
            field_line("Stable version", &form.version, form.field == 2),
            field_line("Renderer ←/→", form.renderer().label(), form.field == 3),
            field_line(
                "Git metadata Space",
                if form.git_metadata {
                    "enabled"
                } else {
                    "disabled"
                },
                form.field == 4,
            ),
            field_line("Extra nixpkgs tools", &form.tools, form.field == 5),
            Line::from(""),
            Line::from(format!("Recent stable releases: {suggestions}")).dark_gray(),
            Line::from("Tab move · Enter next/create · Esc cancel"),
        ],
    );
}

fn draw_settings(frame: &mut Frame<'_>, directory: &str, field: usize, create_envrc: bool) {
    modal_text(
        frame,
        "Settings",
        vec![
            field_line("Default projects directory", directory, field == 0),
            field_line(
                "Create .envrc Space",
                if create_envrc { "enabled" } else { "disabled" },
                field == 1,
            ),
            Line::from(""),
            Line::from("Tab move · Enter save · Esc cancel"),
            Line::from("Shell close integration: eval \"$(godot-nvm shell-init zsh)\"").dark_gray(),
        ],
    );
}

fn draw_help(frame: &mut Frame<'_>) {
    modal_text(
        frame,
        "Help",
        vec![
            Line::from("Godot Nix Project Manager").bold(),
            Line::from(""),
            Line::from("The selected project's Nix dev shell is always the source of truth."),
            Line::from("o/Enter launches a detached editor and keeps this dashboard open."),
            Line::from("x launches it and asks the sourced shell integration to exit."),
            Line::from("r rebuilds the selected dev shell and refreshes its Godot version."),
            Line::from("d only unregisters; it never deletes project files."),
            Line::from(""),
            Line::from("Press Esc, q, or ? to close help."),
        ],
    );
}

fn modal_list(frame: &mut Frame<'_>, title: &str, entries: &[&str], selected: usize, hint: &str) {
    let mut lines = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if index == selected {
                Line::from(format!("› {entry}")).style(Style::default().bg(ACCENT).fg(Color::Black))
            } else {
                Line::from(format!("  {entry}"))
            }
        })
        .collect::<Vec<_>>();
    lines.push(Line::from(""));
    lines.push(Line::from(hint));
    modal_text(frame, title, lines);
}

fn modal_text(frame: &mut Frame<'_>, title: &str, lines: Vec<Line<'_>>) {
    let area = centered(78, 62, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} "))
                .border_style(Style::default().fg(ACCENT)),
        ),
        area,
    );
}

fn field_line<'a>(label: &'a str, value: &'a str, active: bool) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{label:24}"), Style::default().fg(ACCENT)),
        Span::styled(value, input_style(active)),
    ])
}

fn input_style(active: bool) -> Style {
    if active {
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn centered(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home_paths() {
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(expand_tilde("~/games"), PathBuf::from(home).join("games"));
        }
    }
}
