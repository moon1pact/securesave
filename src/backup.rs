use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::manifest::{FileEntry, Manifest};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    #[default]
    None,
    Zstd,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackupOptions {
    pub compression: Compression,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub files_copied: u64,
    pub bytes_copied: u64,
    pub bytes_written: u64,
    pub files_unchanged: u64,
    pub dirs_created: u64,
    pub symlinks_recreated: u64,
    pub symlinks_unchanged: u64,
    pub entries_skipped: u64,
    pub warnings: Vec<String>,
}

struct BackupRun {
    compression: Compression,
    old_manifest: Manifest,
    new_manifest: Manifest,
    summary: Summary,
}

pub fn backup_dir(source: &Path, destination: &Path, options: &BackupOptions) -> Result<Summary> {
    let mut run = BackupRun {
        compression: options.compression,
        old_manifest: match options.compression {
            Compression::None => Manifest::default(),
            Compression::Zstd => Manifest::load(&manifest_path(destination))?,
        },
        new_manifest: Manifest::default(),
        summary: Summary::default(),
    };

    copy_tree(source, destination, Path::new(""), &mut run)?;

    if options.compression == Compression::Zstd {
        run.new_manifest.save(&manifest_path(destination))?;
    }
    Ok(run.summary)
}

pub(crate) fn manifest_path(destination: &Path) -> PathBuf {
    destination.join(".securesave").join("manifest.json")
}

fn copy_tree(src: &Path, dest: &Path, rel: &Path, run: &mut BackupRun) -> Result<()> {
    if !dest.exists() {
        fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;
        run.summary.dirs_created += 1;
    }

    let entries = fs::read_dir(src).map_err(|e| Error::io(src, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(src, e))?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let rel_path = rel.join(entry.file_name());
        let file_type = entry.file_type().map_err(|e| Error::io(&src_path, e))?;

        if run.compression == Compression::Zstd
            && rel.as_os_str().is_empty()
            && entry.file_name() == ".securesave"
        {
            run.summary.entries_skipped += 1;
            continue;
        }

        if file_type.is_dir() {
            copy_tree(&src_path, &dest_path, &rel_path, run)?;
        } else if file_type.is_file() {
            let src_meta = entry.metadata().map_err(|e| Error::io(&src_path, e))?;
            match run.compression {
                Compression::None => {
                    if is_up_to_date(&src_meta, &dest_path) {
                        run.summary.files_unchanged += 1;
                    } else {
                        let bytes = copy_file(&src_path, &src_meta, &dest_path)?;
                        run.summary.bytes_copied += bytes;
                        run.summary.bytes_written += bytes;
                        run.summary.files_copied += 1;
                    }
                }
                Compression::Zstd => {
                    let key = rel_path
                        .to_str()
                        .ok_or_else(|| Error::NonUtf8Path {
                            path: src_path.clone(),
                        })?
                        .to_string();
                    let dest_zst = zst_path(&dest_path);
                    if is_up_to_date_zstd(&src_meta, run.old_manifest.files.get(&key), &dest_zst) {
                        run.summary.files_unchanged += 1;
                    } else {
                        let (read, written) = copy_file_zstd(&src_path, &src_meta, &dest_zst)?;
                        run.summary.bytes_copied += read;
                        run.summary.bytes_written += written;
                        run.summary.files_copied += 1;
                    }
                    run.new_manifest
                        .files
                        .insert(key, FileEntry::new(&src_meta));
                }
            }
        } else if file_type.is_symlink() {
            let target = fs::read_link(&src_path).map_err(|e| Error::io(&src_path, e))?;
            if fs::read_link(&dest_path).is_ok_and(|existing| existing == target) {
                run.summary.symlinks_unchanged += 1;
            } else {
                create_symlink(&target, &dest_path)?;
                run.summary.symlinks_recreated += 1;
            }
        } else {
            run.summary.entries_skipped += 1;
        }
    }
    Ok(())
}

fn is_up_to_date(src_meta: &fs::Metadata, dest: &Path) -> bool {
    let Ok(dest_meta) = dest.symlink_metadata() else {
        return false;
    };
    if !dest_meta.is_file() {
        return false;
    }
    let (Ok(src_mtime), Ok(dest_mtime)) = (src_meta.modified(), dest_meta.modified()) else {
        return false;
    };
    src_meta.len() == dest_meta.len() && src_mtime == dest_mtime
}

fn copy_file(src: &Path, src_meta: &fs::Metadata, dest: &Path) -> Result<u64> {
    let tmp = tmp_path(dest);
    let result = (|| {
        let bytes = fs::copy(src, &tmp).map_err(|e| Error::io(src, e))?;
        let file = fs::File::open(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let times = fs::FileTimes::new()
            .set_accessed(src_meta.accessed().map_err(|e| Error::io(src, e))?)
            .set_modified(src_meta.modified().map_err(|e| Error::io(src, e))?);
        file.set_times(times).map_err(|e| Error::io(&tmp, e))?;
        file.sync_all().map_err(|e| Error::io(&tmp, e))?;
        fs::rename(&tmp, dest).map_err(|e| Error::io(dest, e))?;
        Ok(bytes)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn is_up_to_date_zstd(src_meta: &fs::Metadata, entry: Option<&FileEntry>, dest_zst: &Path) -> bool {
    entry.is_some_and(|e| e.matches(src_meta))
        && dest_zst.symlink_metadata().is_ok_and(|m| m.is_file())
}

fn copy_file_zstd(src: &Path, src_meta: &fs::Metadata, dest: &Path) -> Result<(u64, u64)> {
    let tmp = tmp_path(dest);
    let result = (|| {
        let mut reader = fs::File::open(src).map_err(|e| Error::io(src, e))?;
        let tmp_file = fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let mut encoder =
            zstd::stream::write::Encoder::new(tmp_file, zstd::DEFAULT_COMPRESSION_LEVEL)
                .map_err(|e| Error::io(&tmp, e))?;
        let bytes_read = io::copy(&mut reader, &mut encoder).map_err(|e| Error::io(src, e))?;
        let file = encoder.finish().map_err(|e| Error::io(&tmp, e))?;

        file.set_permissions(src_meta.permissions())
            .map_err(|e| Error::io(&tmp, e))?;
        let times = fs::FileTimes::new()
            .set_accessed(src_meta.accessed().map_err(|e| Error::io(src, e))?)
            .set_modified(src_meta.modified().map_err(|e| Error::io(src, e))?);
        file.set_times(times).map_err(|e| Error::io(&tmp, e))?;
        file.sync_all().map_err(|e| Error::io(&tmp, e))?;
        let bytes_written = file.metadata().map_err(|e| Error::io(&tmp, e))?.len();
        fs::rename(&tmp, dest).map_err(|e| Error::io(dest, e))?;
        Ok((bytes_read, bytes_written))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn zst_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".zst");
    dest.with_file_name(name)
}

pub(crate) fn tmp_path(dest: &Path) -> PathBuf {
    let mut name = OsString::from(".securesave-tmp.");
    if let Some(file_name) = dest.file_name() {
        name.push(file_name);
    }
    dest.with_file_name(name)
}

pub(crate) fn create_symlink(target: &Path, dest: &Path) -> Result<()> {
    let tmp = tmp_path(dest);
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).map_err(|e| Error::io(&tmp, e))?;
    fs::rename(&tmp, dest).map_err(|e| Error::io(dest, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "securesave-test-{}-{test_name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_mtime(path: &Path, mtime: std::time::SystemTime) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(mtime))
            .unwrap();
    }

    #[test]
    fn backs_up_a_nested_tree() {
        let root = scratch_dir("nested-tree");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(src.join("sub/deeper")).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::write(src.join("sub/b.txt"), b"world!").unwrap();
        fs::write(src.join("sub/deeper/c.txt"), b"").unwrap();

        let summary = backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        assert_eq!(summary.files_copied, 3);
        assert_eq!(summary.bytes_copied, 11);
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(dest.join("sub/b.txt")).unwrap(), b"world!");
        assert_eq!(fs::read(dest.join("sub/deeper/c.txt")).unwrap(), b"");
    }

    #[test]
    fn recreates_symlinks_without_following_them() {
        let root = scratch_dir("symlinks");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("real.txt"), b"data").unwrap();
        std::os::unix::fs::symlink("real.txt", src.join("link.txt")).unwrap();

        let summary = backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        assert_eq!(summary.symlinks_recreated, 1);
        let copied = dest.join("link.txt");
        assert!(copied.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&copied).unwrap(), PathBuf::from("real.txt"));
    }

    #[test]
    fn preserves_file_mtimes() {
        let root = scratch_dir("mtimes");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        let file_path = src.join("old.txt");
        fs::write(&file_path, b"data").unwrap();

        let old_mtime =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        set_mtime(&file_path, old_mtime);

        backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        let copied_mtime = fs::metadata(dest.join("old.txt"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(copied_mtime, old_mtime);
    }

    #[test]
    fn second_run_skips_unchanged_files() {
        let root = scratch_dir("skip-unchanged");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();

        backup_dir(&src, &dest, &BackupOptions::default()).unwrap();
        let second = backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        assert_eq!(second.files_copied, 0);
        assert_eq!(second.files_unchanged, 1);
        assert_eq!(second.bytes_copied, 0);
    }

    #[test]
    fn recopies_when_size_differs() {
        let root = scratch_dir("size-differs");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        let src_file = src.join("a.txt");
        fs::write(&src_file, b"hello").unwrap();
        backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        let mtime = fs::metadata(&src_file).unwrap().modified().unwrap();
        fs::write(&src_file, b"hello, world").unwrap();
        set_mtime(&src_file, mtime);

        let second = backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        assert_eq!(second.files_copied, 1);
        assert_eq!(second.files_unchanged, 0);
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"hello, world");
    }

    #[test]
    fn recopies_when_mtime_differs() {
        let root = scratch_dir("mtime-differs");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        let src_file = src.join("a.txt");
        fs::write(&src_file, b"hello").unwrap();
        backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        fs::write(&src_file, b"world").unwrap();
        set_mtime(
            &src_file,
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000_000),
        );

        let second = backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        assert_eq!(second.files_copied, 1);
        assert_eq!(second.files_unchanged, 0);
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"world");
    }

    #[test]
    fn copies_files_missing_from_the_destination() {
        let root = scratch_dir("missing-in-dest");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        backup_dir(&src, &dest, &BackupOptions::default()).unwrap();
        fs::remove_file(dest.join("a.txt")).unwrap();

        let second = backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        assert_eq!(second.files_copied, 1);
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"hello");
    }

    #[test]
    fn second_run_skips_unchanged_symlinks() {
        let root = scratch_dir("skip-symlinks");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        std::os::unix::fs::symlink("a.txt", src.join("link.txt")).unwrap();

        backup_dir(&src, &dest, &BackupOptions::default()).unwrap();
        let second = backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        assert_eq!(second.symlinks_recreated, 0);
        assert_eq!(second.symlinks_unchanged, 1);
    }

    #[test]
    fn recreates_symlinks_whose_target_changed() {
        let root = scratch_dir("symlink-target-changed");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::write(src.join("b.txt"), b"world").unwrap();
        let src_link = src.join("link.txt");
        std::os::unix::fs::symlink("a.txt", &src_link).unwrap();
        backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        fs::remove_file(&src_link).unwrap();
        std::os::unix::fs::symlink("b.txt", &src_link).unwrap();

        let second = backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        assert_eq!(second.symlinks_recreated, 1);
        assert_eq!(second.symlinks_unchanged, 0);
        assert_eq!(
            fs::read_link(dest.join("link.txt")).unwrap(),
            PathBuf::from("b.txt")
        );
    }

    #[test]
    fn leaves_no_temporary_files_behind() {
        let root = scratch_dir("no-tmp-files");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();

        backup_dir(&src, &dest, &BackupOptions::default()).unwrap();

        let names: Vec<_> = fs::read_dir(&dest)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![OsString::from("a.txt")]);
    }

    fn zstd() -> BackupOptions {
        BackupOptions {
            compression: Compression::Zstd,
        }
    }

    fn decode(path: &Path) -> Vec<u8> {
        zstd::stream::decode_all(fs::File::open(path).unwrap()).unwrap()
    }

    #[test]
    fn zstd_backup_roundtrips_content_and_writes_a_manifest() {
        let root = scratch_dir("zstd-roundtrip");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"hello hello hello").unwrap();
        fs::write(src.join("sub/b.txt"), b"world").unwrap();
        std::os::unix::fs::symlink("a.txt", src.join("link.txt")).unwrap();

        let summary = backup_dir(&src, &dest, &zstd()).unwrap();

        assert_eq!(summary.files_copied, 2);
        assert_eq!(summary.symlinks_recreated, 1);
        assert!(summary.bytes_written > 0);
        assert_eq!(decode(&dest.join("a.txt.zst")), b"hello hello hello");
        assert_eq!(decode(&dest.join("sub/b.txt.zst")), b"world");
        assert!(
            dest.join("link.txt")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(dest.join(".securesave/manifest.json").is_file());
    }

    #[test]
    fn zstd_second_run_skips_unchanged_files() {
        let root = scratch_dir("zstd-skip-unchanged");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::write(src.join("b.txt"), b"world").unwrap();

        backup_dir(&src, &dest, &zstd()).unwrap();
        let second = backup_dir(&src, &dest, &zstd()).unwrap();

        assert_eq!(second.files_copied, 0);
        assert_eq!(second.files_unchanged, 2);
        assert_eq!(second.bytes_written, 0);
    }

    #[test]
    fn zstd_recopies_a_changed_file() {
        let root = scratch_dir("zstd-changed");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        backup_dir(&src, &dest, &zstd()).unwrap();

        fs::write(src.join("a.txt"), b"changed content").unwrap();
        let second = backup_dir(&src, &dest, &zstd()).unwrap();

        assert_eq!(second.files_copied, 1);
        assert_eq!(decode(&dest.join("a.txt.zst")), b"changed content");
    }

    #[test]
    fn zstd_never_trusts_the_manifest_alone() {
        let root = scratch_dir("zstd-manifest-lies");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::write(src.join("b.txt"), b"world").unwrap();
        backup_dir(&src, &dest, &zstd()).unwrap();

        fs::remove_file(dest.join("a.txt.zst")).unwrap();

        let second = backup_dir(&src, &dest, &zstd()).unwrap();

        assert_eq!(second.files_copied, 1);
        assert_eq!(second.files_unchanged, 1);
        assert_eq!(decode(&dest.join("a.txt.zst")), b"hello");
    }

    #[test]
    fn zstd_without_a_manifest_recopies_everything() {
        let root = scratch_dir("zstd-no-manifest");
        let src = root.join("src");
        let dest = root.join("dest");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::write(src.join("b.txt"), b"world").unwrap();
        backup_dir(&src, &dest, &zstd()).unwrap();

        fs::remove_file(dest.join(".securesave/manifest.json")).unwrap();

        let second = backup_dir(&src, &dest, &zstd()).unwrap();

        assert_eq!(second.files_copied, 2);
        assert_eq!(second.files_unchanged, 0);
    }

    #[test]
    fn fails_with_the_offending_path_in_the_error() {
        let root = scratch_dir("error-path");
        let missing = root.join("does-not-exist");
        let dest = root.join("dest");

        let err = backup_dir(&missing, &dest, &BackupOptions::default()).unwrap_err();

        assert!(err.to_string().contains("does-not-exist"));
    }
}
