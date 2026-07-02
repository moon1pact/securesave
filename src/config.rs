use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::backup::Compression;
use crate::error::{Error, Result};

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub source: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub compression: Compression,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub jobs: BTreeMap<String, Job>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        toml::from_str(&text).map_err(|e| Error::ConfigParse {
            path: path.to_path_buf(),
            source: e,
        })
    }

    pub fn default_path() -> Option<PathBuf> {
        default_path_from(env::var_os("XDG_CONFIG_HOME"), env::var_os("HOME"))
    }

    pub fn job(&self, name: &str) -> Result<&Job> {
        self.jobs.get(name).ok_or_else(|| Error::JobNotFound {
            name: name.to_string(),
            available: self.jobs.keys().cloned().collect(),
        })
    }
}

fn default_path_from(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = match xdg_config_home {
        Some(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
        _ => PathBuf::from(home?).join(".config"),
    };
    Some(base.join("securesave").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config> {
        toml::from_str(text).map_err(|e| Error::ConfigParse {
            path: PathBuf::from("test.toml"),
            source: e,
        })
    }

    #[test]
    fn parses_a_config_with_jobs() {
        let config = parse(
            r#"
            [jobs.photos]
            source = "/home/moon/Photos"
            destination = "/mnt/backup/photos"
            "#,
        )
        .unwrap();

        let job = config.job("photos").unwrap();
        assert_eq!(job.source, PathBuf::from("/home/moon/Photos"));
        assert_eq!(job.destination, PathBuf::from("/mnt/backup/photos"));
    }

    #[test]
    fn compression_defaults_to_none_for_existing_configs() {
        let config = parse(
            r#"
            [jobs.photos]
            source = "/a"
            destination = "/b"
            "#,
        )
        .unwrap();

        assert_eq!(config.job("photos").unwrap().compression, Compression::None);
    }

    #[test]
    fn parses_zstd_compression() {
        let config = parse(
            r#"
            [jobs.photos]
            source = "/a"
            destination = "/b"
            compression = "zstd"
            "#,
        )
        .unwrap();

        assert_eq!(config.job("photos").unwrap().compression, Compression::Zstd);
    }

    #[test]
    fn rejects_an_unknown_compression_value() {
        let err = parse(
            r#"
            [jobs.photos]
            source = "/a"
            destination = "/b"
            compression = "brotli"
            "#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("brotli"));
    }

    #[test]
    fn an_empty_file_is_a_valid_config() {
        let config = parse("").unwrap();
        assert!(config.jobs.is_empty());
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = parse(
            r#"
            [jobs.photos]
            sorce = "/home/moon/Photos"
            destination = "/mnt/backup/photos"
            "#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("sorce"));
    }

    #[test]
    fn unknown_job_error_lists_available_jobs() {
        let config = parse(
            r#"
            [jobs.photos]
            source = "/a"
            destination = "/b"

            [jobs.documents]
            source = "/c"
            destination = "/d"
            "#,
        )
        .unwrap();

        let message = config.job("videos").unwrap_err().to_string();
        assert!(message.contains("videos"));
        assert!(message.contains("documents, photos"));
    }

    #[test]
    fn xdg_config_home_takes_precedence_when_absolute() {
        let path = default_path_from(Some("/custom/config".into()), Some("/home/moon".into()));
        assert_eq!(
            path,
            Some(PathBuf::from("/custom/config/securesave/config.toml"))
        );
    }

    #[test]
    fn relative_or_missing_xdg_config_home_falls_back_to_home() {
        for xdg in [None, Some(OsString::from("")), Some(OsString::from("rel"))] {
            let path = default_path_from(xdg, Some("/home/moon".into()));
            assert_eq!(
                path,
                Some(PathBuf::from("/home/moon/.config/securesave/config.toml"))
            );
        }
    }

    #[test]
    fn no_home_at_all_yields_none() {
        assert_eq!(default_path_from(None, None), None);
    }
}
