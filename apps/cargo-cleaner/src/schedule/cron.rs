use eyre::{Result, WrapErr, bail, ensure};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

const MARKER: &str = "\t# cargo-cleaner:";

pub struct Cron {
    table: String,
}

impl Cron {
    fn command() -> Command {
        let mut command = Command::new("/usr/bin/crontab");
        command.env("LC_ALL", "C").stderr(Stdio::piped());
        command
    }

    pub fn read() -> Result<Self> {
        let output = Self::command()
            .arg("-l")
            .output()
            .wrap_err("Could not open crontab. Scheduling requires cron.")?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            if !error.trim().starts_with("crontab: no crontab for ")
                && !error.trim().starts_with("no crontab for ")
            {
                bail!("Could not read your crontab. Check that cron is available for your user.");
            }
        }
        let table = String::from_utf8(output.stdout).wrap_err("Your crontab must be UTF-8")?;
        Ok(Self { table })
    }

    pub fn contains(&self, id: &str) -> bool {
        self.table.lines().any(|line| managed_id(line) == Some(id))
    }

    pub fn add(
        &mut self,
        expression: &str,
        executable: &Path,
        state: &Path,
        id: &str,
    ) -> Result<()> {
        let executable = quote(executable)?;
        let state = quote(state)?;
        if !self.table.is_empty() && !self.table.ends_with('\n') {
            self.table.push('\n');
        }
        self.table.push_str(&format!(
            "{expression} {executable} schedule run {id} --state {state}{MARKER}{id}\n"
        ));
        Ok(())
    }

    pub fn remove(&mut self, ids: &[String]) {
        self.table = self
            .table
            .split_inclusive('\n')
            .filter(|line| {
                !managed_id(line.trim_end_matches('\n'))
                    .is_some_and(|id| ids.iter().any(|selected| selected == id))
            })
            .collect();
    }

    pub fn install(self) -> Result<()> {
        let mut child = Self::command()
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .wrap_err("Could not start crontab")?;
        let write = child
            .stdin
            .take()
            .expect("crontab stdin was piped")
            .write_all(self.table.as_bytes());
        let output = child
            .wait_with_output()
            .wrap_err("Could not finish updating your crontab")?;
        ensure!(
            output.status.success(),
            "Could not update your crontab. Check that cron is available for your user."
        );
        write.wrap_err("Could not send the schedule to cron")?;
        Ok(())
    }
}

fn managed_id(line: &str) -> Option<&str> {
    let (command, id) = line.rsplit_once(MARKER)?;
    (!command.trim_start().starts_with('#')
        && command.contains(" schedule run ")
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    .then_some(id)
}

fn quote(path: &Path) -> Result<String> {
    // ASVS 1.2.5: quote shell arguments and reject characters cron interprets
    // before passing the command to the shell.
    let path = path
        .to_str()
        .ok_or_else(|| eyre::eyre!("Cron needs a UTF-8 file path"))?;
    ensure!(
        !path.contains(['%', '\n', '\r']),
        "Cron paths cannot contain a percent sign or line break"
    );
    Ok(format!("'{}'", path.replace('\'', "'\"'\"'")))
}

#[cfg(test)]
mod tests {
    use super::{Cron, quote};
    use std::path::Path;

    #[test]
    fn removes_only_managed_jobs_and_preserves_other_entries() {
        let original = "MAILTO=\"\"\n5 5 * * * existing-command\n";
        let mut cron = Cron {
            table: original.to_owned(),
        };
        cron.add(
            "0 2 * * 0",
            Path::new("/bin/cargo-cleaner"),
            Path::new("/path with spaces"),
            "job-123",
        )
        .unwrap();
        cron.add(
            "0 3 * * *",
            Path::new("/bin/cargo-cleaner"),
            Path::new("/other"),
            "job-456",
        )
        .unwrap();
        assert!(cron.contains("job-123"));
        cron.remove(&["job-123".into()]);
        assert!(!cron.contains("job-123"));
        assert!(cron.contains("job-456"));
        cron.remove(&["job-456".into()]);
        assert_eq!(cron.table, original);
    }

    #[test]
    fn quotes_shell_paths_and_rejects_cron_metacharacters() {
        assert_eq!(quote(Path::new("/tmp/a'b")).unwrap(), "'/tmp/a'\"'\"'b'");
        assert!(quote(Path::new("/tmp/a%b")).is_err());
        assert!(quote(Path::new("/tmp/a\nb")).is_err());
    }
}
