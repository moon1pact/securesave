use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::backup::tmp_path;
use crate::error::{Error, Result};

const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub size: u64,
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
}

impl FileEntry {
    pub fn new(meta: &fs::Metadata) -> FileEntry {
        let (mtime_secs, mtime_nanos) = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| (d.as_secs(), d.subsec_nanos()))
            .unwrap_or((0, 0));
        FileEntry {
            size: meta.len(),
            mtime_secs,
            mtime_nanos,
        }
    }

    pub fn matches(&self, meta: &fs::Metadata) -> bool {
        *self == FileEntry::new(meta)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    #[serde(default)]
    pub files: BTreeMap<String, FileEntry>,
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest {
            version: VERSION,
            files: BTreeMap::new(),
        }
    }
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Manifest> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Manifest::default()),
            Err(e) => return Err(Error::io(path, e)),
        };
        let manifest: Manifest = serde_json::from_str(&text).map_err(|e| Error::ManifestParse {
            path: path.to_path_buf(),
            source: e,
        })?;
        if manifest.version != VERSION {
            return Err(Error::ManifestVersion {
                path: path.to_path_buf(),
                found: manifest.version,
            });
        }
        Ok(manifest)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let tmp = tmp_path(path);
        let result = (|| {
            let json =
                serde_json::to_string_pretty(self).expect("manifest serialization cannot fail");
            let mut file = fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            file.write_all(json.as_bytes())
                .map_err(|e| Error::io(&tmp, e))?;
            file.sync_all().map_err(|e| Error::io(&tmp, e))?;
            fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch_dir(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "securesave-manifest-test-{}-{test_name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn saves_and_reloads_identically() {
        let dir = scratch_dir("roundtrip");
        let path = dir.join("sub").join("manifest.json");

        let mut manifest = Manifest::default();
        manifest.files.insert(
            "photos/a.jpg".to_string(),
            FileEntry {
                size: 42,
                mtime_secs: 1_000_000_000,
                mtime_nanos: 123,
            },
        );

        manifest.save(&path).unwrap();
        assert_eq!(Manifest::load(&path).unwrap(), manifest);
    }

    #[test]
    fn a_missing_file_is_an_empty_manifest() {
        let dir = scratch_dir("missing");
        let manifest = Manifest::load(&dir.join("does-not-exist.json")).unwrap();
        assert!(manifest.files.is_empty());
    }

    #[test]
    fn corrupt_json_is_a_loud_error() {
        let dir = scratch_dir("corrupt");
        let path = dir.join("manifest.json");
        fs::write(&path, b"{ not json").unwrap();

        let message = Manifest::load(&path).unwrap_err().to_string();
        assert!(message.contains("invalid manifest"));
    }

    #[test]
    fn an_unknown_version_is_rejected() {
        let dir = scratch_dir("version");
        let path = dir.join("manifest.json");
        fs::write(&path, br#"{ "version": 99, "files": {} }"#).unwrap();

        let message = Manifest::load(&path).unwrap_err().to_string();
        assert!(message.contains("version 99"));
    }
}
