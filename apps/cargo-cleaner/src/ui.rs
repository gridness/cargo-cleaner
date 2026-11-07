use dialoguer::{Input, MultiSelect, Select};
use eyre::{Result, WrapErr, eyre};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    io::{self, IsTerminal, Write},
    path::Path,
};

pub struct Ui {
    pub json: bool,
    transcript: Option<RefCell<Transcript>>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Transcript {
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

const LOG_BYTES: usize = 64 * 1024;

fn append_log(output: &mut String, truncated: &mut bool, text: &str) {
    let mut end = text.len().min(LOG_BYTES.saturating_sub(output.len()));
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&text[..end]);
    *truncated |= end < text.len();
}

impl Ui {
    pub fn new(json: bool) -> Self {
        Self {
            json,
            transcript: None,
        }
    }

    pub fn recording(json: bool) -> Self {
        Self {
            json,
            transcript: Some(RefCell::new(Transcript::default())),
        }
    }

    pub fn into_transcript(self) -> Transcript {
        self.transcript.map(RefCell::into_inner).unwrap_or_default()
    }

    fn record(&self, text: &str, stderr: bool) {
        if let Some(transcript) = &self.transcript {
            let mut transcript = transcript.borrow_mut();
            let Transcript {
                stdout,
                stderr: errors,
                stdout_truncated,
                stderr_truncated,
            } = &mut *transcript;
            if stderr {
                append_log(errors, stderr_truncated, text);
            } else {
                append_log(stdout, stdout_truncated, text);
            }
        }
    }

    pub fn line(&self, text: impl std::fmt::Display) -> Result<()> {
        let text = format!("{text}\n");
        self.record(&text, false);
        io::stdout().lock().write_all(text.as_bytes())?;
        Ok(())
    }

    pub fn interactive(&self) -> bool {
        !self.json && io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    pub fn json(&self, value: &impl Serialize) -> Result<bool> {
        if self.transcript.is_some() {
            let text = serde_json::to_string(value).wrap_err("Could not write JSON")?;
            self.line(text)?;
            return Ok(true);
        }
        let mut stdout = io::stdout().lock();
        serde_json::to_writer(&mut stdout, value).wrap_err("Could not write JSON")?;
        writeln!(stdout)?;
        Ok(true)
    }

    pub fn warn(&self, message: &str) {
        if !self.json {
            self.record(&format!("Skipped {message}\n"), true);
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

#[cfg(test)]
mod tests {
    use super::{LOG_BYTES, append_log};

    #[test]
    fn caps_log_output_without_splitting_utf8() {
        let mut output = "x".repeat(LOG_BYTES - 1);
        let mut truncated = false;
        append_log(&mut output, &mut truncated, "世界\n");
        assert!(truncated);
        assert_eq!(output.len(), LOG_BYTES - 1);
        append_log(&mut output, &mut truncated, "!");
        assert_eq!(output.len(), LOG_BYTES);
        append_log(&mut output, &mut truncated, "more");
        assert_eq!(output.len(), LOG_BYTES);
    }
}
