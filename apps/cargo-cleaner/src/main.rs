#![forbid(unsafe_code)]

mod doctor;
mod explain;
mod git;
mod hooks;
mod managed;
mod projects;
mod schedule;
mod skill;
mod ui;

use clap::{
    Arg, ArgAction, Command, CommandFactory, FromArgMatches, Parser, Subcommand, ValueHint,
    builder::PossibleValuesParser,
};
use eyre::{Result, eyre};
use std::{path::PathBuf, process::ExitCode};
use ui::Ui;

#[derive(Parser)]
#[command(name = "cargo-cleaner", version = version(), about, arg_required_else_help = true)]
struct Cli {
    /// Write results as JSON for scripts
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Find Cargo projects and measure their build caches
    Scan {
        /// Search this folder instead of the whole system
        #[arg(long, value_hint = ValueHint::DirPath)]
        path: Option<PathBuf>,
    },
    /// Clean build caches for a folder, a project, or the whole system
    Clean {
        /// Use "." for this folder and its subfolders, "all" for the system, or a project name or path
        #[arg(value_hint = ValueHint::DirPath)]
        project: PathBuf,

        /// Show what would be cleaned
        #[arg(long)]
        dry_run: bool,
    },
    /// Manage recurring cleanup jobs
    #[command(subcommand)]
    Schedule(schedule::Action),
    /// Run cleanup after Git events or a command
    #[command(subcommand)]
    Hooks(hooks::Action),
    /// Install or remove the cargo-cleaner agent skill
    #[command(subcommand)]
    Skill(skill::Action),
    /// Check Cargo, scheduling, hooks, and agent skills
    Doctor,
    /// Explain the previous command's result and why entries were skipped
    Explain,
}

fn version() -> &'static str {
    option_env!("CARGO_CLEANER_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn long_flags(command: Command) -> Command {
    let command = command.disable_help_flag(true).arg(
        Arg::new("help")
            .long("help")
            .help("Print help")
            .action(ArgAction::Help),
    );
    command
        .mut_args(|arg| arg.short(None))
        .mut_subcommands(long_flags)
}

fn run() -> Result<ExitCode> {
    let command =
        clap_autocomplete::add_subcommand(Cli::command()).mut_subcommand("complete", |command| {
            command
                .about("Print a shell completion script")
                .mut_arg("shell", |arg| {
                    arg.required(true)
                        .value_parser(PossibleValuesParser::new([
                            "bash",
                            "zsh",
                            "fish",
                            "powershell",
                            "elvish",
                        ]))
                        .help("Shell to generate completions for")
                })
                .mut_arg("print", |arg| arg.default_value("true").hide(true))
        });
    let command = long_flags(command).disable_version_flag(true).arg(
        Arg::new("version")
            .long("version")
            .help("Print version")
            .action(ArgAction::Version),
    );
    let matches = command.clone().get_matches();
    if let Some(result) = clap_autocomplete::test_subcommand(&matches, command.clone()) {
        result.map_err(|error| eyre!("{error}"))?;
        return Ok(ExitCode::SUCCESS);
    }
    let cli = Cli::from_arg_matches(&matches)?;
    let ui = Ui::new(cli.json);
    if matches!(cli.command, Task::Explain) {
        explain::show(&ui)?;
        return Ok(ExitCode::SUCCESS);
    }
    // Scheduled runs have their own logs and must not replace a manual result.
    let remember = !matches!(cli.command, Task::Schedule(schedule::Action::Run { .. }));
    let mut command_name = "cargo-cleaner".to_owned();
    let mut definition = &command;
    let mut arguments = &matches;
    while let Some((name, submatches)) = arguments.subcommand() {
        command_name.push(' ');
        command_name.push_str(name);
        definition = definition
            .find_subcommand(name)
            .expect("parsed command exists");
        arguments = submatches;
    }
    let description = definition
        .get_about()
        .map(ToString::to_string)
        .unwrap_or_default();
    let result = (|| {
        let success = match cli.command {
            Task::Scan { path } => projects::scan(&ui, path),
            Task::Clean { project, dry_run } => projects::clean(&ui, &project, dry_run),
            Task::Schedule(action) => schedule::run(&ui, action),
            Task::Hooks(action) => return hooks::run(&ui, action),
            Task::Skill(action) => skill::run(&ui, action),
            Task::Doctor => doctor::run(&ui),
            Task::Explain => unreachable!(),
        }?;
        Ok(if success {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        })
    })();
    if remember && let Err(error) = explain::save(&ui, command_name, description, &result) {
        // Saving diagnostics must never change a command's result or JSON output.
        eprintln!(
            "Could not save the explanation: {}",
            ui::safe(&error.to_string())
        );
    }
    result
}

fn main() -> ExitCode {
    if std::env::var_os("NO_COLOR").is_some() {
        console::set_colors_enabled(false);
        console::set_colors_enabled_stderr(false);
    }
    match run() {
        Ok(code) => code,
        Err(error) => {
            Ui::error(&ui::safe(&error.to_string()));
            ExitCode::FAILURE
        }
    }
}
