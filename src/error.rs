use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },
    JobNotFound {
        name: String,
        available: Vec<String>,
    },
    ManifestParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    ManifestVersion {
        path: PathBuf,
        found: u32,
    },
    NonUtf8Path {
        path: PathBuf,
    },
    NoConfigDir,
    TargetNotEmpty {
        path: PathBuf,
    },
    BackupInconsistent {
        path: PathBuf,
        reason: String,
    },
}

impl Error {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => {
                write!(f, "{}: {}", path.display(), source)
            }
            Error::ConfigParse { path, source } => {
                write!(f, "{}: invalid configuration:\n{}", path.display(), source)
            }
            Error::JobNotFound { name, available } => {
                if available.is_empty() {
                    write!(
                        f,
                        "job '{name}' not found: the configuration defines no jobs"
                    )
                } else {
                    write!(
                        f,
                        "job '{name}' not found (available jobs: {})",
                        available.join(", ")
                    )
                }
            }
            Error::ManifestParse { path, source } => {
                write!(
                    f,
                    "{}: invalid manifest: {source} (delete the file to force a full backup)",
                    path.display()
                )
            }
            Error::ManifestVersion { path, found } => {
                write!(
                    f,
                    "{}: manifest version {found} is not supported by this version of securesave",
                    path.display()
                )
            }
            Error::NonUtf8Path { path } => {
                write!(
                    f,
                    "{}: file name is not valid UTF-8 (required for compressed backups)",
                    path.display()
                )
            }
            Error::NoConfigDir => {
                write!(f, "cannot locate the configuration file (HOME is not set)")
            }
            Error::TargetNotEmpty { path } => {
                write!(
                    f,
                    "{}: refusing to restore into a non-empty directory",
                    path.display()
                )
            }
            Error::BackupInconsistent { path, reason } => {
                write!(f, "{}: inconsistent backup: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::ConfigParse { source, .. } => Some(source),
            Error::ManifestParse { source, .. } => Some(source),
            Error::JobNotFound { .. }
            | Error::ManifestVersion { .. }
            | Error::NonUtf8Path { .. }
            | Error::NoConfigDir
            | Error::TargetNotEmpty { .. }
            | Error::BackupInconsistent { .. } => None,
        }
    }
}
