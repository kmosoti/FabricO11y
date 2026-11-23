//! Zero-TOCTOU `OwnedFd` / `*at` filesystem and strict private-mode helpers.

use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, Stat, fchmod, fstat, fsync, open, openat, renameat, statat,
    unlinkat,
};
use rustix::io::Errno;
use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub use std::os::fd::OwnedFd;

/// Opens an existing directory strictly with `O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`.
pub(crate) fn open_dir_fd_nofollow(path: &Path, label: &str) -> Result<OwnedFd, String> {
    open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|err| {
        if err == Errno::LOOP
            || (err == Errno::NOTDIR
                && std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()))
        {
            format!("refusing symlinked {label} {}", path.display())
        } else {
            format!("failed opening {label} {}: {err}", path.display())
        }
    })
}

/// Opens a direct child directory relative to an anchored directory FD without following symlinks.
pub(crate) fn open_subdir_fd_nofollow(
    parent_fd: impl AsFd,
    child_name: &str,
    label: &str,
) -> Result<OwnedFd, String> {
    openat(
        &parent_fd,
        child_name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|err| {
        if err == Errno::LOOP
            || (err == Errno::NOTDIR
                && matches!(
                    stat_child_nofollow(&parent_fd, child_name, label),
                    Ok(Some(st)) if FileType::from_raw_mode(st.st_mode as _) == FileType::Symlink
                ))
        {
            format!("refusing symlinked {label} {child_name}")
        } else {
            format!("failed opening {label} {child_name}: {err}")
        }
    })
}

/// Inspects a directory entry relative to `parent_fd` without following symlinks.
pub(crate) fn stat_child_nofollow(
    parent_fd: impl AsFd,
    child_name: &str,
    label: &str,
) -> Result<Option<Stat>, String> {
    match statat(parent_fd, child_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(st) => Ok(Some(st)),
        Err(Errno::NOENT) => Ok(None),
        Err(err) => Err(format!("failed inspecting {label} {child_name}: {err}")),
    }
}

/// Verifies `dir_fd` is still the exact `(dev, ino)` directory currently named `child_name`
/// inside `parent_fd`, preventing directory swap or symlink replacement between checks.
pub(crate) fn verify_dir_fd_unchanged(
    parent_fd: impl AsFd,
    child_name: &str,
    dir_fd: impl AsFd,
    expected_dev: u64,
    expected_ino: u64,
    label: &str,
) -> Result<(), String> {
    let fd_stat = fstat(dir_fd).map_err(|e| format!("failed stating {label} fd: {e}"))?;
    let entry_stat = statat(&parent_fd, child_name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|e| format!("failed re-inspecting {label} {child_name}: {e}"))?;
    let entry_ty = FileType::from_raw_mode(entry_stat.st_mode as _);
    if entry_ty == FileType::Symlink || entry_ty != FileType::Directory {
        return Err(format!(
            "refusing non-directory or symlinked {label} {child_name}"
        ));
    }
    if fd_stat.st_dev as u64 != expected_dev
        || fd_stat.st_ino as u64 != expected_ino
        || entry_stat.st_dev as u64 != expected_dev
        || entry_stat.st_ino as u64 != expected_ino
    {
        return Err(format!(
            "{label} {child_name} changed concurrently during atomic write"
        ));
    }
    Ok(())
}

/// Enforces mode `0o700` on an owned directory via an `O_NOFOLLOW | O_DIRECTORY` FD,
/// bypassing ambient umask and pathname races.
pub(crate) fn enforce_private_dir_path_mode(path: &Path, label: &str) -> Result<(), String> {
    let fd = open_dir_fd_nofollow(path, label)?;
    let st = fstat(&fd).map_err(|e| format!("failed stating {label} {}: {e}", path.display()))?;
    if FileType::from_raw_mode(st.st_mode as _) != FileType::Directory {
        return Err(format!("non-directory {label} {}", path.display()));
    }
    let my_uid = rustix::process::getuid().as_raw();
    if st.st_uid == my_uid && ((st.st_mode as u32) & 0o777) != 0o700 {
        fchmod(&fd, Mode::from_raw_mode(0o700))
            .map_err(|e| format!("failed setting 0700 on {label} {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Enforces mode `0o600` on an already-open regular custody file FD regardless of ambient umask.
pub(crate) fn enforce_private_file_mode(file: &File, label: &str) -> Result<(), String> {
    let st = fstat(file).map_err(|e| format!("failed stating {label} fd: {e}"))?;
    if FileType::from_raw_mode(st.st_mode as _) != FileType::RegularFile {
        return Err(format!("non-regular {label} fd"));
    }
    if ((st.st_mode as u32) & 0o777) != 0o600 {
        fchmod(file, Mode::from_raw_mode(0o600))
            .map_err(|e| format!("failed setting 0600 on {label}: {e}"))?;
    }
    Ok(())
}

/// Writes `bytes` to a `O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC` temporary file inside
/// `staging_fd` (mode `0o600`), verifies anchored directory identities before/after, and
/// atomically publishes it via `renameat` into `target_fd/target_name`, fsyncing directories.
pub(crate) fn write_staged_then_renameat(
    data_dir: &Path,
    data_fd: &OwnedFd,
    staging_name: &str,
    target_dir_name: &str,
    tmp_name: &str,
    target_name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let data_stat = fstat(data_fd)
        .map_err(|e| format!("failed stating data dir {}: {e}", data_dir.display()))?;
    let data_path_meta = std::fs::symlink_metadata(data_dir)
        .map_err(|e| format!("failed inspecting data dir {}: {e}", data_dir.display()))?;
    if data_path_meta.file_type().is_symlink()
        || !data_path_meta.is_dir()
        || data_path_meta.dev() != data_stat.st_dev as u64
        || data_path_meta.ino() != data_stat.st_ino as u64
    {
        return Err(format!(
            "data dir {} changed concurrently before publish",
            data_dir.display()
        ));
    }

    let staging_fd = open_subdir_fd_nofollow(data_fd, staging_name, "staging dir")?;
    let state_fd = open_subdir_fd_nofollow(data_fd, target_dir_name, "state dir")?;
    let staging_stat =
        fstat(&staging_fd).map_err(|e| format!("failed stating staging dir: {e}"))?;
    let state_stat = fstat(&state_fd).map_err(|e| format!("failed stating state dir: {e}"))?;
    let staging_dev = staging_stat.st_dev as u64;
    let staging_ino = staging_stat.st_ino as u64;
    let state_dev = state_stat.st_dev as u64;
    let state_ino = state_stat.st_ino as u64;

    let check_target_safe = || -> Result<(), String> {
        if let Some(st) = stat_child_nofollow(&state_fd, target_name, target_name)? {
            let ty = FileType::from_raw_mode(st.st_mode as _);
            if ty == FileType::Symlink {
                return Err(format!("refusing symlinked {target_name}"));
            }
            if ty != FileType::RegularFile {
                return Err(format!("non-regular {target_name}"));
            }
        }
        Ok(())
    };
    check_target_safe()?;

    let tmp_fd = openat(
        &staging_fd,
        tmp_name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|e| format!("failed creating {tmp_name}: {e}"))?;

    let mut file = File::from(tmp_fd);
    if let Err(err) = enforce_private_file_mode(&file, tmp_name)
        .and_then(|()| file.write_all(bytes).map_err(|e| e.to_string()))
        .and_then(|()| file.sync_all().map_err(|e| e.to_string()))
    {
        drop(file);
        let _ = unlinkat(&staging_fd, tmp_name, AtFlags::empty());
        return Err(err);
    }
    drop(file);

    if let Err(err) = verify_dir_fd_unchanged(
        data_fd,
        staging_name,
        &staging_fd,
        staging_dev,
        staging_ino,
        "staging dir",
    )
    .and_then(|()| {
        verify_dir_fd_unchanged(
            data_fd,
            target_dir_name,
            &state_fd,
            state_dev,
            state_ino,
            "state dir",
        )
    })
    .and_then(|()| check_target_safe())
    {
        let _ = unlinkat(&staging_fd, tmp_name, AtFlags::empty());
        return Err(err);
    }

    if let Err(err) = renameat(&staging_fd, tmp_name, &state_fd, target_name) {
        let _ = unlinkat(&staging_fd, tmp_name, AtFlags::empty());
        return Err(format!("failed renaming {target_name}: {err}"));
    }

    verify_dir_fd_unchanged(
        data_fd,
        staging_name,
        &staging_fd,
        staging_dev,
        staging_ino,
        "staging dir",
    )?;
    verify_dir_fd_unchanged(
        data_fd,
        target_dir_name,
        &state_fd,
        state_dev,
        state_ino,
        "state dir",
    )?;
    let _ = fsync(&staging_fd);
    fsync(&state_fd).map_err(|e| format!("failed syncing state dir: {e}"))?;
    Ok(())
}
