use crate::{
    managed,
    schedule::Store,
    ui::{self, Ui},
};
use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
};

const FILE: &str = "last-command.report";
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Default, Serialize, Deserialize)]
pub struct Details {
    entries: Vec<Detail>,
    omitted: usize,
}

#[derive(Serialize, Deserialize)]
struct Detail {
    path: Option<PathBuf>,
    message: String,
}

impl Details {
    pub fn omit(&mut self, count: usize) {
        self.omitted = self.omitted.saturating_add(count);
    }

    pub fn push(&mut self, path: Option<&Path>, message: String) {
        if self.entries.len() < 200 {
            let mut message = message;
            if let Some((end, _)) = message.char_indices().nth(4096) {
                message.truncate(end);
                message.push_str("... [detail truncated]");
            }
            self.entries.push(Detail {
                path: path.map(Path::to_owned),
                message,
            });
        } else {
            self.omitted = self.omitted.saturating_add(1);
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Report {
    command: String,
    description: String,
    success: bool,
    #[serde(flatten)]
    details: Details,
}

pub fn save(
    ui: &Ui,
    command: String,
    description: String,
    result: &Result<ExitCode>,
) -> Result<()> {
    let mut details = ui.explanation.take();
    if let Err(error) = result {
        details.push(None, format!("The command stopped because: {error:#}"));
    }
    let mut report = Report {
        command,
        description,
        success: result.as_ref().is_ok_and(|code| *code == ExitCode::SUCCESS),
        details,
    };
    let store = Store::open()?;
    let _lock = store.lock("explain.lock")?;
    let mut bytes = serde_json::to_vec(&report)?;
    while bytes.len() as u64 > MAX_BYTES && !report.details.entries.is_empty() {
        report.details.entries.pop();
        report.details.omitted = report.details.omitted.saturating_add(1);
        bytes = serde_json::to_vec(&report)?;
    }
    // ASVS 14.2.4, 16.4.2: keep only the latest report in private user state.
    // Publish atomically so another command never reads a partial report.
    let mut temp = tempfile::NamedTempFile::new_in(&store.path)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(store.path.join(FILE))
        .wrap_err("Could not save the explanation")?;
    Ok(())
}

pub fn show(ui: &Ui) -> Result<bool> {
    let store = Store::open()?;
    // ASVS 1.5.2: bound typed JSON input and reject symlinks and special files.
    let Some(contents) = managed::read(&store.path.join(FILE), MAX_BYTES)? else {
        if ui.json {
            return ui.json(&Option::<Report>::None);
        }
        ui.line("No previous command to explain. Run a cargo-cleaner command first. Explanations are saved starting with this version.")?;
        return Ok(true);
    };
    let report: Report = serde_json::from_slice(&contents.bytes).wrap_err(
        "Could not read the saved explanation. Run another cargo-cleaner command to replace it",
    )?;
    if ui.json {
        return ui.json(&report);
    }
    // ASVS 1.1.2, 16.4.1: escape external text only when displaying it.
    ui.line(format!("Previous command: {}", ui::safe(&report.command)))?;
    ui.line(ui::safe(&report.description))?;
    ui.line(if report.success {
        "The command completed successfully."
    } else {
        "The command did not fully complete. See the reasons below."
    })?;
    for detail in report.details.entries {
        if let Some(path) = detail.path {
            ui.line(format!(
                "{}: {}",
                ui::path(&path),
                ui::safe(&detail.message)
            ))?;
        } else {
            ui.line(ui::safe(&detail.message))?;
        }
    }
    if report.details.omitted > 0 {
        ui.line(format!(
            "{} more details were omitted to keep the saved explanation small.",
            report.details.omitted
        ))?;
    }
    Ok(true)
}
