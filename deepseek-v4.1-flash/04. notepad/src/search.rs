//! Literal (non-regex) search over a document snapshot.
//!
//! # Contract
//!
//! * An empty query produces **no** matches and no error. Next/Previous with an
//!   empty query are no-ops, and Replace All is refused rather than deleting the
//!   document — the one dangerous thing a naive empty-search implementation does.
//! * Matching is on bytes, but every match is validated to start and end on a
//!   UTF-8 character boundary, so a query can never match the inside of a
//!   character. A case-insensitive ASCII query therefore behaves sanely on
//!   non-ASCII text instead of producing corrupted offsets.
//! * Case-insensitive matching folds ASCII only. Full Unicode case folding is
//!   explicitly out of scope, and the case-sensitive toggle makes the exact
//!   behaviour available either way.
//! * Runs against a [`Snapshot`], so it can never observe a half-applied edit
//!   and never blocks the UI thread. Cancellation is polled every chunk.

use crate::buffer::Snapshot;
use crate::progress::Progress;
use std::sync::Arc;

/// Bytes processed between cancellation and progress checks.
const TICK: usize = 1 << 20;

#[derive(Clone, Debug)]
pub struct Query {
    pub text: Vec<u8>,
    pub case_sensitive: bool,
}

impl Query {
    /// Build a query, or `None` when it is empty.
    pub fn new(text: &str, case_sensitive: bool) -> Option<Query> {
        if text.is_empty() {
            return None;
        }
        Some(Query {
            text: text.as_bytes().to_vec(),
            case_sensitive,
        })
    }

    /// First byte, used to skip cheaply when case-insensitive.
    fn first_pair(&self) -> (u8, Option<u8>) {
        let b = self.text[0];
        if self.case_sensitive {
            (b, None)
        } else {
            (b.to_ascii_lowercase(), Some(b.to_ascii_uppercase()))
        }
    }
}

#[inline]
fn eq_at(hay: &[u8], at: usize, needle: &[u8], case_sensitive: bool) -> bool {
    if at + needle.len() > hay.len() {
        return false;
    }
    let win = &hay[at..at + needle.len()];
    if case_sensitive {
        win == needle
    } else {
        win.eq_ignore_ascii_case(needle)
    }
}

/// One match, as a byte range in the snapshot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Match {
    pub start: usize,
    pub end: usize,
}

/// Search a snapshot, calling `on_match` for every hit in document order.
///
/// Returns `true` if the scan completed, `false` if it was cancelled. Matching
/// never spans a chunk boundary incorrectly: the chunk being scanned is extended
/// with `len(needle) - 1` bytes of lookahead read from the following chunks.
pub fn scan<F: FnMut(Match)>(
    snap: &Snapshot,
    query: &Query,
    progress: &Arc<Progress>,
    mut on_match: F,
) -> bool {
    if query.text.is_empty() || snap.is_empty() {
        return true;
    }
    let needle = &query.text;
    let nlen = needle.len();
    let total = snap.len() as u64;
    progress.set_total(total);
    progress.set_phase("scanning");

    // Materialise one window at a time: [window_start, window_end) with a tail
    // of nlen-1 bytes so a match straddling the window edge is still found.
    let window = 8usize << 20;
    let mut base = 0usize;
    let mut buf: Vec<u8> = Vec::with_capacity(window + nlen);

    while base < snap.len() {
        let want = (snap.len() - base).min(window);
        let read_end = (base + want + nlen).min(snap.len());
        snap.read_into(base, read_end, &mut buf);

        let limit = want; // matches must start within the window body
        let mut i = 0usize;
        while i < limit {
            let hay = &buf[i..];
            let found = if query.case_sensitive {
                memchr::memchr(needle[0], hay)
            } else {
                let (lo, hi) = query.first_pair();
                memchr::memchr2(lo, hi.unwrap_or(lo), hay)
            };
            let Some(off) = found else { break };
            let at = i + off;
            if at >= limit {
                break;
            }
            if eq_at(&buf, at, needle, query.case_sensitive) {
                // Only report matches that begin and end on character boundaries,
                // checked against the window rather than the snapshot so the hot
                // loop stays linear.
                let start_ok = (buf[at] & 0xC0) != 0x80;
                let end_ok = match buf.get(at + nlen) {
                    Some(&b) => (b & 0xC0) != 0x80,
                    None => base + at + nlen >= snap.len(),
                };
                if start_ok && end_ok {
                    on_match(Match {
                        start: base + at,
                        end: base + at + nlen,
                    });
                }
                i = at + nlen;
            } else {
                i = at + 1;
            }
        }

        base += want;
        progress.set_done(base as u64);
        if progress.is_cancelled() {
            return false;
        }
    }
    progress.set_done(total);
    true
}

/// Collect every match. Used by tests and by small documents.
pub fn find_all(snap: &Snapshot, query: &Query) -> Vec<Match> {
    let progress = Progress::new();
    let mut out = Vec::new();
    scan(snap, query, &progress, |m| out.push(m));
    out
}

/// Index of the first match starting strictly after `pos`, if any.
pub fn first_after(matches: &[Match], pos: usize) -> Option<usize> {
    match matches.partition_point(|m| m.start <= pos) {
        i if i < matches.len() => Some(i),
        _ => None,
    }
}

/// Index of the last match starting strictly before `pos`, if any.
pub fn last_before(matches: &[Match], pos: usize) -> Option<usize> {
    match matches.partition_point(|m| m.start < pos) {
        0 => None,
        i => Some(i - 1),
    }
}

/// Index of the match containing `pos`, if any.
pub fn containing(matches: &[Match], pos: usize) -> Option<usize> {
    let i = matches.partition_point(|m| m.end <= pos);
    matches.get(i).filter(|m| m.start <= pos).map(|_| i)
}

/// The match to treat as "current" when the cursor is at `caret`.
///
/// Prefers the match containing the caret; otherwise the first one after it;
/// otherwise wraps to the first match.
pub fn current_for(matches: &[Match], caret: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    containing(matches, caret)
        .or_else(|| first_after(matches, caret))
        .or(Some(0))
}

// ---------------------------------------------------------------------------
// Replace-all planning
// ---------------------------------------------------------------------------

/// One entry of a replace-all result.
///
/// The worker produces these; the main thread turns them into a piece list.
/// Unchanged regions are carried as `Keep`, which is a 16-byte piece referring to
/// an immutable source — so a replace-all over a 100 MiB document copies only the
/// replacement text, not the document.
pub enum Segment {
    /// Keep this piece of the original document.
    Keep(crate::buffer::Piece),
    /// Insert this freshly produced text.
    Insert(Vec<u8>),
}

/// Build the segment list for replacing every match with `replacement`.
///
/// Matches must be sorted and non-overlapping, which is what [`scan`] produces.
/// Returns `None` if the work was cancelled.
pub fn build_replace_plan(
    snap: &Snapshot,
    matches: &[Match],
    replacement: &[u8],
    progress: &Arc<Progress>,
) -> Option<Vec<Segment>> {
    if matches.is_empty() {
        return Some(Vec::new());
    }
    progress.set_total(snap.len() as u64);
    progress.set_phase("building replacement");

    let mut out: Vec<Segment> = Vec::with_capacity(snap.pieces().len() + matches.len() * 2);
    let mut abs = 0usize;
    let mut mi = 0usize;
    let mut since_tick = 0usize;

    for &p in snap.pieces() {
        let pstart = abs;
        let pend = abs + p.len as usize;
        let mut cursor = pstart;

        while mi < matches.len() && matches[mi].start < pend {
            let m = matches[mi];
            let ms = m.start.max(cursor);
            let me = m.end.min(pend);
            if ms > cursor {
                out.push(Segment::Keep(
                    snap.clip_piece(p, cursor - pstart, ms - pstart),
                ));
            }
            // Emit the replacement exactly once, in the piece where the match
            // begins; a match spanning pieces is only started once.
            if m.start >= pstart && m.start < pend {
                out.push(Segment::Insert(replacement.to_vec()));
            }
            cursor = cursor.max(me);
            if m.end > pend {
                // Match continues into the next piece; revisit it there.
                break;
            }
            mi += 1;
        }

        if cursor < pend {
            out.push(Segment::Keep(
                snap.clip_piece(p, cursor - pstart, pend - pstart),
            ));
        }

        abs = pend;
        since_tick += p.len as usize;
        if since_tick >= TICK {
            since_tick = 0;
            progress.set_done(abs as u64);
            if progress.is_cancelled() {
                return None;
            }
        }
    }

    progress.set_done(snap.len() as u64);
    Some(out)
}

// ---------------------------------------------------------------------------
// UI-side search state
// ---------------------------------------------------------------------------

/// Everything the search bar needs for one document.
#[derive(Default)]
pub struct SearchState {
    pub open: bool,
    pub replace_open: bool,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    /// Matches against `epoch`; empty while a scan is running or after an edit.
    pub matches: Vec<Match>,
    pub current: Option<usize>,
    /// Buffer generation these matches were computed from.
    pub epoch: u64,
    /// True once a scan for `epoch` finished (possibly with zero matches).
    pub complete: bool,
    /// Set when a scan was cancelled or superseded.
    pub stale: bool,
}

impl SearchState {
    /// Invalidate results because the document changed.
    pub fn invalidate(&mut self) {
        self.matches.clear();
        self.current = None;
        self.complete = false;
        self.stale = false;
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn current_ordinal(&self) -> Option<usize> {
        self.current.map(|i| i + 1)
    }

    /// Human-readable match count for the search bar.
    pub fn count_label(&self, scanning: bool) -> String {
        if self.query.is_empty() {
            return "no query".to_string();
        }
        if scanning {
            return format!("{} found so far…", self.matches.len());
        }
        if !self.complete {
            return "—".to_string();
        }
        match (self.matches.len(), self.current_ordinal()) {
            (0, _) => "no matches".to_string(),
            (n, Some(i)) => format!("{i} of {n}"),
            (n, None) => format!("{n} matches"),
        }
    }
}
