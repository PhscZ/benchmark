//! Integration tests for the document buffer, undo, search and file handling.
//!
//! The centrepiece is [`random_edits_match_a_naive_string_model`]: the piece table
//! is driven through thousands of pseudo-random edits and after every single one
//! its bytes, its line index and its character boundaries are compared against a
//! plain `String` mutated the same way. That one test covers the invariants that
//! are otherwise easy to break silently — piece splitting, newline bookkeeping,
//! and byte-offset arithmetic.

use notepad::buffer::{Buffer, CHUNK_CAP};
use notepad::document::{detect_eol, Document, Eol};
use notepad::motion;
use notepad::progress::Progress;
use notepad::search::{self, Query};
use notepad::undo::{EditKind, History, Sel};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Deterministic PRNG so failures reproduce exactly.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

fn buf_text(b: &Buffer) -> String {
    String::from_utf8(b.read_vec(0, b.len())).expect("buffer must stay valid UTF-8")
}

fn snap_text(s: &notepad::buffer::Snapshot) -> String {
    String::from_utf8(s.read_vec(0, s.len())).expect("snapshot must stay valid UTF-8")
}

/// A unique scratch directory for one test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "notepad-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).expect("write fixture");
    p
}

/// Reference line model: `(start, content_end, full_end)` per line, computed the
/// slow, obviously-correct way.
fn reference_lines(s: &str) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let mut content_end = i;
            if content_end > start && bytes[content_end - 1] == b'\r' {
                content_end -= 1;
            }
            out.push((start, content_end, i + 1));
            start = i + 1;
        }
        i += 1;
    }
    out.push((start, bytes.len(), bytes.len()));
    out
}

// ---------------------------------------------------------------------------
// Buffer correctness
// ---------------------------------------------------------------------------

#[test]
fn empty_buffer_reports_one_line() {
    let b = Buffer::empty();
    assert_eq!(b.len(), 0);
    assert_eq!(b.line_count(), 1);
    assert_eq!(b.line_start(0), 0);
    assert_eq!(b.line_span(0), (0, 0));
    assert!(b.is_empty());
}

#[test]
fn insert_and_read_back() {
    let mut b = Buffer::empty();
    b.insert(0, b"hello").unwrap();
    assert_eq!(buf_text(&b), "hello");
    b.insert(5, b" world").unwrap();
    assert_eq!(buf_text(&b), "hello world");
    b.insert(0, b">> ").unwrap();
    assert_eq!(buf_text(&b), ">> hello world");
    assert_eq!(b.len(), 14);
}

#[test]
fn remove_across_pieces() {
    let mut b = Buffer::empty();
    b.insert(0, b"abcdefghij").unwrap();
    b.insert(3, b"XYZ").unwrap();
    assert_eq!(buf_text(&b), "abcXYZdefghij");
    b.remove(1, 9);
    assert_eq!(buf_text(&b), "aghij");
}

#[test]
fn line_index_tracks_newlines() {
    let mut b = Buffer::empty();
    b.insert(0, b"one\ntwo\nthree").unwrap();
    assert_eq!(b.line_count(), 3);
    assert_eq!(b.line_start(0), 0);
    assert_eq!(b.line_start(1), 4);
    assert_eq!(b.line_start(2), 8);
    assert_eq!(b.line_span(0), (0, 3));
    assert_eq!(b.line_span(1), (4, 7));
    assert_eq!(b.line_span(2), (8, 13));
    assert_eq!(b.line_of_byte(0), 0);
    assert_eq!(b.line_of_byte(3), 0);
    assert_eq!(b.line_of_byte(4), 1);
    assert_eq!(b.line_of_byte(12), 2);

    // Inserting a newline in the middle shifts every later line.
    b.insert(3, b"\n").unwrap();
    assert_eq!(buf_text(&b), "one\n\ntwo\nthree");
    assert_eq!(b.line_count(), 4);
    assert_eq!(b.line_start(2), 5);
    assert_eq!(b.line_of_byte(5), 2);
}

#[test]
fn trailing_newline_does_not_add_a_phantom_line() {
    let mut b = Buffer::empty();
    b.insert(0, b"a\n").unwrap();
    assert_eq!(b.line_count(), 2, "\"a\\n\" is one terminated line plus an empty last line");
    assert_eq!(b.line_span(0), (0, 1));
    assert_eq!(b.line_span(1), (2, 2));
}

#[test]
fn crlf_is_excluded_from_line_spans() {
    let mut b = Buffer::empty();
    b.insert(0, b"alpha\r\nbeta\r\n").unwrap();
    assert_eq!(b.line_count(), 3);
    assert_eq!(b.line_span(0), (0, 5), "the CR must not be part of the content");
    assert_eq!(b.line_span(1), (7, 11));
    assert_eq!(b.line_full_span(0), (0, 7));
}

#[test]
fn character_boundaries_on_multibyte_text() {
    let mut b = Buffer::empty();
    b.insert(0, "aé漢😀z".as_bytes()).unwrap();
    let s = buf_text(&b);
    assert_eq!(s, "aé漢😀z");

    // Walk forward and confirm every step lands on a boundary and covers exactly
    // one character.
    let mut p = 0usize;
    let mut chars = Vec::new();
    while p < b.len() {
        assert!(b.is_char_boundary(p), "offset {p} must be a boundary");
        let next = b.next_char(p);
        assert!(next > p, "must always make progress");
        chars.push(String::from_utf8(b.read_vec(p, next)).unwrap());
        p = next;
    }
    assert_eq!(chars.join(""), "aé漢😀z");
    assert_eq!(chars.len(), 5);

    // And backwards.
    let mut p = b.len();
    let mut back = Vec::new();
    while p > 0 {
        let prev = b.prev_char(p);
        back.push(String::from_utf8(b.read_vec(prev, p)).unwrap());
        p = prev;
    }
    back.reverse();
    assert_eq!(back.join(""), "aé漢😀z");
}

#[test]
fn clamp_boundary_snaps_into_a_character() {
    let mut b = Buffer::empty();
    b.insert(0, "é".as_bytes()).unwrap(); // 2 bytes
    assert_eq!(b.len(), 2);
    assert_eq!(b.clamp_boundary(1), 0, "middle of a 2-byte char snaps back");
    assert_eq!(b.clamp_boundary(2), 2);
}

#[test]
fn original_file_is_sliced_into_bounded_pieces() {
    let bytes = vec![b'a'; CHUNK_CAP * 3 + 7];
    let b = Buffer::from_bytes(bytes).unwrap();
    assert_eq!(b.len(), CHUNK_CAP * 3 + 7);
    assert_eq!(
        b.piece_count(),
        4,
        "a file loads as ceil(size / CHUNK_CAP) pieces so splits stay bounded"
    );
}

#[test]
fn random_edits_match_a_naive_string_model() {
    let mut rng = Rng::new(0xC0FFEE);
    let mut b = Buffer::empty();
    let mut model = String::new();

    // A mixed alphabet including multi-byte characters, newlines and CRLF, so the
    // line index and the character-boundary logic are both exercised.
    let atoms: &[&str] = &[
        "a", "b", "Z", " ", "\n", "\r\n", "\t", "é", "漢", "😀", "ß", "Ω", "x\ny",
    ];

    for step in 0..4000 {
        // Work in character boundaries so both sides stay valid UTF-8.
        let boundaries: Vec<usize> = {
            let mut v = Vec::new();
            let mut p = 0;
            while p <= model.len() {
                if model.is_char_boundary(p) {
                    v.push(p);
                }
                p += 1;
            }
            v
        };
        let pick = |rng: &mut Rng| boundaries[rng.below(boundaries.len())];

        match rng.below(10) {
            0..=4 => {
                // Insert.
                let at = pick(&mut rng);
                let atom = atoms[rng.below(atoms.len())];
                b.insert(at, atom.as_bytes()).unwrap();
                model.insert_str(at, atom);
            }
            5..=6 => {
                // Delete a range.
                let a = pick(&mut rng);
                let c = pick(&mut rng);
                let (from, to) = if a <= c { (a, c) } else { (c, a) };
                b.remove(from, to);
                model.replace_range(from..to, "");
            }
            _ => {
                // Replace a range with new text.
                let a = pick(&mut rng);
                let c = pick(&mut rng);
                let (from, to) = if a <= c { (a, c) } else { (c, a) };
                let atom = atoms[rng.below(atoms.len())];
                b.replace_capture(from, to, atom.as_bytes()).unwrap();
                model.replace_range(from..to, atom);
            }
        }

        // Structural check on every step: a mishandled split/merge shows up here
        // immediately rather than as a mysterious corruption much later.
        b.check_invariants();

        assert_eq!(
            b.len(),
            model.len(),
            "length diverged at step {step} (bytes {b:?} vs {model:?})"
        );
        assert_eq!(buf_text(&b), model, "content diverged at step {step}");

        // The line index must agree with the reference on every line.
        let refs = reference_lines(&model);
        assert_eq!(
            b.line_count(),
            refs.len(),
            "line count diverged at step {step}: {model:?}"
        );
        for (line, &(start, content_end, full_end)) in refs.iter().enumerate() {
            assert_eq!(b.line_start(line), start, "line {line} start, step {step}");
            assert_eq!(b.line_span(line), (start, content_end), "line {line} span, step {step}");
            assert_eq!(
                b.line_full_span(line),
                (start, full_end),
                "line {line} full span, step {step}"
            );
        }

        // And the reverse mapping must agree too. The reference counts newline
        // *bytes*, so it is well defined at offsets that are not character
        // boundaries — which is exactly what the byte-offset API must tolerate.
        let model_bytes = model.as_bytes();
        for pos in 0..=model.len() {
            let expected = model_bytes[..pos].iter().filter(|&&c| c == b'\n').count();
            assert_eq!(b.line_of_byte(pos), expected, "line_of_byte({pos}) at step {step}");
        }

        // Never leave the buffer holding invalid UTF-8.
        assert!(std::str::from_utf8(&b.read_vec(0, b.len())).is_ok());
    }
}

#[test]
fn repeated_appends_keep_exact_length_and_order() {
    // Regression test for a split bug that duplicated nodes: appending at the very
    // end of the document re-split at a piece boundary, and the tree ended up
    // sharing a child between two subtrees. Length grew faster than the text, the
    // piece list came back out of order, and the tree eventually became cyclic.
    let mut b = Buffer::empty();
    let mut model = String::new();
    for i in 0..200 {
        let t = format!("{i:02}");
        b.insert(b.len(), t.as_bytes()).unwrap();
        model.push_str(&t);

        b.check_invariants();
        assert_eq!(b.len(), model.len(), "length diverged at i={i}");
        assert_eq!(b.line_count(), 1);
        assert_eq!(buf_text(&b), model, "content diverged at i={i}");

        // The flattened piece list (used by snapshots and therefore by saving and
        // searching) must be in document order too.
        let flat: String = b
            .flatten()
            .iter()
            .map(|p| {
                String::from_utf8(b.read_vec(p.start as usize, (p.start + p.len) as usize)).unwrap()
            })
            .collect();
        assert_eq!(flat, model, "piece order diverged at i={i}");
    }
}

#[test]
fn piece_stream_reassembles_the_document() {
    let mut b = Buffer::empty();
    b.insert(0, b"0123456789").unwrap();
    b.insert(5, b"ABCDE").unwrap();
    b.remove(2, 12);
    let expected = buf_text(&b);

    let mut streamed = Vec::new();
    let mut s = b.stream_from(0);
    while let Some(chunk) = s.next_slice() {
        streamed.extend_from_slice(chunk);
    }
    assert_eq!(String::from_utf8(streamed).unwrap(), expected);

    // Starting from an offset must yield the same tail.
    for start in [0usize, 1, 3, 7, b.len()] {
        let mut got = Vec::new();
        let mut s = b.stream_from(start);
        while let Some(chunk) = s.next_slice() {
            got.extend_from_slice(chunk);
        }
        assert_eq!(
            String::from_utf8(got).unwrap(),
            &expected[start.min(expected.len())..],
            "stream from {start}"
        );
    }
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

#[test]
fn snapshot_is_isolated_from_later_edits() {
    let mut b = Buffer::empty();
    b.insert(0, b"original content").unwrap();
    let snap = b.snapshot();

    // Mutate the live buffer in ways that force copy-on-write and piece splits.
    b.insert(8, b"INSERTED ").unwrap();
    b.remove(0, 3);
    b.insert(b.len(), b" tail").unwrap();

    assert_eq!(snap_text(&snap), "original content", "snapshot must not change");
    // Inserting at offset 8 lands just before the space in "original content", and
    // the inserted text itself ends with a space, so two spaces meet there.
    assert_eq!(buf_text(&b), "ginalINSERTED  content tail");

    // The snapshot's own index must still describe its own bytes.
    assert_eq!(snap.len(), "original content".len());
    assert_eq!(snap.line_count(), 1);
    assert_eq!(snap.byte_at(0), Some(b'o'));
}

#[test]
fn snapshot_survives_many_edits_to_its_chunks() {
    // Force repeated appends into the same chunk after the snapshot was taken.
    let mut b = Buffer::empty();
    b.insert(0, b"seed").unwrap();
    let snap = b.snapshot();
    for i in 0..2000 {
        b.insert(b.len(), format!("{i},").as_bytes()).unwrap();
    }
    assert_eq!(snap_text(&snap), "seed");
    assert!(buf_text(&b).starts_with("seed0,1,2,"));
}

// ---------------------------------------------------------------------------
// Undo / redo
// ---------------------------------------------------------------------------

fn doc_with(text: &str) -> Document {
    let mut b = Buffer::empty();
    b.insert(0, text.as_bytes()).unwrap();
    let mut d = Document::new(1, b, None, Eol::Lf, false);
    d.text.history.reset();
    d
}

#[test]
fn undo_restores_text_cursor_and_selection() {
    let mut d = doc_with("hello world");
    d.set_caret(5);
    d.insert_str(" there", EditKind::Paste).unwrap();
    assert_eq!(buf_text(&d.text.buffer), "hello there world");

    assert!(d.undo());
    assert_eq!(buf_text(&d.text.buffer), "hello world");
    assert_eq!(d.text.sel, Sel::caret(5), "cursor returns to where it was");

    // A selection is restored too.
    d.set_sel(Sel { anchor: 0, caret: 5 });
    d.insert_str("HELLO", EditKind::Paste).unwrap();
    assert_eq!(buf_text(&d.text.buffer), "HELLO world");
    assert!(d.undo());
    assert_eq!(buf_text(&d.text.buffer), "hello world");
    assert_eq!(d.text.sel, Sel { anchor: 0, caret: 5 });
}

#[test]
fn typing_groups_into_one_undo_step() {
    let mut d = doc_with("");
    for ch in "hello".chars() {
        d.insert_str(&ch.to_string(), EditKind::Typing).unwrap();
    }
    assert_eq!(buf_text(&d.text.buffer), "hello");
    assert_eq!(d.text.history.undo_depth(), 1, "consecutive typing is one step");

    assert!(d.undo());
    assert_eq!(buf_text(&d.text.buffer), "", "one undo removes the whole run");
}

#[test]
fn a_newline_breaks_the_typing_group() {
    let mut d = doc_with("");
    d.insert_str("abc", EditKind::Typing).unwrap();
    d.insert_newline().unwrap();
    d.insert_str("def", EditKind::Typing).unwrap();
    assert_eq!(buf_text(&d.text.buffer), "abc\ndef");
    assert_eq!(d.text.history.undo_depth(), 3, "abc | newline | def");

    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "abc\n");
    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "abc");
    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "");
}

#[test]
fn a_cursor_move_breaks_the_typing_group() {
    let mut d = doc_with("");
    d.insert_str("abc", EditKind::Typing).unwrap();
    d.set_caret(0); // an explicit move closes the group
    d.insert_str("xyz", EditKind::Typing).unwrap();
    assert_eq!(buf_text(&d.text.buffer), "xyzabc");
    assert_eq!(d.text.history.undo_depth(), 2);

    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "abc");
}

#[test]
fn backspace_groups_and_undoes_as_one_run() {
    let mut d = doc_with("abcdef");
    d.set_caret(6);
    for _ in 0..3 {
        assert!(d.backspace());
    }
    assert_eq!(buf_text(&d.text.buffer), "abc");
    assert_eq!(d.text.history.undo_depth(), 1);

    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "abcdef");
    assert_eq!(d.text.sel, Sel::caret(6));
}

#[test]
fn forward_delete_groups_and_undoes_as_one_run() {
    let mut d = doc_with("abcdef");
    d.set_caret(0);
    for _ in 0..3 {
        assert!(d.delete_forward());
    }
    assert_eq!(buf_text(&d.text.buffer), "def");
    assert_eq!(d.text.history.undo_depth(), 1);
    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "abcdef");
}

#[test]
fn paste_is_a_single_undo_step() {
    let mut d = doc_with("start|end");
    d.set_caret(6);
    d.insert_str("A\nB\nC\nD\nE", EditKind::Paste).unwrap();
    assert_eq!(buf_text(&d.text.buffer), "start|A\nB\nC\nD\nEend");
    assert_eq!(d.text.history.undo_depth(), 1, "paste is never split");
    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "start|end");
}

#[test]
fn editing_after_undo_discards_the_redo_branch() {
    let mut d = doc_with("");
    d.insert_str("one", EditKind::Paste).unwrap();
    d.insert_str("two", EditKind::Paste).unwrap();
    assert!(d.undo());
    assert_eq!(buf_text(&d.text.buffer), "one");
    assert!(d.text.history.can_redo());

    d.insert_str("three", EditKind::Paste).unwrap();
    assert!(
        !d.text.history.can_redo(),
        "a new edit must abandon the redo branch"
    );
    assert!(!d.redo());
    assert_eq!(buf_text(&d.text.buffer), "onethree");
}

#[test]
fn redo_restores_text_and_selection() {
    let mut d = doc_with("hello world");
    d.set_sel(Sel { anchor: 6, caret: 11 });
    d.insert_str("there", EditKind::Paste).unwrap();
    assert_eq!(buf_text(&d.text.buffer), "hello there");

    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "hello world");
    assert!(d.redo());
    assert_eq!(buf_text(&d.text.buffer), "hello there");
    assert_eq!(d.text.sel, Sel::caret(11));
}

#[test]
fn saved_state_tracks_undo_and_redo() {
    let mut d = doc_with("");
    assert!(!d.is_dirty(), "a fresh document is clean");

    d.insert_str("hello", EditKind::Typing).unwrap();
    assert!(d.is_dirty());

    d.mark_saved();
    assert!(!d.is_dirty(), "saving clears the indicator");
    assert!(
        d.text.history.can_undo(),
        "saving must not clear the undo history"
    );

    // Undoing past the save point makes it dirty again.
    d.undo();
    assert!(d.is_dirty());
    assert_eq!(buf_text(&d.text.buffer), "");

    // Redoing back onto the saved state clears it again.
    d.redo();
    assert!(!d.is_dirty(), "redo back to the saved state clears the indicator");
    assert_eq!(buf_text(&d.text.buffer), "hello");
}

#[test]
fn undo_to_the_saved_state_clears_the_indicator_across_many_edits() {
    let mut d = doc_with("");
    for ch in "abc".chars() {
        d.insert_str(&ch.to_string(), EditKind::Typing).unwrap();
    }
    d.mark_saved();
    for ch in "def".chars() {
        d.insert_str(&ch.to_string(), EditKind::Typing).unwrap();
    }
    assert!(d.is_dirty());
    assert_eq!(d.text.history.undo_depth(), 2, "abc | def");

    d.undo();
    assert!(
        !d.is_dirty(),
        "undoing back to the saved state must clear the indicator"
    );
    assert_eq!(buf_text(&d.text.buffer), "abc");
}

#[test]
fn save_during_editing_keeps_later_changes_dirty() {
    // Mirrors what the app does: capture the serial, write a snapshot, then record
    // that serial as saved.
    let mut d = doc_with("base");
    d.set_caret(4);
    d.insert_str("1", EditKind::Typing).unwrap();
    // Exactly what the app does on Ctrl+S.
    let (snap, serial) = d.snapshot_for_save();

    // The user keeps typing while the write is in flight.
    d.insert_str("2", EditKind::Typing).unwrap();

    d.mark_saved_at(serial);
    assert!(
        d.is_dirty(),
        "edits made during the save must remain unsaved"
    );
    assert_eq!(snap_text(&snap), "base1", "the file holds the snapshot");

    // Undoing back to what was written clears the indicator.
    d.undo();
    assert!(!d.is_dirty());
    assert_eq!(buf_text(&d.text.buffer), "base1");

    // The snapshot's serial names a real undo step: redoing returns to the state
    // that was written, and the document is clean again.
    assert!(d.redo());
    assert_eq!(buf_text(&d.text.buffer), "base12");
    assert!(d.is_dirty(), "the later edits are not in the file");
}

#[test]
fn snapshot_for_save_separates_typing_into_distinct_undo_steps() {
    // Without the group break, "type, save, type" would merge into one step and
    // undoing it could never land on the saved state.
    let mut d = doc_with("");
    d.insert_str("aaa", EditKind::Typing).unwrap();
    let (_snap, serial) = d.snapshot_for_save();
    d.insert_str("bbb", EditKind::Typing).unwrap();
    d.mark_saved_at(serial);

    assert!(d.is_dirty());
    assert_eq!(d.text.history.undo_depth(), 2, "save is an undo boundary");

    d.undo();
    assert!(!d.is_dirty(), "undoing to the saved state clears the indicator");
    assert_eq!(buf_text(&d.text.buffer), "aaa");
}

#[test]
fn history_budget_evicts_oldest_steps_only() {
    let mut b = Buffer::empty();
    let mut h = History::new(64);
    for i in 0..20 {
        let text = format!("{i:02}");
        let delta = b.insert(b.len(), text.as_bytes()).unwrap();
        let sel = Sel::caret(b.len());
        h.record(notepad::undo::make_edit(
            0,
            delta.removed,
            delta.removed_len,
            delta.inserted,
            delta.inserted_len,
            sel,
            sel,
            EditKind::Paste,
        ));
    }
    assert!(h.retained_bytes() <= 64, "history must respect its budget");
    assert!(h.can_undo(), "recent steps stay available");
    // The newest edit is always undoable.
    let before = b.len();
    h.undo(&mut b);
    assert!(b.len() < before);
}

// ---------------------------------------------------------------------------
// Document-level editing
// ---------------------------------------------------------------------------

#[test]
fn typing_replaces_the_selection() {
    let mut d = doc_with("hello world");
    d.set_sel(Sel { anchor: 6, caret: 11 });
    d.insert_str("there", EditKind::Typing).unwrap();
    assert_eq!(buf_text(&d.text.buffer), "hello there");
    assert_eq!(d.text.sel, Sel::caret(11));
    assert_eq!(d.text.history.undo_depth(), 1);
}

#[test]
fn cut_returns_text_and_is_one_undo_step() {
    let mut d = doc_with("hello world");
    d.set_sel(Sel { anchor: 0, caret: 6 });
    let cut = d.cut().expect("selection present");
    assert_eq!(cut, "hello ");
    assert_eq!(buf_text(&d.text.buffer), "world");
    assert_eq!(d.text.history.undo_depth(), 1);
    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "hello world");
}

#[test]
fn backspace_at_the_start_does_nothing() {
    let mut d = doc_with("abc");
    d.set_caret(0);
    assert!(!d.backspace());
    assert_eq!(buf_text(&d.text.buffer), "abc");
    assert!(!d.text.history.can_undo());
}

#[test]
fn delete_at_the_end_does_nothing() {
    let mut d = doc_with("abc");
    d.set_caret(3);
    assert!(!d.delete_forward());
    assert_eq!(buf_text(&d.text.buffer), "abc");
}

#[test]
fn backspace_deletes_a_whole_multibyte_character() {
    let mut d = doc_with("aé漢");
    d.set_caret(d.text.buffer.len());
    assert!(d.backspace());
    assert_eq!(buf_text(&d.text.buffer), "aé");
    assert!(d.backspace());
    assert_eq!(buf_text(&d.text.buffer), "a");
}

#[test]
fn select_all_then_type_replaces_everything() {
    let mut d = doc_with("one\ntwo\nthree");
    d.select_all();
    d.insert_str("done", EditKind::Typing).unwrap();
    assert_eq!(buf_text(&d.text.buffer), "done");
    assert_eq!(d.text.history.undo_depth(), 1);
    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "one\ntwo\nthree");
}

#[test]
fn cursor_line_col_is_one_based() {
    let mut d = doc_with("ab\ncdef\n\nx");
    d.set_caret(0);
    assert_eq!(d.cursor_line_col(), (1, 1));
    d.set_caret(3);
    assert_eq!(d.cursor_line_col(), (2, 1));
    d.set_caret(6);
    assert_eq!(d.cursor_line_col(), (2, 4));
    d.set_caret(8);
    assert_eq!(d.cursor_line_col(), (3, 1));
    d.set_caret(9);
    assert_eq!(d.cursor_line_col(), (4, 1));
}

#[test]
fn document_keeps_its_own_state() {
    // Two documents must not share cursor, history or dirty flag.
    let mut a = doc_with("alpha");
    let mut b = doc_with("beta");
    a.set_caret(5);
    b.set_caret(0);
    a.insert_str("!", EditKind::Typing).unwrap();

    assert_eq!(buf_text(&a.text.buffer), "alpha!");
    assert_eq!(buf_text(&b.text.buffer), "beta");
    assert!(a.is_dirty());
    assert!(!b.is_dirty());
    assert_eq!(a.text.sel.caret, 6);
    assert_eq!(b.text.sel.caret, 0);
    assert!(a.text.history.can_undo());
    assert!(!b.text.history.can_undo());
}

// ---------------------------------------------------------------------------
// Motions
// ---------------------------------------------------------------------------

#[test]
fn word_boundaries_are_reasonable() {
    let b = {
        let mut b = Buffer::empty();
        b.insert(0, b"alpha beta_gamma  delta.delta\nnext").unwrap();
        b
    };
    // Forward from the start of "alpha" lands on "beta_gamma".
    assert_eq!(motion::next_word_boundary(&b, 0), 6);
    // Forward from inside "alpha" also lands on the next word.
    assert_eq!(motion::next_word_boundary(&b, 2), 6);
    // Forward from the start of "beta_gamma" skips the whitespace run.
    assert_eq!(motion::next_word_boundary(&b, 6), 18);
    // Backward from the end of "alpha" lands at its start.
    assert_eq!(motion::prev_word_boundary(&b, 5), 0);
    assert_eq!(motion::prev_word_boundary(&b, 6), 0);
    assert_eq!(motion::prev_word_boundary(&b, 11), 6);
}

#[test]
fn word_at_selects_the_word_under_the_caret() {
    let b = {
        let mut b = Buffer::empty();
        b.insert(0, b"one two three").unwrap();
        b
    };
    assert_eq!(motion::word_at(&b, 1), (0, 3));
    assert_eq!(motion::word_at(&b, 4), (4, 7));
    assert_eq!(motion::word_at(&b, 12), (8, 13));
    // Whitespace is its own run, so double-clicking a space does not eat words.
    assert_eq!(motion::word_at(&b, 3), (3, 4));
}

#[test]
fn home_prefers_first_non_blank_then_column_zero() {
    let b = {
        let mut b = Buffer::empty();
        b.insert(0, b"    indented\nplain").unwrap();
        b
    };
    assert_eq!(motion::line_home(&b, 8), 4, "first press goes to first non-blank");
    assert_eq!(motion::line_home(&b, 4), 0, "second press goes to column 0");
    assert_eq!(motion::line_home(&b, 13), 13, "a line with no indent stays put");
}

#[test]
fn line_end_excludes_the_terminator() {
    let b = {
        let mut b = Buffer::empty();
        b.insert(0, b"abc\ndef\r\nghi").unwrap();
        b
    };
    // Lines are "abc" (0..3), "def" (4..7) and "ghi" (9..12); the byte at 7 and 8
    // is the CRLF that terminates line 1.
    assert_eq!(motion::line_end(&b, 0), 3);
    assert_eq!(motion::line_end(&b, 1), 3);
    assert_eq!(motion::line_end(&b, 3), 3, "the terminator belongs to its own line");
    assert_eq!(motion::line_end(&b, 4), 7);
    assert_eq!(motion::line_end(&b, 7), 7, "CRLF line ends before the CR");
    assert_eq!(motion::line_end(&b, 8), 7, "the LF is still on line 1");
    assert_eq!(motion::line_end(&b, 9), 12);
    assert_eq!(motion::line_end(&b, 12), 12, "the last line has no terminator");
}

#[test]
fn vertical_movement_keeps_the_desired_column() {
    // This is the mechanism behind Up/Down and PageUp/PageDown: the caret's column
    // is remembered, and a short line only clamps it temporarily.
    let b = {
        let mut b = Buffer::empty();
        b.insert(0, b"abcdefghijklmnop
short
abcdefghijklmnop").unwrap();
        b
    };
    let desired = motion::column_of(&b, 10);
    assert_eq!(desired, 10);

    // Down onto the short line clamps to its end...
    let on_short = motion::offset_at_column(&b, 1, desired);
    assert_eq!(motion::column_of(&b, on_short), 5);

    // ...but moving on with the remembered column returns to column 10.
    let back = motion::offset_at_column(&b, 2, desired);
    assert_eq!(motion::column_of(&b, back), 10);

    // The same holds for a page-sized jump.
    assert_eq!(motion::column_of(&b, motion::offset_at_column(&b, 2, desired)), 10);
}

#[test]
fn columns_round_trip_across_short_and_long_lines() {
    let b = {
        let mut b = Buffer::empty();
        b.insert(0, "a\nlonger line here\né漢x\n".as_bytes()).unwrap();
        b
    };
    for line in 0..b.line_count() {
        let (start, end) = b.line_span(line);
        let mut p = start;
        let mut col = 0usize;
        while p <= end {
            assert_eq!(motion::offset_at_column(&b, line, col), p.min(end));
            if p == end {
                break;
            }
            p = b.next_char(p);
            col += 1;
        }
    }
    // Asking for a column past the end clamps to the line end.
    assert_eq!(motion::offset_at_column(&b, 0, 99), 1);
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

fn search_all(text: &str, query: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    let b = {
        let mut b = Buffer::empty();
        b.insert(0, text.as_bytes()).unwrap();
        b
    };
    let snap = b.snapshot();
    let q = Query::new(query, case_sensitive).expect("non-empty query");
    search::find_all(&snap, &q)
        .into_iter()
        .map(|m| (m.start, m.end))
        .collect()
}

#[test]
fn literal_search_finds_all_occurrences() {
    assert_eq!(
        search_all("abcabcabc", "abc", true),
        vec![(0, 3), (3, 6), (6, 9)]
    );
    // Matches do not overlap.
    assert_eq!(search_all("aaaa", "aa", true), vec![(0, 2), (2, 4)]);
}

#[test]
fn search_respects_case_sensitivity() {
    assert_eq!(search_all("Foo foo FOO", "foo", true), vec![(4, 7)]);
    assert_eq!(
        search_all("Foo foo FOO", "foo", false),
        vec![(0, 3), (4, 7), (8, 11)]
    );
}

#[test]
fn empty_query_produces_no_matches_and_no_error() {
    assert!(Query::new("", true).is_none());
    assert!(Query::new("", false).is_none());

    let b = {
        let mut b = Buffer::empty();
        b.insert(0, b"some text").unwrap();
        b
    };
    let snap = b.snapshot();
    // An empty query must not be constructible, so there is nothing to run.
    assert!(Query::new("", true).is_none());
    // And a scan with no matches reports completion.
    let progress = Progress::new();
    let q = Query::new("zzz", true).unwrap();
    let mut n = 0;
    assert!(search::scan(&snap, &q, &progress, |_| n += 1));
    assert_eq!(n, 0);
}

#[test]
fn search_never_matches_inside_a_multibyte_character() {
    // "é" is C3 A9; searching for a lone continuation byte must not match.
    let text = "aé b漢 c😀";
    assert_eq!(search_all(text, "é", true), vec![(1, 3)]);
    assert_eq!(search_all(text, "漢", true), vec![(5, 8)]);
    assert_eq!(search_all(text, "😀", true), vec![(10, 14)]);

    // A query that only exists as a byte-subsequence of a character must not match.
    assert!(search_all(text, "\u{A9}", true).is_empty());
    assert!(search_all("漢", "\u{6f}", true).is_empty());
}

#[test]
fn search_spans_the_window_boundary() {
    // Build text large enough to exceed the internal 8 MiB window, with a match
    // placed exactly across the seam.
    let window = 8usize << 20;
    let mut text = vec![b'x'; window - 2];
    text.extend_from_slice(b"NEEDLE");
    text.extend_from_slice(&[b'y'; 64]);
    let mut b = Buffer::empty();
    b.insert(0, &text).unwrap();
    let snap = b.snapshot();
    let q = Query::new("NEEDLE", true).unwrap();
    let found = search::find_all(&snap, &q);
    assert_eq!(found.len(), 1, "a match straddling the scan window must be found");
    assert_eq!(found[0].start, window - 2);
}

#[test]
fn search_matches_across_piece_boundaries() {
    let mut b = Buffer::empty();
    b.insert(0, b"prefix").unwrap();
    b.insert(6, b"-MIDDLE-").unwrap();
    b.insert(14, b"suffix").unwrap();
    let snap = b.snapshot();
    let q = Query::new("prefix-MIDDLE-suffix", true).unwrap();
    let found = search::find_all(&snap, &q);
    assert_eq!(found.len(), 1, "a match spanning several pieces must be found");
    assert_eq!((found[0].start, found[0].end), (0, b.len()));
}

#[test]
fn search_is_cancellable() {
    let mut b = Buffer::empty();
    b.insert(0, &vec![b'a'; 32 << 20]).unwrap();
    let snap = b.snapshot();
    let q = Query::new("zzz", true).unwrap();

    let progress = Progress::new();
    progress.cancel();
    let mut n = 0;
    let completed = search::scan(&snap, &q, &progress, |_| n += 1);
    assert!(!completed, "a cancelled scan must report that it did not finish");
    assert_eq!(n, 0);
}

#[test]
fn match_navigation_helpers_wrap_and_locate() {
    let b = {
        let mut b = Buffer::empty();
        b.insert(0, b"aa bb aa bb aa").unwrap();
        b
    };
    let snap = b.snapshot();
    let q = Query::new("aa", true).unwrap();
    let m = search::find_all(&snap, &q);
    assert_eq!(m.len(), 3);

    assert_eq!(search::containing(&m, 1), Some(0));
    assert_eq!(search::containing(&m, 7), Some(1));
    assert_eq!(search::containing(&m, 3), None, "in the gap between matches");

    assert_eq!(search::first_after(&m, 0), Some(1));
    assert_eq!(search::first_after(&m, 8), Some(2));
    assert_eq!(search::first_after(&m, 13), None, "no match after the last one");
    assert_eq!(search::last_before(&m, 8), Some(1));
    assert_eq!(search::last_before(&m, 0), None);

    // Current-match resolution prefers containment, then the next one, then wraps.
    assert_eq!(search::current_for(&m, 7), Some(1));
    assert_eq!(search::current_for(&m, 4), Some(1));
    assert_eq!(search::current_for(&m, 999), Some(0), "wraps to the first match");
    assert_eq!(search::current_for(&[], 0), None);
}

#[test]
fn replace_plan_covers_the_document_exactly() {
    let b = {
        let mut b = Buffer::empty();
        b.insert(0, b"one two one two one").unwrap();
        b
    };
    let snap = b.snapshot();
    let q = Query::new("one", true).unwrap();
    let matches = search::find_all(&snap, &q);
    assert_eq!(matches.len(), 3);

    let progress = Progress::new();
    let plan = search::build_replace_plan(&snap, &matches, b"1", &progress).expect("not cancelled");

    // Reassembling the plan must reproduce the expected document.
    let mut out = Vec::new();
    for seg in &plan {
        match seg {
            search::Segment::Keep(p) => out.extend_from_slice(snap.slice_of(*p)),
            search::Segment::Insert(bytes) => out.extend_from_slice(bytes),
        }
    }
    assert_eq!(String::from_utf8(out).unwrap(), "1 two 1 two 1");
}

#[test]
fn replace_plan_handles_a_match_spanning_pieces() {
    let mut b = Buffer::empty();
    b.insert(0, b"AAA").unwrap();
    b.insert(3, b"TARGET").unwrap();
    b.insert(9, b"BBB").unwrap();
    let snap = b.snapshot();
    let q = Query::new("TARGET", true).unwrap();
    let matches = search::find_all(&snap, &q);
    assert_eq!(matches.len(), 1);

    let progress = Progress::new();
    let plan = search::build_replace_plan(&snap, &matches, b"X", &progress).unwrap();
    let mut out = Vec::new();
    for seg in &plan {
        match seg {
            search::Segment::Keep(p) => out.extend_from_slice(snap.slice_of(*p)),
            search::Segment::Insert(bytes) => out.extend_from_slice(bytes),
        }
    }
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "AAAXBBB",
        "the replacement must appear exactly once, not once per piece it spans"
    );
}

#[test]
fn replace_all_through_the_document_is_one_undo_step() {
    let mut d = doc_with("cat dog cat bird cat");
    d.search.query = "cat".to_string();
    d.search.replacement = "fox".to_string();
    let snap = d.snapshot();
    let q = Query::new("cat", true).unwrap();
    let matches = search::find_all(&snap, &q);
    assert_eq!(matches.len(), 3);

    let progress = Progress::new();
    let plan = search::build_replace_plan(&snap, &matches, b"fox", &progress).unwrap();
    d.apply_replace_all(plan, matches.len());

    assert_eq!(buf_text(&d.text.buffer), "fox dog fox bird fox");
    assert!(d.is_dirty());
    assert_eq!(d.text.history.undo_depth(), 1, "replace-all is a single step");

    d.undo();
    assert_eq!(buf_text(&d.text.buffer), "cat dog cat bird cat");
    assert!(!d.is_dirty(), "undoing replace-all returns to the clean state");
}

#[test]
fn replace_all_preserves_line_structure() {
    let mut d = doc_with("x\nfoo\nfoo\nx");
    let snap = d.snapshot();
    let q = Query::new("foo", true).unwrap();
    let matches = search::find_all(&snap, &q);
    let progress = Progress::new();
    let plan = search::build_replace_plan(&snap, &matches, b"bar", &progress).unwrap();
    d.apply_replace_all(plan, matches.len());
    assert_eq!(buf_text(&d.text.buffer), "x\nbar\nbar\nx");
    assert_eq!(d.line_count(), 4);
}

// ---------------------------------------------------------------------------
// File I/O
// ---------------------------------------------------------------------------

#[test]
fn load_rejects_invalid_utf8_with_an_offset() {
    let dir = scratch("utf8");
    // Valid prefix, then a stray continuation byte.
    let mut bytes = b"valid text\n".to_vec();
    bytes.push(0x80);
    bytes.extend_from_slice(b"more");
    let p = write_file(&dir, "bad.txt", &bytes);

    let progress = Progress::new();
    let err = notepad::fileio::load(&p, &progress).expect_err("must be rejected");
    match err {
        notepad::fileio::FileError::NotUtf8 { offset, .. } => {
            assert_eq!(offset, 11, "offset points at the first invalid byte");
        }
        other => panic!("expected NotUtf8, got {other}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_rejects_truncated_and_overlong_sequences() {
    let dir = scratch("utf8b");
    let cases: &[(&str, &[u8])] = &[
        ("truncated2.txt", &[b'a', 0xC3]),
        ("truncated3.txt", &[b'a', 0xE2, 0x82]),
        ("truncated4.txt", &[b'a', 0xF0, 0x9F, 0x98]),
        ("overlong.txt", &[b'a', 0xC0, 0xAF]),
        ("stray_ff.txt", &[b'a', 0xFF]),
        ("lone_cont.txt", &[b'a', 0x80, 0x80]),
    ];
    let progress = Progress::new();
    for (name, bytes) in cases {
        let p = write_file(&dir, name, bytes);
        assert!(
            notepad::fileio::load(&p, &progress).is_err(),
            "{name} must be rejected as invalid UTF-8"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_accepts_valid_utf8_and_reports_size() {
    let dir = scratch("utf8ok");
    let text = "héllo 漢字 😀\nsecond line\n";
    let p = write_file(&dir, "ok.txt", text.as_bytes());
    let progress = Progress::new();
    let loaded = notepad::fileio::load(&p, &progress).expect("valid UTF-8 loads");
    assert_eq!(loaded.size, text.len() as u64);
    assert!(!loaded.had_bom);
    assert_eq!(buf_text(&loaded.buffer), text);
    assert_eq!(loaded.buffer.line_count(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bom_is_detected_and_round_trips() {
    let dir = scratch("bom");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"Hello BOM\n");
    let p = write_file(&dir, "bom.txt", &bytes);

    let progress = Progress::new();
    let loaded = notepad::fileio::load(&p, &progress).expect("BOM'd UTF-8 loads");
    assert!(loaded.had_bom, "the BOM must be remembered");
    assert_eq!(buf_text(&loaded.buffer), "Hello BOM\n", "the BOM is not document text");

    // Saving it back must reproduce the original bytes exactly.
    let out = dir.join("out.txt");
    notepad::fileio::save_atomic(&out, &progress, |w| {
        notepad::fileio::write_buffer(w, &loaded.buffer, true, &progress)
    })
    .expect("save succeeds");
    assert_eq!(std::fs::read(&out).unwrap(), bytes, "BOM round-trip must be byte-exact");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn crlf_is_detected_and_preserved_on_save() {
    let dir = scratch("crlf");
    let text = "alpha\r\nbeta\r\ngamma\r\n";
    let p = write_file(&dir, "crlf.txt", text.as_bytes());

    let progress = Progress::new();
    let loaded = notepad::fileio::load(&p, &progress).unwrap();
    assert_eq!(detect_eol(text.as_bytes()), Eol::Crlf);

    let out = dir.join("out.txt");
    notepad::fileio::save_atomic(&out, &progress, |w| {
        notepad::fileio::write_buffer(w, &loaded.buffer, false, &progress)
    })
    .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), text.as_bytes(), "CRLF preserved byte-for-byte");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn eol_detection_distinguishes_lf_crlf_and_mixed() {
    assert_eq!(detect_eol(b"a\nb\n"), Eol::Lf);
    assert_eq!(detect_eol(b"a\r\nb\r\n"), Eol::Crlf);
    assert_eq!(detect_eol(b"a\r\nb\n"), Eol::Mixed);
    assert_eq!(detect_eol(b"no newlines"), Eol::Lf);
    assert_eq!(detect_eol(b""), Eol::Lf);
}

#[test]
fn failed_save_leaves_the_original_file_intact() {
    let dir = scratch("atomic");
    let original = b"important original content\n";
    let p = write_file(&dir, "keep.txt", original);

    let progress = Progress::new();
    // The source fails partway through, simulating a full disk or a write error.
    let result = notepad::fileio::save_atomic(&p, &progress, |w| {
        w.write_all(b"partial garbage")?;
        Err(std::io::Error::other("simulated failure"))
    });
    assert!(result.is_err(), "the failure must be reported");

    assert_eq!(
        std::fs::read(&p).unwrap(),
        original,
        "a failed write must not truncate or modify the original file"
    );

    // No stray temporary files may be left behind.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp files must be cleaned up: {leftovers:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn successful_save_replaces_the_file_and_leaves_no_temp() {
    let dir = scratch("atomic2");
    let p = write_file(&dir, "doc.txt", b"old content that is longer\n");

    let mut b = Buffer::empty();
    b.insert(0, b"new\n").unwrap();
    let progress = Progress::new();
    let written = notepad::fileio::save_atomic(&p, &progress, |w| {
        notepad::fileio::write_buffer(w, &b, false, &progress)
    })
    .expect("save succeeds");

    assert_eq!(written, 4);
    assert_eq!(std::fs::read(&p).unwrap(), b"new\n");
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "no temp files after a successful save");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_a_large_document_round_trips_exactly() {
    let dir = scratch("large");
    // Mixed content with multi-byte characters and many lines.
    let mut text = String::new();
    for i in 0..20_000 {
        text.push_str(&format!("line {i} — héllo 漢字 😀\n"));
    }
    let p = write_file(&dir, "large.txt", text.as_bytes());

    let progress = Progress::new();
    let loaded = notepad::fileio::load(&p, &progress).unwrap();
    assert_eq!(loaded.buffer.line_count(), 20_001);

    let out = dir.join("large_out.txt");
    notepad::fileio::save_atomic(&out, &progress, |w| {
        notepad::fileio::write_buffer(w, &loaded.buffer, false, &progress)
    })
    .unwrap();
    assert_eq!(
        std::fs::read(&out).unwrap(),
        text.as_bytes(),
        "a large round-trip must be byte-exact"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Large-file behaviour
// ---------------------------------------------------------------------------

#[test]
fn million_line_index_is_correct_and_cheap() {
    // Build a document with a million short lines.
    let mut text = String::with_capacity(40_000_000);
    let mut expected_starts = Vec::with_capacity(1_000_001);
    for i in 0..1_000_000u32 {
        expected_starts.push(text.len());
        text.push_str(&format!("line number {i} of the fixture\n"));
    }
    expected_starts.push(text.len());

    let start = std::time::Instant::now();
    let b = Buffer::from_bytes(text.into_bytes()).unwrap();
    let load = start.elapsed();

    assert_eq!(b.line_count(), 1_000_001);
    assert!(
        load < std::time::Duration::from_secs(20),
        "building the index for 1M lines took {load:?}"
    );

    // Spot-check the line index at many positions, including the extremes.
    for &line in &[0usize, 1, 2, 999, 500_000, 999_998, 999_999, 1_000_000] {
        assert_eq!(b.line_start(line), expected_starts[line], "line_start({line})");
    }

    // A keystroke at the end must not touch the whole document.
    let mut b = b;
    let t = std::time::Instant::now();
    for i in 0..200 {
        b.insert(b.len(), format!("{i}").as_bytes()).unwrap();
    }
    let edit = t.elapsed();
    assert!(
        edit < std::time::Duration::from_millis(500),
        "200 edits on a 1M-line document took {edit:?}"
    );

    b.check_invariants();
    // The line index still resolves correctly after edits.
    assert_eq!(b.line_start(999_999), expected_starts[999_999]);
    assert_eq!(
        b.line_count(),
        1_000_001,
        "appending text with no newlines does not add a line"
    );
}

#[test]
fn long_single_line_does_not_cost_the_whole_line() {
    // A 200 KiB line, as in the large fixture.
    let mut text = vec![b'x'; 200_000];
    text.extend_from_slice(b"END");
    let mut b = Buffer::empty();
    b.insert(0, &text).unwrap();

    assert_eq!(b.line_count(), 1);
    assert_eq!(b.line_span(0), (0, 200_003));

    // Reading a slice near the end must not materialise the whole line.
    let t = std::time::Instant::now();
    for _ in 0..10_000 {
        let mut out = [0u8; 16];
        let n = b.read_prefix(199_990, &mut out);
        assert_eq!(n, 13);
        assert_eq!(&out[..3], b"xxx");
    }
    let elapsed = t.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "10k reads near the end of a long line took {elapsed:?}"
    );
}

#[test]
fn edits_near_the_end_of_a_large_document_are_fast() {
    let mut text = String::with_capacity(4 << 20);
    for i in 0..100_000 {
        text.push_str(&format!("row {i}\n"));
    }
    let mut b = Buffer::from_bytes(text.into_bytes()).unwrap();
    let end = b.len();

    let t = std::time::Instant::now();
    for i in 0..1000 {
        b.insert(end, format!("{i}\n").as_bytes()).unwrap();
    }
    let elapsed = t.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "1000 appends at the end took {elapsed:?}"
    );
    assert_eq!(b.line_count(), 101_001);
}

// ---------------------------------------------------------------------------
// Error reporting
// ---------------------------------------------------------------------------

#[test]
fn file_errors_name_the_path_and_reason() {
    let dir = scratch("errs");
    let missing = dir.join("does-not-exist.txt");
    let progress = Progress::new();
    let err = notepad::fileio::load(&missing, &progress).expect_err("missing file");
    let msg = err.to_string();
    assert!(msg.contains("does-not-exist.txt"), "message names the file: {msg}");

    let bad = write_file(&dir, "bad.txt", &[0xFF, 0xFE]);
    let err = notepad::fileio::load(&bad, &progress).expect_err("invalid utf-8");
    let msg = err.to_string();
    assert!(msg.contains("UTF-8"), "message explains the encoding problem: {msg}");
    assert!(msg.contains("bad.txt"), "message names the file: {msg}");
    let _ = std::fs::remove_dir_all(&dir);
}
