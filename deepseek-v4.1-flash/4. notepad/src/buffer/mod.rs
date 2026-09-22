//! The document buffer: a piece table over an implicit treap.
//!
//! # Why a piece table
//!
//! A piece table keeps the document as a *sequence of views* into immutable byte
//! sources instead of as one contiguous array. An edit is then a splice of the
//! piece list plus an append to the "added text" source — it never moves the
//! existing document bytes. That is what makes a keystroke in a 100 MiB file
//! cost the same as a keystroke in a 100 byte file.
//!
//! # Layout
//!
//! ```text
//!   original : Arc<Vec<u8>>        the bytes as loaded, never mutated
//!   chunks   : ChunkStore          append-only source for every inserted byte
//!   tree     : Treap               pieces in document order, each
//!                                  {src, start, len, newline_count}
//! ```
//!
//! `original` is sliced into pieces of at most [`CHUNK_CAP`] bytes at load time
//! rather than stored as one giant piece. That bound matters: when an edit cuts a
//! piece in half, the newline counts of the two halves are recovered by scanning
//! them, and the bound keeps that scan at 64 KiB instead of "the whole file".
//!
//! # Indexing strategy
//!
//! The treap is *implicit*: nodes are addressed by byte position, and every node
//! caches `sub_len` (bytes) and `sub_nl` (newlines) for its subtree. Both
//! directions of the line index therefore fall out of one structure:
//!
//! * `line_of_byte(pos)` — `newlines_before(pos)`, a descent accumulating
//!   left-subtree newline counts, `O(log n)`;
//! * `line_start(line)` — `newline_pos(line - 1) + 1`, the same descent asking
//!   for the k-th newline, `O(log n)` plus one bounded scan inside the piece
//!   that contains it.
//!
//! There is no separate line-start array to keep in sync, and no full-document
//! rescan per keystroke. Rendering additionally walks a [`PieceStream`], which
//! yields contiguous slices in document order from any byte offset, so a frame
//! touches only the visible bytes.
//!
//! # Memory trade-offs
//!
//! * Original file bytes: 1x the file size, resident, shared with snapshots.
//! * Inserted bytes: 1x everything ever typed, in 64 KiB chunks. Bytes deleted
//!   from the document stay resident as long as an undo record or snapshot
//!   references them; the undo stack is therefore the main unbounded growth
//!   vector, and it is capped (see [`crate::undo`]).
//! * Piece: 16 bytes, plus a 32 byte treap node while it is live. A file loaded
//!   as `n/64 KiB` pieces starts with `n/65536` pieces and grows by at most 2
//!   pieces per edit.
//! * Snapshot: `O(#chunks)` `Arc` increments plus `O(#pieces)` bytes of piece
//!   list. Cheap enough to take one per background job.
//!
//! # Limits
//!
//! Piece offsets are `u32`, so a document is capped at [`MAX_DOC_LEN`] bytes
//! (4 GiB - 1) and individual sources are capped at the same. Both are enforced
//! at load and at insert; exceeding them is a clean error, never a wrap.

mod chunk;
mod treap;

pub use chunk::{ChunkStore, CHUNK_CAP};
pub use treap::Treap;

use std::sync::Arc;

/// Null node / end-of-stream sentinel.
pub const NIL: u32 = u32::MAX;
/// Piece source tag meaning "the original loaded bytes".
pub const ORIGINAL: u32 = u32::MAX - 1;

/// Maximum document size, imposed by `u32` piece offsets.
pub const MAX_DOC_LEN: usize = u32::MAX as usize;

/// A view into an immutable byte source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Piece {
    /// [`ORIGINAL`], or an index into the chunk store.
    pub src: u32,
    /// Offset of the first byte within the source.
    pub start: u32,
    /// Number of bytes.
    pub len: u32,
    /// Number of `\n` bytes in `[start, start + len)`.
    pub nl: u32,
}

/// Byte-source resolver handed to tree operations that need to look at bytes.
#[derive(Clone, Copy)]
pub struct PieceSource<'a> {
    original: &'a [u8],
    chunks: &'a ChunkStore,
}

impl<'a> PieceSource<'a> {
    #[inline]
    pub fn get(&self, src: u32, start: u32, len: u32) -> &'a [u8] {
        let base = if src == ORIGINAL {
            self.original
        } else {
            self.chunks.get(src)
        };
        &base[start as usize..(start + len) as usize]
    }

    /// Split a piece at `off`, recovering both halves' newline counts.
    fn split_piece(&self, p: Piece, off: u32) -> (Piece, Piece) {
        let bytes = self.get(p.src, p.start, p.len);
        let (a, b) = bytes.split_at(off as usize);
        (
            Piece {
                src: p.src,
                start: p.start,
                len: off,
                nl: count_newlines(a),
            },
            Piece {
                src: p.src,
                start: p.start + off,
                len: p.len - off,
                nl: count_newlines(b),
            },
        )
    }
}

#[inline]
pub(crate) fn count_newlines(bytes: &[u8]) -> u32 {
    memchr::memchr_iter(b'\n', bytes).count() as u32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferError {
    /// The edit would push the document past [`MAX_DOC_LEN`].
    TooLarge,
}

impl std::fmt::Display for BufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BufferError::TooLarge => write!(
                f,
                "document would exceed the {} GiB limit",
                MAX_DOC_LEN / (1024 * 1024 * 1024)
            ),
        }
    }
}

impl std::error::Error for BufferError {}

/// A completed mutation, described at piece granularity.
///
/// This is exactly the information undo/redo needs, and it is produced by the
/// same call that performs the edit, so the two can never disagree.
pub struct Delta {
    /// Pieces removed from `[pos, pos + removed_len)`.
    pub removed: Vec<Piece>,
    pub removed_len: usize,
    /// Pieces now occupying `[pos, pos + inserted_len)`.
    pub inserted: Vec<Piece>,
    pub inserted_len: usize,
}

/// A mutable text document held as a piece table.
pub struct Buffer {
    original: Arc<Vec<u8>>,
    chunks: ChunkStore,
    tree: Treap,
    /// Bumped on every mutation; render caches key off this.
    generation: u64,
}

impl Buffer {
    pub fn empty() -> Self {
        Buffer {
            original: Arc::new(Vec::new()),
            chunks: ChunkStore::new(),
            tree: Treap::new(),
            generation: 1,
        }
    }

    /// Build a buffer from raw file bytes.
    ///
    /// The bytes must already be validated UTF-8; this function does not check,
    /// it only slices them into pieces and counts their newlines in one pass.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, BufferError> {
        if bytes.len() > MAX_DOC_LEN {
            return Err(BufferError::TooLarge);
        }
        let original = Arc::new(bytes);
        let pieces = slice_original(&original);
        let mut tree = Treap::new();
        tree.reset(&pieces);
        Ok(Buffer {
            original,
            chunks: ChunkStore::new(),
            tree,
            generation: 1,
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.tree.len() as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Number of `\n` bytes.
    #[inline]
    pub fn newline_count(&self) -> usize {
        self.tree.newlines() as usize
    }

    /// Number of lines. A trailing newline does not create an extra empty line
    /// beyond the one the file already has, matching common editor behaviour:
    /// `"a\n"` is one line, `"a\nb"` is two.
    #[inline]
    pub fn line_count(&self) -> usize {
        self.newline_count() + 1
    }

    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn piece_count(&self) -> usize {
        self.tree.piece_count()
    }

    /// Resident bytes attributable to this buffer (excluding shared snapshots).
    pub fn memory_bytes(&self) -> usize {
        self.original.len() + self.chunks.bytes() + self.tree.piece_count() * 48
    }

    #[inline]
    fn src(&self) -> PieceSource<'_> {
        PieceSource {
            original: &self.original,
            chunks: &self.chunks,
        }
    }

    // ---- mutation -------------------------------------------------------

    /// Insert `data` at byte offset `pos`.
    pub fn insert(&mut self, pos: usize, data: &[u8]) -> Result<Delta, BufferError> {
        self.replace_capture(pos, pos, data)
    }

    /// Delete `[from, to)`.
    pub fn remove(&mut self, from: usize, to: usize) -> Delta {
        self.replace_capture(from, to, &[])
            .expect("deletion cannot exceed the document size limit")
    }

    /// Replace `[from, to)` with `data`, reporting the piece-level delta.
    ///
    /// The reported `removed` run is exactly what undo needs to put the old bytes
    /// back: pieces reference immutable sources, so restoring them copies no
    /// text, however large the deletion was.
    ///
    /// On error the buffer is left untouched.
    pub fn replace_capture(
        &mut self,
        from: usize,
        to: usize,
        data: &[u8],
    ) -> Result<Delta, BufferError> {
        debug_assert!(from <= to && to <= self.len());
        let new_len = self.len() - (to - from) + data.len();
        if new_len > MAX_DOC_LEN {
            return Err(BufferError::TooLarge);
        }

        // Borrow the immutable sources and the tree as *separate fields* so the
        // tree can be mutated while a `PieceSource` is alive.
        let mut removed = Vec::new();
        if to > from {
            let src = PieceSource {
                original: &self.original,
                chunks: &self.chunks,
            };
            self.tree
                .take_run(from as u32, to as u32, &src, &mut removed);
        }

        let inserted = if data.is_empty() {
            Vec::new()
        } else {
            let pieces = self.write_chunks(data);
            let src = PieceSource {
                original: &self.original,
                chunks: &self.chunks,
            };
            self.tree.insert_run(from as u32, &pieces, &src);
            pieces
        };

        self.generation += 1;
        Ok(Delta {
            removed_len: to - from,
            inserted_len: data.len(),
            removed,
            inserted,
        })
    }

    /// Re-insert a previously removed run of pieces at `pos`.
    pub fn insert_pieces(&mut self, pos: usize, pieces: &[Piece]) {
        if pieces.is_empty() {
            return;
        }
        let src = PieceSource {
            original: &self.original,
            chunks: &self.chunks,
        };
        self.tree.insert_run(pos as u32, pieces, &src);
        self.generation += 1;
    }

    /// Remove `[from, to)` and hand back its pieces.
    pub fn take_pieces(&mut self, from: usize, to: usize) -> Vec<Piece> {
        let mut removed = Vec::new();
        if to > from {
            let src = PieceSource {
                original: &self.original,
                chunks: &self.chunks,
            };
            self.tree
                .take_run(from as u32, to as u32, &src, &mut removed);
            self.generation += 1;
        }
        removed
    }

    /// Append `data` to the chunk store, returning the pieces that cover it.
    fn write_chunks(&mut self, data: &[u8]) -> Vec<Piece> {
        let mut pieces = Vec::with_capacity(data.len() / CHUNK_CAP + 1);
        let mut rest = data;
        while !rest.is_empty() {
            let (src, start, written) = self.chunks.append(rest);
            pieces.push(Piece {
                src,
                start,
                len: written as u32,
                nl: count_newlines(&rest[..written]),
            });
            rest = &rest[written..];
        }
        pieces
    }

    /// Dump the treap arena for diagnosis. See [`treap::NodeRow`] for the field order.
    pub fn dump_tree(&self) -> Vec<treap::NodeRow> {
        self.tree.dump()
    }

    /// The treap's current root node index.
    pub fn root_index(&self) -> u32 {
        self.tree.root()
    }

    /// Verify the structural invariants of the piece table.
    ///
    /// Checks the treap itself, then confirms that flattening the tree and
    /// streaming it produce the same bytes — those two paths must agree, because
    /// snapshots (and therefore saves and searches) use the flattened piece list
    /// while rendering and editing use the stream.
    pub fn check_invariants(&self) {
        self.tree.check_invariants();

        let flat = self.tree.flatten();
        let src = self.src();
        let mut joined = Vec::with_capacity(self.len());
        for p in &flat {
            joined.extend_from_slice(src.get(p.src, p.start, p.len));
        }

        let mut streamed = Vec::with_capacity(self.len());
        let mut s = self.stream_from(0);
        while let Some(slice) = s.next_slice() {
            streamed.extend_from_slice(slice);
        }

        assert_eq!(
            joined.len(),
            self.len(),
            "flattened pieces do not sum to the reported length"
        );
        assert_eq!(
            joined, streamed,
            "flatten() disagrees with the piece stream about document order"
        );
    }

    // ---- reading --------------------------------------------------------

    /// Append `[from, to)` to `out`. `out` is cleared first so callers can reuse
    /// one buffer across lines without reallocating.
    pub fn read_into(&self, from: usize, to: usize, out: &mut Vec<u8>) {
        out.clear();
        if from >= to || to > self.len() {
            return;
        }
        let mut need = to - from;
        out.reserve(need);
        let mut stream = self.stream_from(from);
        while need > 0 {
            match stream.next_slice() {
                Some(s) => {
                    let take = need.min(s.len());
                    out.extend_from_slice(&s[..take]);
                    need -= take;
                }
                None => break,
            }
        }
    }

    pub fn read_vec(&self, from: usize, to: usize) -> Vec<u8> {
        let mut v = Vec::new();
        self.read_into(from, to, &mut v);
        v
    }

    /// Copy up to `out.len()` bytes starting at `pos`; returns how many were read.
    pub fn read_prefix(&self, pos: usize, out: &mut [u8]) -> usize {
        if out.is_empty() || pos >= self.len() {
            return 0;
        }
        let mut stream = self.stream_from(pos);
        let mut done = 0;
        while done < out.len() {
            match stream.next_slice() {
                Some(s) => {
                    let take = (out.len() - done).min(s.len());
                    out[done..done + take].copy_from_slice(&s[..take]);
                    done += take;
                }
                None => break,
            }
        }
        done
    }

    pub fn byte_at(&self, pos: usize) -> Option<u8> {
        if pos >= self.len() {
            return None;
        }
        let mut stream = self.stream_from(pos);
        stream.next_slice().and_then(|s| s.first().copied())
    }

    /// Contiguous byte slices in document order, starting at `pos`.
    #[inline]
    pub fn stream_from(&self, pos: usize) -> PieceStream<'_> {
        PieceStream::new(&self.tree, self.src(), pos)
    }

    // ---- line index -----------------------------------------------------

    /// Byte offset of the first byte of `line` (0-based). Clamped to the end.
    pub fn line_start(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        let src = self.src();
        match self.tree.newline_pos((line - 1) as u32, &src) {
            Some(p) => (p as usize + 1).min(self.len()),
            None => self.len(),
        }
    }

    /// 0-based line containing byte offset `pos`.
    pub fn line_of_byte(&self, pos: usize) -> usize {
        let src = self.src();
        self.tree.newlines_before(pos.min(self.len()) as u32, &src) as usize
    }

    /// Byte offset of the `line`-th newline, if it exists.
    pub fn newline_pos(&self, line: usize) -> Option<usize> {
        let src = self.src();
        self.tree.newline_pos(line as u32, &src).map(|p| p as usize)
    }

    /// Content span of `line`, excluding the line terminator.
    ///
    /// A `CR` is only part of the terminator when an `LF` actually follows it. A
    /// lone trailing `CR` at the end of the document is content, not an ending.
    pub fn line_span(&self, line: usize) -> (usize, usize) {
        let start = self.line_start(line);
        let (end, terminated) = match self.newline_pos(line) {
            Some(p) => (p, true),
            None => (self.len(), false),
        };
        let mut e = end.max(start);
        if terminated && e > start && self.byte_at(e - 1) == Some(b'\r') {
            e -= 1;
        }
        (start, e)
    }

    /// Total span of `line` including its terminator.
    pub fn line_full_span(&self, line: usize) -> (usize, usize) {
        let start = self.line_start(line);
        let end = match self.newline_pos(line) {
            Some(p) => p + 1,
            None => self.len(),
        };
        (start, end.max(start))
    }

    // ---- character boundaries -------------------------------------------

    pub fn is_char_boundary(&self, pos: usize) -> bool {
        let n = self.len();
        if pos > n {
            return false;
        }
        if pos == n {
            return true;
        }
        let mut b = [0u8; 1];
        if self.read_prefix(pos, &mut b) == 0 {
            return true;
        }
        (b[0] & 0xC0) != 0x80
    }

    /// Offset of the next character boundary after `pos`.
    pub fn next_char(&self, pos: usize) -> usize {
        let n = self.len();
        if pos >= n {
            return n;
        }
        let mut b = [0u8; 4];
        let got = self.read_prefix(pos, &mut b);
        if got == 0 {
            return n;
        }
        (pos + utf8_seq_len(b[0])).min(n)
    }

    /// Offset of the previous character boundary before `pos`.
    pub fn prev_char(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let start = pos.saturating_sub(4);
        let mut b = [0u8; 4];
        let got = self.read_prefix(start, &mut b[..pos - start]);
        if got == 0 {
            return start;
        }
        let mut i = got;
        while i > 1 && (b[i - 1] & 0xC0) == 0x80 {
            i -= 1;
        }
        start + i - 1
    }

    /// Snap `pos` back to the nearest character boundary.
    pub fn clamp_boundary(&self, pos: usize) -> usize {
        let n = self.len();
        let mut p = pos.min(n);
        let mut guard = 0;
        while p > 0 && !self.is_char_boundary(p) && guard < 4 {
            p -= 1;
            guard += 1;
        }
        p
    }

    // ---- snapshots ------------------------------------------------------

    /// Cheap, immutable, self-contained view for background work.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            original: Arc::clone(&self.original),
            chunks: self.chunks.clone(),
            pieces: self.tree.flatten(),
            len: self.len(),
            newlines: self.newline_count(),
        }
    }

    /// Flatten the live tree into a piece list in document order.
    pub fn flatten(&self) -> Vec<Piece> {
        self.tree.flatten()
    }

    /// Replace the whole piece list, keeping the same immutable sources.
    ///
    /// This is how a background replace-all lands: the worker computes the new
    /// piece list plus the bytes it wants inserted, and the main thread
    /// materialises those bytes into *this* buffer's chunk store. Keeping one
    /// store per document is what makes the whole thing safe — undo records
    /// reference chunk indices, and they stay valid only if the store outlives
    /// every piece that points into it.
    pub fn install(&mut self, pieces: Vec<Piece>) {
        debug_assert!(pieces.iter().all(|p| p.len > 0));
        self.tree.reset(&pieces);
        self.generation += 1;
    }

    /// Append `data` to the chunk store without touching the tree, returning the
    /// pieces that cover it. Used by the replace-all landing path.
    pub fn stage_bytes(&mut self, data: &[u8]) -> Vec<Piece> {
        self.write_chunks(data)
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::empty()
    }
}

impl std::fmt::Debug for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The treap arena would be thousands of lines of noise; the shape of the
        // document is what matters when debugging.
        f.debug_struct("Buffer")
            .field("len", &self.len())
            .field("lines", &self.line_count())
            .field("pieces", &self.piece_count())
            .field("original_bytes", &self.original.len())
            .field("chunk_bytes", &self.chunks.bytes())
            .finish()
    }
}

/// Slice the original bytes into pieces bounded by [`CHUNK_CAP`].
fn slice_original(bytes: &[u8]) -> Vec<Piece> {
    let mut out = Vec::with_capacity(bytes.len() / CHUNK_CAP + 1);
    let mut off = 0usize;
    while off < bytes.len() {
        let take = (bytes.len() - off).min(CHUNK_CAP);
        let s = &bytes[off..off + take];
        out.push(Piece {
            src: ORIGINAL,
            start: off as u32,
            len: take as u32,
            nl: count_newlines(s),
        });
        off += take;
    }
    out
}

#[inline]
fn utf8_seq_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >= 0xF0 {
        4
    } else if b >= 0xE0 {
        3
    } else if b >= 0xC0 {
        2
    } else {
        // Stray continuation byte; treat as one byte so we always make progress.
        1
    }
}

// ---------------------------------------------------------------------------
// Streaming over the live tree
// ---------------------------------------------------------------------------

/// Yields contiguous byte slices in document order starting at a byte offset.
///
/// Each slice is a whole piece (clipped at the start offset), so a full
/// traversal costs one step per piece and never per byte.
pub struct PieceStream<'a> {
    src: PieceSource<'a>,
    nodes: &'a [super::buffer::treap::Node],
    root: u32,
    stack: Vec<u32>,
    cur_src: u32,
    cur_start: u32,
    cur_rem: u32,
    done: bool,
}

impl<'a> PieceStream<'a> {
    fn new(tree: &'a Treap, src: PieceSource<'a>, pos: usize) -> Self {
        let mut s = PieceStream {
            src,
            nodes: tree.nodes(),
            root: tree.root(),
            stack: Vec::with_capacity(32),
            cur_src: 0,
            cur_start: 0,
            cur_rem: 0,
            done: false,
        };
        s.seek(pos as u32);
        s
    }

    fn seek(&mut self, pos: u32) {
        let mut cur = self.root;
        let mut pos = pos;
        while cur != NIL {
            let n = &self.nodes[cur as usize];
            let (l, r, piece) = (n.left, n.right, n.piece);
            let llen = if l == NIL {
                0
            } else {
                self.nodes[l as usize].sub_len
            };
            if pos < llen {
                self.stack.push(cur);
                cur = l;
            } else if pos >= llen + piece.len {
                pos -= llen + piece.len;
                cur = r;
            } else {
                let off = pos - llen;
                self.cur_src = piece.src;
                self.cur_start = piece.start + off;
                self.cur_rem = piece.len - off;
                let mut c = r;
                while c != NIL {
                    self.stack.push(c);
                    c = self.nodes[c as usize].left;
                }
                return;
            }
        }
        self.done = true;
    }

    /// Next contiguous slice, or `None` at the end of the document.
    pub fn next_slice(&mut self) -> Option<&'a [u8]> {
        if self.cur_rem > 0 {
            let s = self.src.get(self.cur_src, self.cur_start, self.cur_rem);
            self.cur_rem = 0;
            return Some(s);
        }
        if self.done {
            return None;
        }
        let n = match self.stack.pop() {
            Some(n) => n,
            None => {
                self.done = true;
                return None;
            }
        };
        let node = &self.nodes[n as usize];
        let piece = node.piece;
        let mut c = node.right;
        while c != NIL {
            self.stack.push(c);
            c = self.nodes[c as usize].left;
        }
        Some(self.src.get(piece.src, piece.start, piece.len))
    }
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// An immutable, shareable copy of a document's byte content.
///
/// Holds only `Arc` handles plus a flat piece list, so taking one on a 100 MiB
/// document copies a few thousand pointers rather than 100 MiB of text.
#[derive(Clone)]
pub struct Snapshot {
    original: Arc<Vec<u8>>,
    chunks: ChunkStore,
    pieces: Vec<Piece>,
    len: usize,
    newlines: usize,
}

impl Snapshot {
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn newline_count(&self) -> usize {
        self.newlines
    }

    #[inline]
    pub fn line_count(&self) -> usize {
        self.newlines + 1
    }

    #[inline]
    pub fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    /// The chunk store, so a background job can build a new buffer that shares
    /// this document's immutable sources.
    pub fn chunk_store(&self) -> &ChunkStore {
        &self.chunks
    }

    #[inline]
    pub fn slice_of(&self, p: Piece) -> &[u8] {
        let base = if p.src == ORIGINAL {
            &self.original[..]
        } else {
            self.chunks.get(p.src)
        };
        &base[p.start as usize..(p.start + p.len) as usize]
    }

    /// Narrow a piece to `[from, to)` within itself, recounting newlines.
    pub fn clip_piece(&self, p: Piece, from: usize, to: usize) -> Piece {
        debug_assert!(from < to && to <= p.len as usize);
        let s = self.slice_of(p);
        Piece {
            src: p.src,
            start: p.start + from as u32,
            len: (to - from) as u32,
            nl: count_newlines(&s[from..to]),
        }
    }

    /// Visit every byte of the document as contiguous slices, in order.
    pub fn for_each_slice<F: FnMut(&[u8])>(&self, mut f: F) {
        for &p in &self.pieces {
            f(self.slice_of(p));
        }
    }

    /// Slices in document order starting at `from`, clipped to the first piece.
    pub fn slices_from(&self, from: usize) -> SnapshotStream<'_> {
        let mut idx = 0usize;
        let mut off = from;
        while idx < self.pieces.len() {
            let p = self.pieces[idx];
            if off < p.len as usize {
                break;
            }
            off -= p.len as usize;
            idx += 1;
        }
        SnapshotStream {
            snap: self,
            idx,
            off,
        }
    }

    pub fn read_into(&self, from: usize, to: usize, out: &mut Vec<u8>) {
        out.clear();
        if from >= to {
            return;
        }
        out.reserve(to - from);
        let mut need = to - from;
        let mut it = self.slices_from(from);
        while need > 0 {
            match it.next_slice() {
                Some(s) => {
                    let take = need.min(s.len());
                    out.extend_from_slice(&s[..take]);
                    need -= take;
                }
                None => break,
            }
        }
    }

    pub fn read_vec(&self, from: usize, to: usize) -> Vec<u8> {
        let mut v = Vec::new();
        self.read_into(from, to, &mut v);
        v
    }

    /// Number of newlines strictly before `pos`. Linear in the piece count, which
    /// is fine for the handful of calls made per frame.
    pub fn newlines_before(&self, pos: usize) -> usize {
        let mut n = 0usize;
        let mut left = pos.min(self.len);
        for &p in &self.pieces {
            if left == 0 {
                break;
            }
            if (p.len as usize) <= left {
                n += p.nl as usize;
                left -= p.len as usize;
            } else {
                n += count_newlines(&self.slice_of(p)[..left]) as usize;
                break;
            }
        }
        n
    }

    /// 0-based line containing byte offset `pos`.
    pub fn line_of_byte(&self, pos: usize) -> usize {
        self.newlines_before(pos)
    }

    /// Byte offset of the first byte of `line`.
    pub fn line_start(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        let mut seen = 0usize;
        let mut base = 0usize;
        for &p in &self.pieces {
            let s = self.slice_of(p);
            if seen + p.nl as usize >= line {
                let want = line - seen;
                let off = memchr::memchr_iter(b'\n', s).nth(want - 1).unwrap();
                return base + off + 1;
            }
            seen += p.nl as usize;
            base += p.len as usize;
        }
        self.len
    }

    /// Content span of `line`, excluding the terminator.
    ///
    /// Mirrors [`Buffer::line_span`]: a `CR` counts as part of the ending only
    /// when an `LF` follows it, so a trailing lone `CR` stays content.
    pub fn line_span(&self, line: usize) -> (usize, usize) {
        let start = self.line_start(line);
        let next = self.line_start(line + 1);
        let terminated =
            next > start && next <= self.len && self.byte_at(next - 1) == Some(b'\n');
        let mut end = if terminated { next - 1 } else { next.max(start) };
        if terminated && end > start && self.byte_at(end - 1) == Some(b'\r') {
            end -= 1;
        }
        (start, end.max(start))
    }

    pub fn byte_at(&self, pos: usize) -> Option<u8> {
        if pos >= self.len {
            return None;
        }
        let mut left = pos;
        for &p in &self.pieces {
            if (p.len as usize) > left {
                return Some(self.slice_of(p)[left]);
            }
            left -= p.len as usize;
        }
        None
    }
}

/// Iterator produced by [`Snapshot::slices_from`].
pub struct SnapshotStream<'a> {
    snap: &'a Snapshot,
    idx: usize,
    off: usize,
}

impl<'a> SnapshotStream<'a> {
    pub fn next_slice(&mut self) -> Option<&'a [u8]> {
        let p = *self.snap.pieces.get(self.idx)?;
        let s = self.snap.slice_of(p);
        let out = &s[self.off.min(s.len())..];
        self.idx += 1;
        self.off = 0;
        if out.is_empty() {
            self.next_slice()
        } else {
            Some(out)
        }
    }
}
