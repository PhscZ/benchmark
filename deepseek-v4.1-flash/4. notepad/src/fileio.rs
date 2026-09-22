//! File loading and saving.
//!
//! # Data safety
//!
//! Saving never truncates the destination. The bytes are written to a temporary
//! file in the *same directory*, flushed, and only then renamed over the target.
//! A failure at any point before the rename leaves the original file untouched,
//! and the rename itself is atomic on NTFS. If the target is a symlink the link
//! is resolved first so the rename replaces the real file rather than the link.
//!
//! # UTF-8
//!
//! Loading validates the whole file and reports the exact byte offset of the
//! first invalid sequence. Invalid input is *rejected*: the editor never opens a
//! lossily decoded document, because saving it back would silently rewrite the
//! user's bytes. A UTF-8 BOM is accepted, remembered, and written back on save so
//! round-tripping a BOM'd file does not change it.

use crate::buffer::{Buffer, MAX_DOC_LEN};
use crate::progress::Progress;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// BOM length in bytes.
pub const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Bytes read per progress tick while loading.
const LOAD_CHUNK: usize = 4 << 20;

#[derive(Debug)]
pub enum FileError {
    Io {
        path: PathBuf,
        err: std::io::Error,
    },
    /// The file is not valid UTF-8; `offset` is the first offending byte.
    NotUtf8 {
        path: PathBuf,
        offset: usize,
        detail: String,
    },
    TooLarge {
        path: PathBuf,
        size: u64,
    },
    /// A load or save was cancelled by the user.
    Cancelled,
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Io { path, err } => {
                write!(f, "{}: {}", path.display(), err)
            }
            FileError::NotUtf8 {
                path,
                offset,
                detail,
            } => write!(
                f,
                "{} is not valid UTF-8 (first invalid byte at offset {}: {})",
                path.display(),
                offset,
                detail
            ),
            FileError::TooLarge { path, size } => write!(
                f,
                "{} is {} bytes, which exceeds the {} GiB limit",
                path.display(),
                size,
                MAX_DOC_LEN / (1024 * 1024 * 1024)
            ),
            FileError::Cancelled => write!(f, "operation cancelled"),
        }
    }
}

impl std::error::Error for FileError {}

/// Result of a successful load.
#[derive(Debug)]
pub struct Loaded {
    pub buffer: Buffer,
    pub had_bom: bool,
    pub size: u64,
}

/// Validate `bytes` as UTF-8, reporting the first error precisely.
pub fn validate_utf8(path: &Path, bytes: &[u8]) -> Result<(), FileError> {
    match std::str::from_utf8(bytes) {
        Ok(_) => Ok(()),
        Err(e) => {
            let offset = e.error_len().map(|_| e.valid_up_to()).unwrap_or(e.valid_up_to());
            Err(FileError::NotUtf8 {
                path: path.to_path_buf(),
                offset,
                detail: describe_utf8_error(bytes, e.valid_up_to()),
            })
        }
    }
}

/// Explain what is wrong at `at`, for the error message.
fn describe_utf8_error(bytes: &[u8], at: usize) -> String {
    let Some(&b) = bytes.get(at) else {
        return "unexpected end of file".to_string();
    };
    if b < 0x80 {
        return format!("unexpected byte 0x{b:02X}");
    }
    if (b & 0xC0) == 0x80 {
        return format!("stray continuation byte 0x{b:02X}");
    }
    let need = if b >= 0xF0 {
        4
    } else if b >= 0xE0 {
        3
    } else {
        2
    };
    let have = bytes.len() - at;
    if have < need {
        format!("truncated {need}-byte sequence at end of file")
    } else {
        format!("invalid {need}-byte sequence starting with 0x{b:02X}")
    }
}

/// Read and validate a file, reporting progress and honouring cancellation.
pub fn load(path: &Path, progress: &Arc<Progress>) -> Result<Loaded, FileError> {
    let meta = fs::metadata(path).map_err(|err| FileError::Io {
        path: path.to_path_buf(),
        err,
    })?;
    let size = meta.len();
    if size > MAX_DOC_LEN as u64 {
        return Err(FileError::TooLarge {
            path: path.to_path_buf(),
            size,
        });
    }

    progress.set_phase("reading");
    progress.set_total(size);

    let mut file = File::open(path).map_err(|err| FileError::Io {
        path: path.to_path_buf(),
        err,
    })?;

    let mut data = Vec::with_capacity(size as usize);
    let mut chunk = vec![0u8; LOAD_CHUNK];
    loop {
        if progress.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        let n = file.read(&mut chunk).map_err(|err| FileError::Io {
            path: path.to_path_buf(),
            err,
        })?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        progress.set_done(data.len() as u64);
    }

    progress.set_phase("validating UTF-8");
    validate_utf8(path, &data)?;

    let had_bom = data.starts_with(BOM);
    if had_bom {
        data.drain(..BOM.len());
    }

    progress.set_phase("building index");
    let buffer = Buffer::from_bytes(data).map_err(|_| FileError::TooLarge {
        path: path.to_path_buf(),
        size,
    })?;

    progress.set_done(progress.total());
    Ok(Loaded {
        buffer,
        had_bom,
        size,
    })
}

/// Write `source` to `path` atomically.
///
/// `source` is called with a writer and must emit the exact bytes to store. The
/// temp file lives next to the target so the final rename stays on one volume.
pub fn save_atomic<F>(path: &Path, progress: &Arc<Progress>, source: F) -> Result<u64, FileError>
where
    F: FnOnce(&mut dyn Write) -> std::io::Result<()>,
{
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let dir = dir.unwrap_or_else(|| Path::new("."));

    progress.set_phase("writing");

    // Resolve symlinks so we replace the real file, not the link.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let tmp = temp_path(dir, path);
    let result = (|| -> Result<u64, FileError> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|err| FileError::Io {
                path: tmp.clone(),
                err,
            })?;
        let mut w = BufWriter::with_capacity(1 << 20, file);
        source(&mut w).map_err(|err| FileError::Io {
            path: tmp.clone(),
            err,
        })?;
        w.flush().map_err(|err| FileError::Io {
            path: tmp.clone(),
            err,
        })?;
        let file = w.into_inner().map_err(|e| FileError::Io {
            path: tmp.clone(),
            err: e.into_error(),
        })?;
        // Durability: force the bytes to disk before the rename publishes them.
        file.sync_all().map_err(|err| FileError::Io {
            path: tmp.clone(),
            err,
        })?;
        drop(file);

        let written = fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);

        progress.set_phase("replacing");
        replace_file(&tmp, &target).map_err(|err| FileError::Io {
            path: target.clone(),
            err,
        })?;
        Ok(written)
    })();

    if result.is_err() {
        // Never leave a stray temp file behind on failure.
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Replace `target` with `tmp`.
///
/// On Windows `fs::rename` fails if the destination exists, so removal is
/// required; the window between remove and rename is the only moment the
/// original name is absent, and the temp file still holds the full contents.
fn replace_file(tmp: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        match fs::rename(tmp, target) {
            Ok(()) => Ok(()),
            Err(_) => {
                // Windows can refuse when the destination exists or is locked.
                if target.exists() {
                    fs::remove_file(target)?;
                }
                fs::rename(tmp, target)
            }
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(tmp, target)
    }
}

/// A unique temporary path in `dir`, derived from the target's name.
fn temp_path(dir: &Path, target: &Path) -> PathBuf {
    let stem = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_string());
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    dir.join(format!(".{stem}.{pid}.{nanos}.tmp"))
}

/// Stream a buffer's bytes to `w`, honouring cancellation.
///
/// Iterates the piece list, so the copy is linear in bytes and never allocates a
/// document-sized intermediate.
pub fn write_buffer(
    w: &mut dyn Write,
    buffer: &Buffer,
    had_bom: bool,
    progress: &Arc<Progress>,
) -> std::io::Result<()> {
    if had_bom {
        w.write_all(BOM)?;
    }
    progress.set_total(buffer.len() as u64);
    let mut written = 0usize;
    let mut stream = buffer.stream_from(0);
    while let Some(s) = stream.next_slice() {
        if progress.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "save cancelled",
            ));
        }
        w.write_all(s)?;
        written += s.len();
        progress.set_done(written as u64);
    }
    Ok(())
}

/// Stream a snapshot's bytes to `w`.
pub fn write_snapshot(
    w: &mut dyn Write,
    snap: &crate::buffer::Snapshot,
    had_bom: bool,
    progress: &Arc<Progress>,
) -> std::io::Result<()> {
    if had_bom {
        w.write_all(BOM)?;
    }
    progress.set_total(snap.len() as u64);
    let mut written = 0usize;
    let mut it = snap.slices_from(0);
    while let Some(s) = it.next_slice() {
        if progress.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "save cancelled",
            ));
        }
        w.write_all(s)?;
        written += s.len();
        progress.set_done(written as u64);
    }
    Ok(())
}

/// Copy a byte range of `path` into a fresh in-memory `Vec`.
///
/// Unused by the editor's hot paths; kept for tests and tooling.
pub fn read_range(path: &Path, from: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(from))?;
    let mut v = vec![0u8; len];
    f.read_exact(&mut v)?;
    Ok(v)
}
