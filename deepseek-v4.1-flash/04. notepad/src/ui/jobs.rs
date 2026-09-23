//! Background worker plumbing.
//!
//! Loading, saving, searching and replace-all all run on their own thread. Each
//! job reports progress through a shared [`Progress`] and delivers exactly one
//! result message. The UI never blocks on a job; it polls the channel once per
//! frame.
//!
//! # Why snapshots
//!
//! Save and search operate on a [`Snapshot`] rather than on the live buffer.
//! Taking a snapshot is `O(#pieces)` pointer copies, so:
//!
//! * a save writes a *consistent* document even if the user keeps typing, and
//!   the edits made meanwhile stay marked unsaved;
//! * a search can never observe a half-applied edit, and an edit that lands
//!   mid-scan bumps the document's epoch, which makes the UI discard the stale
//!   result instead of highlighting text that has moved.

use crate::buffer::{Snapshot, MAX_DOC_LEN};
use crate::document::DocId;
use crate::fileio::{self, FileError, Loaded};
use crate::progress::Progress;
use crate::search::{self, Match, Query, Segment};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// Maximum matches retained for one search, to bound memory on a pathological
/// query over a huge document.
pub const MAX_MATCHES: usize = 1_000_000;

/// A message from a worker to the UI.
pub enum JobMsg {
    Loaded {
        doc: DocId,
        path: PathBuf,
        result: Result<Loaded, FileError>,
    },
    Saved {
        doc: DocId,
        path: PathBuf,
        /// Bytes written, or the failure.
        result: Result<u64, FileError>,
        /// Undo serial that was on top when the snapshot was taken.
        serial: u64,
    },
    Searched {
        doc: DocId,
        epoch: u64,
        matches: Vec<Match>,
        truncated: bool,
        completed: bool,
    },
    ReplacePlanned {
        doc: DocId,
        epoch: u64,
        plan: Option<Vec<Segment>>,
        count: usize,
    },
}

pub struct Jobs {
    tx: Sender<JobMsg>,
    rx: Receiver<JobMsg>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new()
    }
}

impl Jobs {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Jobs { tx, rx }
    }

    /// Drain every message that has arrived since the last poll.
    pub fn poll(&self) -> Vec<JobMsg> {
        let mut out = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            out.push(m);
        }
        out
    }

    /// Read and validate a file on a worker thread.
    pub fn spawn_load(&self, doc: DocId, path: PathBuf, progress: Arc<Progress>) {
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name(format!("load-{doc}"))
            .spawn(move || {
                let result = fileio::load(&path, &progress);
                let _ = tx.send(JobMsg::Loaded { doc, path, result });
            })
            .expect("failed to spawn load thread");
    }

    /// Write a snapshot to disk atomically on a worker thread.
    pub fn spawn_save(
        &self,
        doc: DocId,
        path: PathBuf,
        snapshot: Snapshot,
        had_bom: bool,
        serial: u64,
        progress: Arc<Progress>,
    ) {
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name(format!("save-{doc}"))
            .spawn(move || {
                let result = fileio::save_atomic(&path, &progress, |w| {
                    fileio::write_snapshot(w, &snapshot, had_bom, &progress)
                });
                let _ = tx.send(JobMsg::Saved {
                    doc,
                    path,
                    result,
                    serial,
                });
            })
            .expect("failed to spawn save thread");
    }

    /// Scan a snapshot for literal matches.
    pub fn spawn_search(
        &self,
        doc: DocId,
        snapshot: Snapshot,
        query: Query,
        epoch: u64,
        progress: Arc<Progress>,
    ) {
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name(format!("search-{doc}"))
            .spawn(move || {
                let mut matches: Vec<Match> = Vec::new();
                let mut truncated = false;
                let completed = search::scan(&snapshot, &query, &progress, |m| {
                    if matches.len() < MAX_MATCHES {
                        matches.push(m);
                    } else {
                        truncated = true;
                    }
                });
                let _ = tx.send(JobMsg::Searched {
                    doc,
                    epoch,
                    matches,
                    truncated,
                    completed,
                });
            })
            .expect("failed to spawn search thread");
    }

    /// Compute the piece-level plan for a replace-all on a worker thread.
    pub fn spawn_replace_plan(
        &self,
        doc: DocId,
        snapshot: Snapshot,
        matches: Vec<Match>,
        replacement: String,
        epoch: u64,
        progress: Arc<Progress>,
    ) {
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name(format!("replace-{doc}"))
            .spawn(move || {
                let count = matches.len();
                let plan =
                    search::build_replace_plan(&snapshot, &matches, replacement.as_bytes(), &progress);
                let _ = tx.send(JobMsg::ReplacePlanned {
                    doc,
                    epoch,
                    plan,
                    count,
                });
            })
            .expect("failed to spawn replace thread");
    }
}

/// Size check used before starting a load, so the UI can refuse early.
pub fn too_large(size: u64) -> bool {
    size > MAX_DOC_LEN as u64
}
