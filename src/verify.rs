use std::fs;
use std::io;
use std::path::Path;

use crate::backup;
use crate::error::Result;
use crate::manifest::Manifest;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub files_checked: u64,
    pub bytes_checked: u64,
    pub issues: Vec<String>,
}

impl VerifyReport {
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }

    fn issue(&mut self, path: &Path, what: impl std::fmt::Display) {
        self.issues.push(format!("{}: {what}", path.display()));
    }
}

pub fn verify_backup(backup: &Path, source: Option<&Path>) -> Result<VerifyReport> {
    let manifest_file = backup::manifest_path(backup);
    if manifest_file.is_file() {
        let manifest = Manifest::load(&manifest_file)?;
        Ok(verify_compressed(backup, &manifest))
    } else {
        Ok(verify_plain(backup, source))
    }
}

fn verify_compressed(backup: &Path, manifest: &Manifest) -> VerifyReport {
    let mut report = VerifyReport::default();
    let mut seen = std::collections::BTreeSet::new();
    walk_compressed(backup, Path::new(""), manifest, &mut report, &mut seen);

    for key in manifest.files.keys() {
        if !seen.contains(key) {
            report
                .issues
                .push(format!("{key}.zst: listed in the manifest but missing"));
        }
    }
    report
}

fn walk_compressed(
    dir: &Path,
    rel: &Path,
    manifest: &Manifest,
    report: &mut VerifyReport,
    seen: &mut std::collections::BTreeSet<String>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return report.issue(dir, format_args!("cannot read directory: {e}")),
    };
    for entry in entries {
        let Ok(entry) = entry else {
            report.issue(dir, "cannot read a directory entry");
            continue;
        };
        let name = entry.file_name();
        let path = entry.path();
        if rel.as_os_str().is_empty() && name == ".securesave" {
            continue;
        }
        if is_leftover_tmp(&name) {
            report.issue(&path, "leftover temporary file from an interrupted run");
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            report.issue(&path, "cannot read file type");
            continue;
        };

        if file_type.is_dir() {
            walk_compressed(&path, &rel.join(&name), manifest, report, seen);
        } else if file_type.is_symlink() {
            if fs::read_link(&path).is_err() {
                report.issue(&path, "unreadable symlink");
            }
        } else if file_type.is_file() {
            let known = name
                .to_str()
                .and_then(|n| n.strip_suffix(".zst"))
                .and_then(|stem| rel.join(stem).to_str().map(str::to_string))
                .and_then(|key| manifest.files.get(&key).map(|entry| (key, entry)));
            match known {
                Some((key, recorded)) => {
                    check_zst_content(&path, recorded.size, report);
                    seen.insert(key);
                }
                None => report.issue(&path, "not in the manifest"),
            }
        } else {
            report.issue(&path, "unexpected special file");
        }
    }
}

fn check_zst_content(path: &Path, recorded_size: u64, report: &mut VerifyReport) {
    let decoded = fs::File::open(path)
        .and_then(zstd::stream::read::Decoder::new)
        .and_then(|mut decoder| io::copy(&mut decoder, &mut io::sink()));
    match decoded {
        Ok(bytes) if bytes == recorded_size => {
            report.files_checked += 1;
            report.bytes_checked += bytes;
        }
        Ok(bytes) => report.issue(
            path,
            format_args!("decompresses to {bytes} bytes but the manifest records {recorded_size}"),
        ),
        Err(e) => report.issue(path, format_args!("cannot decompress: {e}")),
    }
}

fn verify_plain(backup: &Path, source: Option<&Path>) -> VerifyReport {
    let mut report = VerifyReport::default();
    walk_plain(backup, &mut report);
    if let Some(source) = source {
        check_source_covered(source, backup, &mut report);
    }
    report
}

fn walk_plain(dir: &Path, report: &mut VerifyReport) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return report.issue(dir, format_args!("cannot read directory: {e}")),
    };
    for entry in entries {
        let Ok(entry) = entry else {
            report.issue(dir, "cannot read a directory entry");
            continue;
        };
        let path = entry.path();
        if is_leftover_tmp(&entry.file_name()) {
            report.issue(&path, "leftover temporary file from an interrupted run");
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            report.issue(&path, "cannot read file type");
            continue;
        };

        if file_type.is_dir() {
            walk_plain(&path, report);
        } else if file_type.is_symlink() {
            if fs::read_link(&path).is_err() {
                report.issue(&path, "unreadable symlink");
            }
        } else if file_type.is_file() {
            let read =
                fs::File::open(&path).and_then(|mut file| io::copy(&mut file, &mut io::sink()));
            match read {
                Ok(bytes) => {
                    report.files_checked += 1;
                    report.bytes_checked += bytes;
                }
                Err(e) => report.issue(&path, format_args!("cannot read: {e}")),
            }
        } else {
            report.issue(&path, "unexpected special file");
        }
    }
}

fn check_source_covered(source: &Path, backup: &Path, report: &mut VerifyReport) {
    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(e) => return report.issue(source, format_args!("cannot read source directory: {e}")),
    };
    for entry in entries {
        let Ok(entry) = entry else {
            report.issue(source, "cannot read a source directory entry");
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let in_backup = backup.join(entry.file_name());
        let backup_type = in_backup.symlink_metadata().map(|m| m.file_type());

        if file_type.is_dir() {
            match backup_type {
                Ok(t) if t.is_dir() => check_source_covered(&entry.path(), &in_backup, report),
                _ => report.issue(&in_backup, "directory missing from the backup"),
            }
        } else if file_type.is_file() {
            if !backup_type.is_ok_and(|t| t.is_file()) {
                report.issue(&in_backup, "file missing from the backup");
            }
        } else if file_type.is_symlink() && !backup_type.is_ok_and(|t| t.is_symlink()) {
            report.issue(&in_backup, "symlink missing from the backup");
        }
    }
}

fn is_leftover_tmp(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with(".securesave-tmp.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{BackupOptions, Compression, backup_dir};
    use std::path::PathBuf;

    fn scratch_dir(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "securesave-verify-test-{}-{test_name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_backup(root: &Path, compression: Compression) -> (PathBuf, PathBuf) {
        let src = root.join("src");
        let backup = root.join("backup");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"hello hello hello").unwrap();
        fs::write(src.join("sub/b.txt"), b"world").unwrap();
        std::os::unix::fs::symlink("a.txt", src.join("link.txt")).unwrap();
        backup_dir(&src, &backup, &BackupOptions { compression }).unwrap();
        (src, backup)
    }

    #[test]
    fn a_sound_compressed_backup_verifies_cleanly() {
        let root = scratch_dir("zstd-clean");
        let (_, backup) = make_backup(&root, Compression::Zstd);

        let report = verify_backup(&backup, None).unwrap();

        assert!(report.is_ok(), "unexpected issues: {:?}", report.issues);
        assert_eq!(report.files_checked, 2);
        assert_eq!(report.bytes_checked, 22);
    }

    #[test]
    fn corrupted_zst_content_is_reported() {
        let root = scratch_dir("zstd-corrupt");
        let (_, backup) = make_backup(&root, Compression::Zstd);
        fs::write(backup.join("a.txt.zst"), b"this is not zstd data").unwrap();

        let report = verify_backup(&backup, None).unwrap();

        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].contains("a.txt.zst"));
        assert!(report.issues[0].contains("cannot decompress"));
    }

    #[test]
    fn wrong_decompressed_size_is_reported() {
        let root = scratch_dir("zstd-size");
        let (_, backup) = make_backup(&root, Compression::Zstd);
        let bogus = zstd::stream::encode_all(&b"wrong length"[..], 0).unwrap();
        fs::write(backup.join("a.txt.zst"), bogus).unwrap();

        let report = verify_backup(&backup, None).unwrap();

        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].contains("manifest records"));
    }

    #[test]
    fn missing_and_stray_files_are_reported_together() {
        let root = scratch_dir("zstd-missing-stray");
        let (_, backup) = make_backup(&root, Compression::Zstd);
        fs::remove_file(backup.join("a.txt.zst")).unwrap();
        fs::write(backup.join("stray.txt"), b"dropped by hand").unwrap();
        fs::write(backup.join(".securesave-tmp.x"), b"leftover").unwrap();

        let report = verify_backup(&backup, None).unwrap();

        assert_eq!(report.issues.len(), 3);
        let all = report.issues.join("\n");
        assert!(all.contains("listed in the manifest but missing"));
        assert!(all.contains("not in the manifest"));
        assert!(all.contains("leftover temporary file"));
    }

    #[test]
    fn a_sound_plain_backup_verifies_cleanly() {
        let root = scratch_dir("plain-clean");
        let (src, backup) = make_backup(&root, Compression::None);

        let report = verify_backup(&backup, Some(&src)).unwrap();

        assert!(report.is_ok(), "unexpected issues: {:?}", report.issues);
        assert_eq!(report.files_checked, 2);
        assert_eq!(report.bytes_checked, 22);
    }

    #[test]
    fn plain_verification_detects_files_missing_from_the_backup() {
        let root = scratch_dir("plain-missing");
        let (src, backup) = make_backup(&root, Compression::None);
        fs::remove_file(backup.join("sub/b.txt")).unwrap();
        fs::remove_file(backup.join("link.txt")).unwrap();

        let report = verify_backup(&backup, Some(&src)).unwrap();

        assert_eq!(report.issues.len(), 2);
        let all = report.issues.join("\n");
        assert!(all.contains("file missing from the backup"));
        assert!(all.contains("symlink missing from the backup"));
    }

    #[test]
    fn plain_leftover_temporaries_are_reported() {
        let root = scratch_dir("plain-tmp");
        let (src, backup) = make_backup(&root, Compression::None);
        fs::write(backup.join(".securesave-tmp.a.txt"), b"partial").unwrap();

        let report = verify_backup(&backup, Some(&src)).unwrap();

        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].contains("leftover temporary file"));
    }
}
