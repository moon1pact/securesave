use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use crate::backup::{self, BackupOptions, Summary};
use crate::error::{Error, Result};
use crate::manifest::{FileEntry, Manifest};

pub fn restore_dir(backup: &Path, target: &Path) -> Result<Summary> {
    ensure_target_usable(target)?;

    let manifest_file = backup::manifest_path(backup);
    if manifest_file.is_file() {
        let manifest = Manifest::load(&manifest_file)?;
        restore_compressed(backup, target, &manifest)
    } else {
        backup::backup_dir(backup, target, &BackupOptions::default())
    }
}

fn ensure_target_usable(target: &Path) -> Result<()> {
    match fs::read_dir(target) {
        Ok(mut entries) => match entries.next() {
            Some(_) => Err(Error::TargetNotEmpty {
                path: target.to_path_buf(),
            }),
            None => Ok(()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(target, e)),
    }
}

fn restore_compressed(backup: &Path, target: &Path, manifest: &Manifest) -> Result<Summary> {
    let mut summary = Summary::default();
    let mut restored = BTreeSet::new();

    restore_tree(
        backup,
        target,
        Path::new(""),
        manifest,
        &mut summary,
        &mut restored,
    )?;

    if let Some(missing) = manifest.files.keys().find(|key| !restored.contains(*key)) {
        return Err(Error::BackupInconsistent {
            path: backup.to_path_buf(),
            reason: format!("'{missing}.zst' is listed in the manifest but missing"),
        });
    }
    Ok(summary)
}

fn restore_tree(
    src: &Path,
    dest: &Path,
    rel: &Path,
    manifest: &Manifest,
    summary: &mut Summary,
    restored: &mut BTreeSet<String>,
) -> Result<()> {
    if !dest.exists() {
        fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;
        summary.dirs_created += 1;
    }

    let entries = fs::read_dir(src).map_err(|e| Error::io(src, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(src, e))?;
        let name = entry.file_name();
        let src_path = entry.path();
        let file_type = entry.file_type().map_err(|e| Error::io(&src_path, e))?;

        if rel.as_os_str().is_empty() && name == ".securesave" {
            continue;
        }

        if file_type.is_dir() {
            restore_tree(
                &src_path,
                &dest.join(&name),
                &rel.join(&name),
                manifest,
                summary,
                restored,
            )?;
        } else if file_type.is_symlink() {
            let link_target = fs::read_link(&src_path).map_err(|e| Error::io(&src_path, e))?;
            backup::create_symlink(&link_target, &dest.join(&name))?;
            summary.symlinks_recreated += 1;
        } else if file_type.is_file() {
            let stem = name.to_str().and_then(|n| n.strip_suffix(".zst"));
            let known = stem.and_then(|stem| {
                let key = rel.join(stem).to_str()?.to_string();
                let recorded = manifest.files.get(&key)?;
                Some((key, recorded))
            });
            match known {
                Some((key, recorded)) => {
                    let dest_file = dest.join(stem.expect("known implies a .zst stem"));
                    let bytes = restore_file_zstd(&src_path, &dest_file, recorded)?;
                    summary.files_copied += 1;
                    summary.bytes_copied += bytes;
                    summary.bytes_written += bytes;
                    restored.insert(key);
                }
                None => {
                    summary.warnings.push(format!(
                        "{}: not in the manifest, skipped",
                        src_path.display()
                    ));
                    summary.entries_skipped += 1;
                }
            }
        } else {
            summary.warnings.push(format!(
                "{}: unexpected special file, skipped",
                src_path.display()
            ));
            summary.entries_skipped += 1;
        }
    }
    Ok(())
}

fn restore_file_zstd(src_zst: &Path, dest: &Path, recorded: &FileEntry) -> Result<u64> {
    let tmp = backup::tmp_path(dest);
    let result = (|| {
        let src_file = fs::File::open(src_zst).map_err(|e| Error::io(src_zst, e))?;
        let src_meta = src_file.metadata().map_err(|e| Error::io(src_zst, e))?;
        let mut decoder =
            zstd::stream::read::Decoder::new(src_file).map_err(|e| Error::io(src_zst, e))?;
        let mut tmp_file = fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let bytes = io::copy(&mut decoder, &mut tmp_file).map_err(|e| Error::io(src_zst, e))?;

        if bytes != recorded.size {
            return Err(Error::BackupInconsistent {
                path: src_zst.to_path_buf(),
                reason: format!(
                    "decompressed to {bytes} bytes but the manifest records {}",
                    recorded.size
                ),
            });
        }

        tmp_file
            .set_permissions(src_meta.permissions())
            .map_err(|e| Error::io(&tmp, e))?;
        let mtime = UNIX_EPOCH + Duration::new(recorded.mtime_secs, recorded.mtime_nanos);
        tmp_file
            .set_times(fs::FileTimes::new().set_modified(mtime))
            .map_err(|e| Error::io(&tmp, e))?;
        tmp_file.sync_all().map_err(|e| Error::io(&tmp, e))?;
        fs::rename(&tmp, dest).map_err(|e| Error::io(dest, e))?;
        Ok(bytes)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{Compression, backup_dir};
    use std::path::PathBuf;

    fn scratch_dir(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "securesave-restore-test-{}-{test_name}",
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
    fn refuses_a_non_empty_target() {
        let root = scratch_dir("non-empty-target");
        let (_, backup) = make_backup(&root, Compression::None);
        let target = root.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("precious.txt"), b"do not touch").unwrap();

        let err = restore_dir(&backup, &target).unwrap_err();

        assert!(err.to_string().contains("non-empty"));
        assert_eq!(
            fs::read(target.join("precious.txt")).unwrap(),
            b"do not touch"
        );
    }

    #[test]
    fn accepts_an_existing_empty_directory() {
        let root = scratch_dir("empty-target");
        let (_, backup) = make_backup(&root, Compression::None);
        let target = root.join("target");
        fs::create_dir_all(&target).unwrap();

        let summary = restore_dir(&backup, &target).unwrap();
        assert_eq!(summary.files_copied, 2);
    }

    #[test]
    fn restores_a_plain_backup() {
        let root = scratch_dir("plain");
        let (_, backup) = make_backup(&root, Compression::None);
        let target = root.join("target");

        let summary = restore_dir(&backup, &target).unwrap();

        assert_eq!(summary.files_copied, 2);
        assert_eq!(summary.symlinks_recreated, 1);
        assert_eq!(
            fs::read(target.join("a.txt")).unwrap(),
            b"hello hello hello"
        );
        assert_eq!(fs::read(target.join("sub/b.txt")).unwrap(), b"world");
        assert_eq!(
            fs::read_link(target.join("link.txt")).unwrap(),
            PathBuf::from("a.txt")
        );
    }

    #[test]
    fn restores_a_compressed_backup_with_mtimes() {
        let root = scratch_dir("compressed");
        let (src, backup) = make_backup(&root, Compression::Zstd);
        let target = root.join("target");

        let summary = restore_dir(&backup, &target).unwrap();

        assert_eq!(summary.files_copied, 2);
        assert_eq!(summary.symlinks_recreated, 1);
        assert!(summary.warnings.is_empty());
        assert_eq!(
            fs::read(target.join("a.txt")).unwrap(),
            b"hello hello hello"
        );
        assert_eq!(fs::read(target.join("sub/b.txt")).unwrap(), b"world");
        assert!(!target.join("a.txt.zst").exists());
        assert!(!target.join(".securesave").exists());
        assert_eq!(
            fs::metadata(target.join("a.txt"))
                .unwrap()
                .modified()
                .unwrap(),
            fs::metadata(src.join("a.txt")).unwrap().modified().unwrap()
        );
    }

    #[test]
    fn a_missing_compressed_file_is_a_blocking_error() {
        let root = scratch_dir("missing-zst");
        let (_, backup) = make_backup(&root, Compression::Zstd);
        fs::remove_file(backup.join("a.txt.zst")).unwrap();

        let err = restore_dir(&backup, &root.join("target")).unwrap_err();

        assert!(err.to_string().contains("a.txt.zst"));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn a_size_mismatch_is_a_blocking_error() {
        let root = scratch_dir("size-mismatch");
        let (_, backup) = make_backup(&root, Compression::Zstd);
        let bogus = zstd::stream::encode_all(&b"wrong length"[..], 0).unwrap();
        fs::write(backup.join("a.txt.zst"), bogus).unwrap();

        let err = restore_dir(&backup, &root.join("target")).unwrap_err();

        assert!(err.to_string().contains("manifest records"));
    }

    #[test]
    fn stray_files_are_skipped_with_a_warning() {
        let root = scratch_dir("stray");
        let (_, backup) = make_backup(&root, Compression::Zstd);
        fs::write(backup.join("stray.txt"), b"dropped by hand").unwrap();
        let stray_zst = zstd::stream::encode_all(&b"data"[..], 0).unwrap();
        fs::write(backup.join("stray.zst"), stray_zst).unwrap();
        let target = root.join("target");

        let summary = restore_dir(&backup, &target).unwrap();

        assert_eq!(summary.files_copied, 2);
        assert_eq!(summary.warnings.len(), 2);
        assert_eq!(summary.entries_skipped, 2);
        assert!(!target.join("stray.txt").exists());
        assert!(!target.join("stray").exists());
        assert!(!target.join("stray.zst").exists());
    }
}
