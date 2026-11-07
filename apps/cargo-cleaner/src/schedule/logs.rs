use super::{Job, Store, store::valid_id};
use crate::ui::{self, Transcript, Ui};
use eyre::{Result, WrapErr, bail, ensure};
use rustix::fs::{Mode, OFlags, open};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    os::unix::fs::MetadataExt,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const RUNS: usize = 20;
const HISTORY_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Run {
    id: String,
    started_at_unix_secs: u64,
    finished_at_unix_secs: Option<u64>,
    duration_ms: Option<u64>,
    status: Status,
    #[serde(flatten)]
    output: Transcript,
    error: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Store {
    pub(super) fn history_lock(&self) -> Result<File> {
        // ASVS 16.4.2: keep logs private and serialize only history updates.
        let fd = open(
            self.path.join("logs.lock"),
            OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        )
        .wrap_err("Could not open the schedule logs lock")?;
        let file = File::from(fd);
        private_file(&file)?;
        file.lock().wrap_err("Could not lock schedule logs")?;
        Ok(file)
    }

    fn history(&self, id: &str) -> Result<Vec<Run>> {
        // ASVS 5.3.2 and 2.2.1: validate IDs before using them as filenames.
        ensure!(valid_id(id), "Invalid schedule ID");
        let fd = match open(
            self.path.join(format!("{id}.logs")),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(Vec::new()),
            Err(error) => return Err(error).wrap_err("Could not open schedule logs"),
        };
        let file = File::from(fd);
        private_file(&file)?;
        ensure!(
            file.metadata()?.len() <= HISTORY_BYTES,
            "Schedule logs exceed the size limit"
        );
        serde_json::from_reader(file).wrap_err("Could not read schedule logs")
    }

    fn save_history(&self, id: &str, runs: &[Run]) -> Result<()> {
        ensure!(valid_id(id), "Invalid schedule ID");
        let mut temp = tempfile::Builder::new()
            .prefix("logs-")
            .tempfile_in(&self.path)?;
        serde_json::to_writer(temp.as_file_mut(), runs)?;
        temp.as_file().sync_all()?;
        temp.persist(self.path.join(format!("{id}.logs")))
            .wrap_err("Could not save schedule logs")?;
        Ok(())
    }
}

fn private_file(file: &File) -> Result<()> {
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o077 == 0
            && metadata.nlink() <= 1,
        "Schedule logs must be private regular files owned by you"
    );
    Ok(())
}

pub(super) fn start(store: &Store, id: &str) -> Result<(Run, Instant)> {
    let _lock = store.history_lock()?;
    let mut runs = store.history(id)?;
    let token = tempfile::Builder::new()
        .prefix("run-")
        .tempfile_in(&store.path)?;
    // ASVS 16.2.1 and 16.2.2: record run identity, outcome, and UTC timestamps.
    let run = Run {
        id: token
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        started_at_unix_secs: now(),
        finished_at_unix_secs: None,
        duration_ms: None,
        status: Status::Running,
        output: Transcript::default(),
        error: None,
    };
    runs.push(run.clone());
    if runs.len() > RUNS {
        runs.drain(..runs.len() - RUNS);
    }
    store.save_history(id, &runs)?;
    Ok((run, Instant::now()))
}

pub(super) fn finish(
    store: &Store,
    id: &str,
    mut run: Run,
    started: Instant,
    output: Transcript,
    result: &Result<bool>,
) -> Result<()> {
    run.finished_at_unix_secs = Some(now());
    run.duration_ms = Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
    run.status = if matches!(result, Ok(true)) {
        Status::Succeeded
    } else {
        Status::Failed
    };
    run.output = output;
    run.error = result
        .as_ref()
        .err()
        .map(|error| ui::safe(&format!("{error:#}")));
    let _lock = store.history_lock()?;
    // Removal holds the same lock, so finishing cannot recreate deleted history.
    if !store.jobs()?.iter().any(|job| job.id == id) {
        return Ok(());
    }
    let mut runs = store.history(id)?;
    if let Some(saved) = runs.iter_mut().find(|saved| saved.id == run.id) {
        *saved = run;
        store.save_history(id, &runs)?;
    }
    Ok(())
}

fn label(job: &Job) -> String {
    format!(
        "{}  {}  {}",
        job.id,
        ui::safe(&job.time),
        job.scope.description()
    )
}

pub(super) fn show(ui: &Ui, id: Option<&str>) -> Result<bool> {
    #[derive(Serialize)]
    struct Output<'a> {
        id: Option<&'a str>,
        runs: Vec<Run>,
    }
    let store = Store::open()?;
    let jobs = store.jobs()?;
    let job = if let Some(id) = id {
        Some(jobs.iter().find(|job| job.id == id).ok_or_else(|| {
            eyre::eyre!("No schedule with that ID. Run cargo-cleaner schedule list.")
        })?)
    } else {
        match jobs.as_slice() {
            [] => None,
            [job] => Some(job),
            _ => {
                let options = jobs.iter().map(label).collect::<Vec<_>>();
                if !ui.interactive() {
                    bail!(
                        "Use --id to choose a schedule. Available schedules: {}. Run cargo-cleaner schedule list for IDs.",
                        options.join("; ")
                    );
                }
                Some(&jobs[ui::select("Which schedule's logs?", &options)?])
            }
        }
    };
    let mut runs = job
        .map(|job| store.history(&job.id))
        .transpose()?
        .unwrap_or_default();
    runs.reverse();
    if ui.json {
        return ui.json(&Output {
            id: job.map(|job| job.id.as_str()),
            runs,
        });
    }
    let Some(job) = job else {
        println!("No cleanup schedules.");
        return Ok(true);
    };
    println!("{}", label(job));
    if runs.is_empty() {
        println!("No recorded runs. Logs are saved when this schedule next runs.");
    }
    for run in runs {
        let status = match run.status {
            Status::Running => "Running or interrupted",
            Status::Succeeded => "Succeeded",
            Status::Failed => "Failed",
        };
        let duration = run
            .duration_ms
            .map_or_else(String::new, |ms| format!("  {ms} ms"));
        println!(
            "\n{status}  started {}s ago{duration}",
            now().saturating_sub(run.started_at_unix_secs)
        );
        // ASVS 16.4.1: escape each line of stored output before terminal display.
        for (name, text, truncated) in [
            ("stdout", &run.output.stdout, run.output.stdout_truncated),
            ("stderr", &run.output.stderr, run.output.stderr_truncated),
        ] {
            if !text.is_empty() {
                println!("{name}:");
                for line in text.lines() {
                    println!("  {}", ui::safe(line));
                }
            }
            if truncated {
                println!("  [{name} truncated at 64 KiB]");
            }
        }
        if let Some(error) = run.error {
            println!("Error: {}", ui::safe(&error));
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{Scope, execute_job};
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        path::Path,
    };

    fn job(store: &Store, project: &Path) -> Job {
        store
            .save(Job {
                id: String::new(),
                time: "daily at 02:30".into(),
                scope: Scope::Projects(vec![project.to_owned()]),
            })
            .unwrap()
    }

    fn project(path: &Path) {
        fs::create_dir_all(path.join("src")).unwrap();
        fs::write(path.join("src/lib.rs"), "").unwrap();
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir(path.join("target")).unwrap();
        fs::write(
            path.join("target/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        fs::write(path.join("target/artifact"), [b'x'; 8192]).unwrap();
    }

    #[test]
    fn records_cleanup_success_safety_failure_and_fatal_error() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let workspace = root.join("project");
        project(&workspace);
        let store = Store::at(&root.join("state")).unwrap();
        let saved = job(&store, &workspace);
        let id = saved.id;
        let execute = |json| {
            let saved = store.jobs().unwrap().pop().unwrap();
            execute_job(
                &Ui::new(json),
                &store,
                saved,
                store.lock("schedules.lock").unwrap(),
            )
        };

        assert!(execute(false).unwrap());
        let runs = store.history(&id).unwrap();
        assert!(matches!(runs[0].status, Status::Succeeded));
        assert!(runs[0].finished_at_unix_secs.unwrap() >= runs[0].started_at_unix_secs);
        assert!(runs[0].duration_ms.is_some());
        assert!(runs[0].output.stdout.contains("Cleaned"));
        assert!(!workspace.join("target").exists());
        assert!(workspace.join("src/lib.rs").exists());

        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "keep me").unwrap();
        symlink(&outside, workspace.join("target")).unwrap();
        assert!(!execute(false).unwrap());
        let runs = store.history(&id).unwrap();
        assert!(matches!(runs[1].status, Status::Failed));
        assert!(runs[1].output.stderr.contains("Skipped"));
        assert!(outside.join("keep").is_file());
        assert!(workspace.join("target").is_symlink());

        assert!(!execute(true).unwrap());
        let runs = store.history(&id).unwrap();
        let summary: serde_json::Value = serde_json::from_str(&runs[2].output.stdout).unwrap();
        assert_eq!(summary["caches"][0]["status"], "skipped");

        let _cleanup = store.lock("cleanup.lock").unwrap();
        assert!(execute(false).is_err());
        let runs = store.history(&id).unwrap();
        assert!(matches!(runs[3].status, Status::Failed));
        assert!(
            runs[3]
                .error
                .as_ref()
                .unwrap()
                .contains("Another cargo-cleaner")
        );
    }

    #[test]
    fn retains_twenty_runs_and_finishes_overlapping_runs_by_id() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::at(&temp.path().join("state")).unwrap();
        let job = job(&store, temp.path());
        let (first, first_started) = start(&store, &job.id).unwrap();
        let (second, second_started) = start(&store, &job.id).unwrap();
        assert!(
            store
                .history(&job.id)
                .unwrap()
                .iter()
                .all(|run| matches!(run.status, Status::Running))
        );
        finish(
            &store,
            &job.id,
            second,
            second_started,
            Transcript::default(),
            &Ok(false),
        )
        .unwrap();
        finish(
            &store,
            &job.id,
            first,
            first_started,
            Transcript::default(),
            &Ok(true),
        )
        .unwrap();
        let runs = store.history(&job.id).unwrap();
        assert!(matches!(runs[0].status, Status::Succeeded));
        assert!(matches!(runs[1].status, Status::Failed));
        let old_id = runs[0].id.clone();
        for _ in 0..RUNS {
            let (run, started) = start(&store, &job.id).unwrap();
            finish(
                &store,
                &job.id,
                run,
                started,
                Transcript::default(),
                &Ok(true),
            )
            .unwrap();
        }
        let runs = store.history(&job.id).unwrap();
        assert_eq!(runs.len(), RUNS);
        assert!(runs.iter().all(|run| run.id != old_id));
        assert_eq!(
            fs::metadata(store.path.join(format!("{}.logs", job.id)))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn rejects_unsafe_history_and_does_not_restore_logs_after_removal() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::at(&temp.path().join("state")).unwrap();
        assert!(store.history("../other").is_err());
        let job = job(&store, temp.path());
        assert!(store.history(&job.id).unwrap().is_empty());
        let target = temp.path().join("keep");
        fs::write(&target, "keep me").unwrap();
        let log = store.path.join(format!("{}.logs", job.id));
        symlink(&target, &log).unwrap();
        assert!(start(&store, &job.id).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
        fs::remove_file(&log).unwrap();
        let (run, started) = start(&store, &job.id).unwrap();
        let snapshot = File::open(&log).unwrap();
        let (next, next_started) = start(&store, &job.id).unwrap();
        // Atomic replacement unlinks an already-open reader's previous snapshot.
        private_file(&snapshot).unwrap();
        finish(
            &store,
            &job.id,
            next,
            next_started,
            Transcript::default(),
            &Ok(true),
        )
        .unwrap();
        store.remove(&job.id).unwrap();
        finish(
            &store,
            &job.id,
            run,
            started,
            Transcript::default(),
            &Ok(true),
        )
        .unwrap();
        assert!(!log.exists());
        assert!(store.jobs().unwrap().is_empty());
    }
}
