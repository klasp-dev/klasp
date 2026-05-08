//! Shared filesystem helpers.

use std::io;
use std::path::Path;

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
