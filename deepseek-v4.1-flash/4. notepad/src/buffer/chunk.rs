//! Append-only store of immutable byte chunks.
//!
//! Every byte ever inserted into a document lives here. Edits never rewrite or
//! drop a chunk; they append. Two properties fall out of that:
//!
//! * Cloning the store is `O(#chunks)` pointer copies, so a background job
//!   (search, save, replace-all) can take a snapshot of a 100 MiB document for
//!   the price of a few thousand `Arc` increments.
//! * A chunk that a live snapshot still references is copied on write the first
//!   time the live buffer appends to it again, costing at most [`CHUNK_CAP`]
//!   bytes. The snapshot keeps observing the old bytes.

use std::sync::Arc;

/// Maximum payload of a single chunk.
///
/// This is the granularity knob for the whole editor. It bounds the work done
/// when an edit splits a piece (the newline counts of both halves are recovered
/// by scanning, so a split costs at most one chunk scan), and it bounds the
/// copy-on-write penalty paid when the live buffer appends to a chunk that a
/// snapshot still holds.
pub const CHUNK_CAP: usize = 64 * 1024;

/// Ordered, append-only sequence of byte chunks.
#[derive(Clone, Default)]
pub struct ChunkStore {
    chunks: Vec<Arc<Vec<u8>>>,
}

impl ChunkStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Total payload bytes held across all chunks.
    pub fn bytes(&self) -> usize {
        self.chunks.iter().map(|c| c.len()).sum()
    }

    pub fn get(&self, idx: u32) -> &[u8] {
        &self.chunks[idx as usize]
    }

    /// Append as much of `data` as fits in the current chunk, starting a new
    /// chunk if necessary.
    ///
    /// Returns `(chunk_index, offset_within_chunk, bytes_written)`. Callers loop
    /// until `data` is exhausted; each individual piece therefore never exceeds
    /// [`CHUNK_CAP`] bytes.
    pub fn append(&mut self, data: &[u8]) -> (u32, u32, usize) {
        debug_assert!(!data.is_empty(), "append called with no data");

        if let Some(i) = self.chunks.len().checked_sub(1) {
            let cur = &self.chunks[i];
            if cur.len() < CHUNK_CAP {
                let room = CHUNK_CAP - cur.len();
                let take = room.min(data.len());
                let v = Arc::make_mut(&mut self.chunks[i]);
                let off = v.len() as u32;
                v.extend_from_slice(&data[..take]);
                return (i as u32, off, take);
            }
        }

        let take = data.len().min(CHUNK_CAP);
        self.chunks.push(Arc::new(data[..take].to_vec()));
        ((self.chunks.len() - 1) as u32, 0, take)
    }
}
