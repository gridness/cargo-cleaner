#![forbid(unsafe_code)]

mod projects;
mod schedule;
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

fn run() -> Result<bool> {
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
    if let Some(result) = clap_autocomplete::test_subcommand(&matches, command) {
        result.map_err(|error| eyre!("{error}"))?;
        return Ok(true);
    }
    let cli = Cli::from_arg_matches(&matches)?;
    let ui = Ui::new(cli.json);
    match cli.command {
        Task::Scan { path } => projects::scan(&ui, path),
        Task::Clean { project, dry_run } => projects::clean(&ui, &project, dry_run),
        Task::Schedule(action) => schedule::run(&ui, action),
    }
}

fn main() -> ExitCode {
    if std::env::var_os("NO_COLOR").is_some() {
        console::set_colors_enabled(false);
        console::set_colors_enabled_stderr(false);
    }
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            Ui::error(&ui::safe(&error.to_string()));
            ExitCode::FAILURE
        }
    }
}
