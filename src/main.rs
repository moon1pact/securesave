use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

use clap::{Parser, Subcommand};

use securesave::{Api, Compression, Job, JobStatus, Result, Summary};

mod serve;

#[derive(Parser)]
#[command(name = "securesave", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(
        about = "Run a backup job from the configuration, or back up a directory",
        long_about = "Run a backup job from the configuration, or back up a directory.\n\n\
            With one argument, runs the job of that name declared in the configuration \
            file. With two arguments, backs up SOURCE into DESTINATION directly."
    )]
    Backup {
        #[arg(
            value_name = "JOB|SOURCE",
            help = "A job name from the configuration file, or a source directory"
        )]
        target: String,
        #[arg(help = "Destination directory (only when backing up a directory directly)")]
        destination: Option<PathBuf>,
    },
    #[command(about = "List the backup jobs defined in the configuration file")]
    List,
    #[command(
        about = "Restore a backup into a new or empty directory",
        long_about = "Restore a backup into a new or empty directory.\n\n\
            The backup format (plain or compressed) is detected automatically. The \
            target must not exist or must be empty: restore never overwrites anything."
    )]
    Restore {
        #[arg(help = "Backup directory to restore from")]
        backup: PathBuf,
        #[arg(help = "Directory to restore into (must not exist or be empty)")]
        target: PathBuf,
    },
    #[command(
        about = "Verify the integrity of a job's backup",
        long_about = "Verify the integrity of a job's backup.\n\n\
            Compressed backups are checked against their manifest and every file is \
            fully decompressed and read. Plain backups are checked structurally and \
            for completeness against the source."
    )]
    Verify {
        #[arg(help = "A job name from the configuration file")]
        job: String,
    },
    #[command(about = "Show the state of every configured job")]
    Status,
    #[command(about = "Serve a read-only status dashboard on localhost")]
    Serve {
        #[arg(
            long,
            default_value_t = 7878,
            help = "Port to listen on (always bound to 127.0.0.1)"
        )]
        port: u16,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let api = Api::from_env();

    match cli.command {
        Command::Backup {
            target,
            destination,
        } => report(
            "Backup",
            match destination {
                Some(destination) => api.backup_path(Path::new(&target), &destination),
                None => api.backup_job(&target),
            },
        ),
        Command::List => run_list(&api),
        Command::Restore { backup, target } => report("Restore", api.restore(&backup, &target)),
        Command::Verify { job } => run_verify(&api, &job),
        Command::Status => run_status(&api),
        Command::Serve { port } => serve::run(&api, port),
    }
}

fn run_list(api: &Api) -> ExitCode {
    match api.list_jobs() {
        Ok(jobs) if jobs.is_empty() => {
            match api.config_path() {
                Some(path) => println!("No jobs defined in {}", path.display()),
                None => println!("No jobs defined"),
            }
            ExitCode::SUCCESS
        }
        Ok(jobs) => {
            print!("{}", format_jobs(&jobs));
            ExitCode::SUCCESS
        }
        Err(err) => fail(&err),
    }
}

fn run_verify(api: &Api, job: &str) -> ExitCode {
    match api.verify_job(job) {
        Ok(report) => {
            for issue in &report.issues {
                println!("issue: {issue}");
            }
            println!(
                "Verify complete: {} file(s), {} bytes checked, {} issue(s)",
                report.files_checked,
                report.bytes_checked,
                report.issues.len(),
            );
            if report.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(err) => fail(&err),
    }
}

fn run_status(api: &Api) -> ExitCode {
    match api.status() {
        Ok(statuses) if statuses.is_empty() => {
            match api.config_path() {
                Some(path) => println!("No jobs defined in {}", path.display()),
                None => println!("No jobs defined"),
            }
            ExitCode::SUCCESS
        }
        Ok(statuses) => {
            print!("{}", format_statuses(&statuses, SystemTime::now()));
            ExitCode::SUCCESS
        }
        Err(err) => fail(&err),
    }
}

fn format_jobs(jobs: &BTreeMap<String, Job>) -> String {
    let rows: Vec<(&str, String, String)> = jobs
        .iter()
        .map(|(name, job)| {
            (
                name.as_str(),
                job.source.display().to_string(),
                job.destination.display().to_string(),
            )
        })
        .collect();
    let name_width = rows.iter().map(|(name, ..)| name.len()).max().unwrap_or(0);
    let source_width = rows
        .iter()
        .map(|(_, source, _)| source.len())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    for (name, source, destination) in rows {
        writeln!(
            out,
            "{name:<name_width$}  {source:<source_width$}  -> {destination}"
        )
        .expect("writing to a String cannot fail");
    }
    out
}

fn format_statuses(statuses: &[JobStatus], now: SystemTime) -> String {
    let name_width = statuses.iter().map(|s| s.name.len()).max().unwrap_or(0);

    let mut out = String::new();
    for status in statuses {
        let compression = match status.compression {
            Compression::None => "none",
            Compression::Zstd => "zstd",
        };
        let mut state = if status.destination_exists {
            "OK".to_string()
        } else {
            "destination missing".to_string()
        };
        match status.last_run {
            Some(time) => {
                let _ = write!(state, ", last run {}", ago(time, now));
                if let Some(files) = status.files_recorded {
                    let _ = write!(state, ", {files} file(s)");
                }
            }
            None => state.push_str(", last run unknown"),
        }
        writeln!(out, "{:<name_width$}  {compression}  {state}", status.name)
            .expect("writing to a String cannot fail");
    }
    out
}

fn ago(time: SystemTime, now: SystemTime) -> String {
    let Ok(elapsed) = now.duration_since(time) else {
        return "in the future?".to_string();
    };
    let secs = elapsed.as_secs();
    match secs {
        0..60 => format!("{secs} second(s) ago"),
        60..3600 => format!("{} minute(s) ago", secs / 60),
        3600..86400 => format!("{} hour(s) ago", secs / 3600),
        _ => format!("{} day(s) ago", secs / 86400),
    }
}

fn report(action: &str, result: Result<Summary>) -> ExitCode {
    match result {
        Ok(summary) => {
            for warning in &summary.warnings {
                eprintln!("securesave: warning: {warning}");
            }
            let mut message = format!(
                "{action} complete: {} file(s) copied ({} bytes), {} up to date, {} symlink(s), {} skipped",
                summary.files_copied,
                summary.bytes_copied,
                summary.files_unchanged + summary.symlinks_unchanged,
                summary.symlinks_recreated,
                summary.entries_skipped,
            );
            if summary.bytes_written != summary.bytes_copied {
                let _ = write!(message, " [compressed to {} bytes]", summary.bytes_written);
            }
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(err) => fail(&err),
    }
}

fn fail(err: &securesave::Error) -> ExitCode {
    eprintln!("securesave: error: {err}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn job(source: &str, destination: &str) -> Job {
        Job {
            source: PathBuf::from(source),
            destination: PathBuf::from(destination),
            compression: Compression::None,
        }
    }

    #[test]
    fn formats_jobs_as_aligned_columns() {
        let jobs = BTreeMap::from([
            (
                "photos".to_string(),
                job("/home/moon/Photos", "/mnt/backup/photos"),
            ),
            (
                "vid".to_string(),
                job("/home/moon/Videos/HD", "/mnt/backup/videos"),
            ),
        ]);

        assert_eq!(
            format_jobs(&jobs),
            "photos  /home/moon/Photos     -> /mnt/backup/photos\n\
             vid     /home/moon/Videos/HD  -> /mnt/backup/videos\n"
        );
    }

    #[test]
    fn formats_no_jobs_as_an_empty_string() {
        assert_eq!(format_jobs(&BTreeMap::new()), "");
    }

    #[test]
    fn formats_statuses_with_and_without_a_last_run() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let statuses = vec![
            JobStatus {
                name: "documents".to_string(),
                source: PathBuf::from("/s"),
                destination: PathBuf::from("/d"),
                compression: Compression::None,
                destination_exists: false,
                last_run: None,
                files_recorded: None,
            },
            JobStatus {
                name: "photos".to_string(),
                source: PathBuf::from("/s"),
                destination: PathBuf::from("/d"),
                compression: Compression::Zstd,
                destination_exists: true,
                last_run: Some(now - Duration::from_secs(2 * 3600)),
                files_recorded: Some(2841),
            },
        ];

        assert_eq!(
            format_statuses(&statuses, now),
            "documents  none  destination missing, last run unknown\n\
             photos     zstd  OK, last run 2 hour(s) ago, 2841 file(s)\n"
        );
    }

    #[test]
    fn ago_picks_the_right_unit() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let cases = [
            (30, "30 second(s) ago"),
            (240, "4 minute(s) ago"),
            (7200, "2 hour(s) ago"),
            (200_000, "2 day(s) ago"),
        ];
        for (secs, expected) in cases {
            assert_eq!(ago(base - Duration::from_secs(secs), base), expected);
        }
    }
}
