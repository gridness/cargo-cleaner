use dialoguer::{Input, MultiSelect, Select};
use eyre::{Result, WrapErr, eyre};
use serde::Serialize;
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};

pub struct Ui {
    pub json: bool,
}

impl Ui {
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    pub fn interactive(&self) -> bool {
        !self.json && io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    pub fn json(&self, value: &impl Serialize) -> Result<bool> {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer(&mut stdout, value).wrap_err("Could not write JSON")?;
        writeln!(stdout)?;
        Ok(true)
    }

    pub fn warn(&self, message: &str) {
        if !self.json {
            eprintln!(
                "{} {message}",
                console::style("Skipped").yellow().for_stderr()
            );
        }
    }

    pub fn error(message: &str) {
        eprintln!("{} {message}", console::style("Error").red().for_stderr());
    }
}

pub fn safe(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_control() || matches!(ch, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

pub fn path(value: &Path) -> String {
    safe(&value.to_string_lossy())
}

pub fn size(bytes: u64) -> String {
    let mut amount = bytes as f64;
    for unit in ["B", "KiB", "MiB", "GiB", "TiB"] {
        if amount < 1024.0 || unit == "TiB" {
            return if unit == "B" {
                format!("{bytes} B")
            } else {
                format!("{amount:.1} {unit}")
            };
        }
        amount /= 1024.0;
    }
    unreachable!()
}

pub fn select(prompt: &str, choices: &[String]) -> Result<usize> {
    Select::new()
        .with_prompt(prompt)
        .items(choices)
        .default(0)
        .max_length(10)
        .report(false)
        .interact_opt()
        .wrap_err("Could not read the selection")?
        .ok_or_else(|| eyre!("Cancelled"))
}

pub fn multi(prompt: &str, choices: &[String]) -> Result<Vec<usize>> {
    let mut prompt = prompt.to_owned();
    loop {
        let result = MultiSelect::new()
            .with_prompt(&prompt)
            .items(choices)
            .max_length(10)
            .report(false)
            .interact_opt()
            .wrap_err("Could not read the selection")?
            .ok_or_else(|| eyre!("Cancelled"))?;
        if !result.is_empty() {
            return Ok(result);
        }
        prompt = "Select at least one project with Space. Press Enter when you're done.".to_owned();
    }
}

pub fn input<T: std::str::FromStr<Err = String>>(prompt: &str, initial: &str) -> Result<T> {
    let text = Input::<String>::new()
        .with_prompt(prompt)
        .with_initial_text(initial)
        .allow_empty(true)
        .report(false)
        .validate_with(|text: &String| text.parse::<T>().map(|_| ()))
        .interact_text()
        .wrap_err("Could not read the answer")?;
    text.parse().map_err(|error| eyre!("{error}"))
}
