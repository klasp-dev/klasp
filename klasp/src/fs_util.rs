//! Shared filesystem helpers.

use std::io;
use std::path::{Path, PathBuf};

/// Atomically write `contents` to `path` via a sibling tempfile + rename.
pub(crate) fn atomic_write_text(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tf = tempfile::NamedTempFile::new_in(dir)?;
    tf.write_all(contents.as_bytes())?;
    tf.flush()?;
    tf.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Return the user's home directory.
///
/// Uses `HOME` on Unix / `USERPROFILE` on Windows, falling back to
/// [`std::env::home_dir`] for edge cases. Returns `None` when the home
/// directory cannot be determined.
#[allow(dead_code)] // used from binary (cmd/init.rs, cmd/setup.rs); not from lib integration tests
pub(crate) fn home_dir() -> Option<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir()
}
