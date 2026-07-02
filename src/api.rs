use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::backup::{self, BackupOptions, Compression, Summary, backup_dir};
use crate::config::{Config, Job};
use crate::error::{Error, Result};
use crate::manifest::Manifest;
use crate::restore::restore_dir;
use crate::verify::{VerifyReport, verify_backup};

pub struct Api {
    config_path: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct JobStatus {
    pub name: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub compression: Compression,
    pub destination_exists: bool,
    pub last_run: Option<SystemTime>,
    pub files_recorded: Option<u64>,
}

impl Api {
    pub fn from_env() -> Api {
        Api {
            config_path: Config::default_path(),
        }
    }

    pub fn new(config_path: PathBuf) -> Api {
        Api {
            config_path: Some(config_path),
        }
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    fn config(&self) -> Result<Config> {
        let path = self.config_path.as_ref().ok_or(Error::NoConfigDir)?;
        match Config::load(path) {
            Err(Error::Io { ref source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok(Config::default())
            }
            result => result,
        }
    }

    pub fn backup_job(&self, name: &str) -> Result<Summary> {
        let config = self.config()?;
        let job = config.job(name)?;
        backup_dir(
            &job.source,
            &job.destination,
            &BackupOptions {
                compression: job.compression,
            },
        )
    }

    pub fn backup_path(&self, source: &Path, destination: &Path) -> Result<Summary> {
        backup_dir(source, destination, &BackupOptions::default())
    }

    pub fn restore(&self, backup: &Path, target: &Path) -> Result<Summary> {
        restore_dir(backup, target)
    }

    pub fn list_jobs(&self) -> Result<BTreeMap<String, Job>> {
        Ok(self.config()?.jobs)
    }

    pub fn verify_job(&self, name: &str) -> Result<VerifyReport> {
        let config = self.config()?;
        let job = config.job(name)?;
        verify_backup(&job.destination, Some(&job.source))
    }

    pub fn verify_path(&self, backup: &Path) -> Result<VerifyReport> {
        verify_backup(backup, None)
    }

    pub fn status(&self) -> Result<Vec<JobStatus>> {
        let config = self.config()?;
        Ok(config
            .jobs
            .into_iter()
            .map(|(name, job)| {
                let manifest_file = backup::manifest_path(&job.destination);
                let last_run = std::fs::metadata(&manifest_file)
                    .and_then(|m| m.modified())
                    .ok();
                let files_recorded = match last_run {
                    Some(_) => Manifest::load(&manifest_file)
                        .ok()
                        .map(|m| m.files.len() as u64),
                    None => None,
                };
                JobStatus {
                    destination_exists: job.destination.is_dir(),
                    last_run,
                    files_recorded,
                    name,
                    source: job.source,
                    destination: job.destination,
                    compression: job.compression,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch_api(test_name: &str) -> (PathBuf, Api) {
        let root = std::env::temp_dir().join(format!(
            "securesave-api-test-{}-{test_name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.txt"), b"hello").unwrap();

        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            format!(
                r#"
                [jobs.plain]
                source = "{root}/src"
                destination = "{root}/backup-plain"

                [jobs.packed]
                source = "{root}/src"
                destination = "{root}/backup-packed"
                compression = "zstd"
                "#,
                root = root.display()
            ),
        )
        .unwrap();
        (root, Api::new(config_path))
    }

    #[test]
    fn backup_job_resolves_configuration() {
        let (root, api) = scratch_api("backup-job");

        let summary = api.backup_job("packed").unwrap();

        assert_eq!(summary.files_copied, 1);
        assert!(root.join("backup-packed/a.txt.zst").is_file());
    }

    #[test]
    fn list_jobs_returns_them_sorted() {
        let (_, api) = scratch_api("list");
        let names: Vec<String> = api.list_jobs().unwrap().into_keys().collect();
        assert_eq!(names, ["packed", "plain"]);
    }

    #[test]
    fn verify_job_checks_plain_completeness() {
        let (root, api) = scratch_api("verify-plain");
        api.backup_job("plain").unwrap();
        fs::remove_file(root.join("backup-plain/a.txt")).unwrap();

        let report = api.verify_job("plain").unwrap();

        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].contains("missing from the backup"));
    }

    #[test]
    fn status_reports_last_run_from_the_manifest() {
        let (_, api) = scratch_api("status");
        api.backup_job("packed").unwrap();

        let statuses = api.status().unwrap();

        assert_eq!(statuses.len(), 2);
        let packed = &statuses[0];
        assert_eq!(packed.name, "packed");
        assert!(packed.destination_exists);
        assert!(packed.last_run.is_some());
        assert_eq!(packed.files_recorded, Some(1));

        let plain = &statuses[1];
        assert_eq!(plain.name, "plain");
        assert!(!plain.destination_exists);
        assert_eq!(plain.last_run, None);
        assert_eq!(plain.files_recorded, None);
    }

    #[test]
    fn a_missing_config_file_is_an_empty_config() {
        let api = Api::new(PathBuf::from("/does/not/exist/config.toml"));

        assert!(api.list_jobs().unwrap().is_empty());
        assert!(api.status().unwrap().is_empty());

        let err = api.backup_job("photos").unwrap_err();
        assert!(err.to_string().contains("defines no jobs"));
    }

    #[test]
    fn config_operations_fail_cleanly_without_a_config_location() {
        let api = Api { config_path: None };

        let err = api.backup_job("photos").unwrap_err();

        assert!(err.to_string().contains("HOME"));
    }
}
