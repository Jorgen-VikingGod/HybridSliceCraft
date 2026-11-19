//! Saving files so that a failed or interrupted save never leaves a
//! half-written file behind: the bytes go to a hidden file beside the target,
//! are flushed to disk, and then replace the target in one step
//! (craft-atomic). A file that was there before stays as it was until then.

use std::fs::File;
use std::path::Path;

use craft_atomic::{NewFilePermissions, WriteOptions};

/// What a save that the caller stopped on purpose reports.
pub const CANCELLED: &str = "Cancelled";

fn message(e: std::io::Error) -> String {
    format!("couldn't save the file: {e}")
}

/// Save `bytes` as `path`.
pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    craft_atomic::atomic_write(path, bytes).map_err(message)
}

/// Save a file that belongs to the person (settings, session): new files are
/// readable by them only.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    WriteOptions::default()
        .with_new_file_permissions(NewFilePermissions::Private)
        .atomic_write(path, bytes)
        .map_err(message)
}

/// Save `path` by writing into the staged file with `fill`. An error from
/// `fill` (such as [`CANCELLED`]) comes back as it was, and `path` is untouched.
pub fn write_with(path: &Path, fill: impl FnOnce(&mut File) -> Result<(), String>) -> Result<(), String> {
    let mut failed = None;
    let r = craft_atomic::atomic_write_with(path, |file| {
        fill(file).map_err(|e| {
            let io = std::io::Error::other(e.clone());
            failed = Some(e);
            io
        })
    });
    match (r, failed) {
        (Ok(()), _) => Ok(()),
        (Err(_), Some(e)) => Err(e),
        (Err(e), None) => Err(message(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_failed_save_leaves_the_old_file_and_no_leftovers() {
        let dir = std::env::temp_dir().join(format!("hsc-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.gcode");
        write(&p, b"old").unwrap();
        let e = write_with(&p, |f| {
            f.write_all(b"half").map_err(|e| e.to_string())?;
            Err(CANCELLED.into())
        });
        assert_eq!(e, Err(CANCELLED.to_string()));
        assert_eq!(std::fs::read(&p).unwrap(), b"old");
        write_with(&p, |f| f.write_all(b"new").map_err(|e| e.to_string())).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "staging files left behind: {names:?}");
        write_private(&dir.join("sub/prefs.json"), b"{}").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
