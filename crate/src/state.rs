//! Small, crash-resistant local state file helpers.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn atomic_write(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), String> {
    atomic_write_inner(path, contents.as_ref(), false)
}

pub fn atomic_write_private(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), String> {
    atomic_write_inner(path, contents.as_ref(), true)
}

fn atomic_write_inner(path: &Path, contents: &[u8], private: bool) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("state path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;

    let temp = temp_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(|e| format!("create {}: {e}", temp.display()))?;
        file.write_all(contents)
            .map_err(|e| format!("write {}: {e}", temp.display()))?;
        file.sync_all()
            .map_err(|e| format!("sync {}: {e}", temp.display()))?;

        #[cfg(unix)]
        if private {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("chmod {}: {e}", temp.display()))?;
        }
        #[cfg(not(unix))]
        let _ = private;

        replace_file(&temp, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    path.with_file_name(format!(".{name}.{}.{}.tmp", std::process::id(), seq))
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|e| format!("replace {}: {e}", to.display()))
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new_name: *const u16, flags: u32) -> i32;
    }

    let from_wide = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let to_wide = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let ok = unsafe {
        MoveFileExW(
            from_wide.as_ptr(),
            to_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(format!(
            "replace {}: {}",
            to.display(),
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::atomic_write;

    #[test]
    fn atomic_write_replaces_existing_content() {
        let dir = std::env::temp_dir().join(format!("graft-state-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("state");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(dir);
    }
}
