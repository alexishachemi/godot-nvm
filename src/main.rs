mod app;
mod config;
mod generator;
mod icon;
mod launcher;
mod model;
mod nix;
mod paths;
mod project;
mod release;
mod ui;

use std::{
    io::{self, IsTerminal},
    process::ExitCode,
    time::Duration,
};

use anyhow::{Context, Result};
use app::App;
use clap::{Parser, Subcommand, ValueEnum};
use crossterm::{
    event, execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use ratatui_image::picker::Picker;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print shell integration for the launch-and-close action.
    ShellInit { shell: Shell },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Shell {
    Bash,
    Zsh,
}

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("godot-nvm: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<i32> {
    let cli = Cli::parse();
    if let Some(Command::ShellInit { shell }) = cli.command {
        print_shell_init(shell);
        return Ok(0);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("the dashboard requires an interactive terminal");
    }
    run_tui()
}

fn run_tui() -> Result<i32> {
    let paths = paths::AppPaths::discover()?;
    let mut app = App::load(paths)?;
    enable_raw_mode().context("could not enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("could not enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let picker = Picker::from_query_stdio().ok();
    app.set_picker(picker);
    let run_result = (|| -> Result<()> {
        while !app.should_quit {
            app.poll_workers();
            terminal.draw(|frame| ui::draw(frame, &app))?;
            if event::poll(Duration::from_millis(100))?
                && let event::Event::Key(key) = event::read()?
            {
                ui::handle_key(&mut app, key);
            }
        }
        Ok(())
    })();

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    run_result?;
    if app.exit_code == app::EXIT_CLOSE_SHELL
        && std::env::var_os("GODOT_NVM_SHELL_INTEGRATION").is_none()
    {
        eprintln!(
            "Godot was launched. To let close mode exit this shell, run: eval \"$(godot-nvm shell-init zsh)\""
        );
        return Ok(0);
    }
    Ok(app.exit_code)
}

fn print_shell_init(shell: Shell) {
    match shell {
        Shell::Bash => print!(
            r#"gnvm() {{
  GODOT_NVM_SHELL_INTEGRATION=1 command godot-nvm "$@"
  local godot_nvm_status=$?
  if [ "$godot_nvm_status" -eq 20 ]; then
    exit 0
  fi
  return "$godot_nvm_status"
}}
"#
        ),
        Shell::Zsh => print!(
            r#"function gnvm() {{
  GODOT_NVM_SHELL_INTEGRATION=1 command godot-nvm "$@"
  local godot_nvm_status=$?
  if (( godot_nvm_status == 20 )); then
    exit 0
  fi
  return $godot_nvm_status
}}
"#
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_exit_code_fits_shell_status() {
        assert!((1..=125).contains(&app::EXIT_CLOSE_SHELL));
    }
}
