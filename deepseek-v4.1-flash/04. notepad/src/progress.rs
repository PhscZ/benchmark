//! Cross-thread progress and cancellation for background jobs.
//!
//! One [`Progress`] is shared between a worker thread and the UI. It is
//! deliberately lock-free on the hot path: a worker doing a 100 MiB scan reports
//! byte counters with relaxed atomics and only touches the mutex-protected phase
//! string when the phase actually changes.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
    phase: Mutex<String>,
}

impl Progress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Set the total work for the current phase.
    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn set_done(&self, done: u64) {
        self.done.store(done, Ordering::Relaxed);
    }

    pub fn add_done(&self, delta: u64) {
        self.done.fetch_add(delta, Ordering::Relaxed);
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Completion in `0.0..=1.0`, or `None` while the total is unknown.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total();
        if total == 0 {
            return None;
        }
        Some((self.done() as f32 / total as f32).clamp(0.0, 1.0))
    }

    /// Replace the human-readable phase label.
    pub fn set_phase(&self, phase: &str) {
        if let Ok(mut g) = self.phase.lock() {
            if g.as_str() != phase {
                g.clear();
                g.push_str(phase);
            }
        }
    }

    pub fn phase(&self) -> String {
        self.phase.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Ask the worker to stop; the worker polls [`Self::is_cancelled`].
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn reset_cancel(&self) {
        self.cancel.store(false, Ordering::Relaxed);
    }
}

/// A running or finished background operation attached to a document.
pub struct Op {
    pub kind: OpKind,
    pub progress: Arc<Progress>,
    pub started: std::time::Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpKind {
    Load,
    Save,
    Search,
    ReplaceAll,
}

impl OpKind {
    pub fn verb(self) -> &'static str {
        match self {
            OpKind::Load => "Loading",
            OpKind::Save => "Saving",
            OpKind::Search => "Searching",
            OpKind::ReplaceAll => "Replacing",
        }
    }

    /// Whether this operation takes exclusive control of the document.
    ///
    /// Search is read-only and runs against a snapshot, so editing stays live
    /// while it runs. Loading, saving and replace-all change what "the document"
    /// means and therefore lock editing until they finish.
    pub fn locks_editing(self) -> bool {
        matches!(self, OpKind::Load | OpKind::Save | OpKind::ReplaceAll)
    }
}

impl Op {
    pub fn new(kind: OpKind) -> Self {
        Op {
            kind,
            progress: Progress::new(),
            started: std::time::Instant::now(),
        }
    }

    pub fn elapsed_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }

    /// Status text including the phase and, when known, a percentage.
    pub fn status_text(&self) -> String {
        let phase = self.progress.phase();
        let pct = self
            .progress
            .fraction()
            .map(|f| format!(" — {}%", (f * 100.0).round() as u32))
            .unwrap_or_default();
        if phase.is_empty() {
            format!("{}{}", self.kind.verb(), pct)
        } else {
            format!("{} {}{}", self.kind.verb(), phase, pct)
        }
    }
}
