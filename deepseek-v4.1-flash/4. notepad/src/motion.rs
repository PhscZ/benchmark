//! Cursor motions, word boundaries and word selection.
//!
//! Everything here works on byte offsets and reads through [`Buffer`], so a
//! motion touches only the bytes it walks over — never the whole document.
//!
//! Scanning is byte-oriented. That is safe for UTF-8 as long as the predicates
//! treat every byte `>= 0x80` as "not whitespace, not punctuation": continuation
//! bytes and lead bytes are then always classified the same way as the character
//! they belong to, so a scan can never stop in the middle of a character.

use crate::buffer::Buffer;

#[inline]
fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r'
}

#[inline]
fn is_word(b: u8) -> bool {
    // `_` counts as a word character, as it does in every mainstream editor, so
    // `snake_case_name` is one word for Ctrl+Left/Right and double-click.
    !is_space(b) && (!is_punct(b) || b == b'_')
}

#[inline]
fn is_punct(b: u8) -> bool {
    b.is_ascii_punctuation()
}

/// First offset at or after `from` whose byte fails `keep`.
fn scan_forward(buf: &Buffer, from: usize, mut keep: impl FnMut(u8) -> bool) -> usize {
    let len = buf.len();
    let mut p = from;
    let mut chunk = [0u8; 8192];
    while p < len {
        let want = (len - p).min(chunk.len());
        let got = buf.read_prefix(p, &mut chunk[..want]);
        if got == 0 {
            break;
        }
        for (i, &b) in chunk[..got].iter().enumerate() {
            if !keep(b) {
                return p + i;
            }
        }
        p += got;
    }
    len
}

/// Offset just past the last byte before `from` that fails `keep`.
fn scan_backward(buf: &Buffer, from: usize, mut keep: impl FnMut(u8) -> bool) -> usize {
    let mut end = from;
    let mut chunk = [0u8; 8192];
    while end > 0 {
        let want = end.min(chunk.len());
        let start = end - want;
        let got = buf.read_prefix(start, &mut chunk[..want]);
        if got == 0 {
            return 0;
        }
        for i in (0..got).rev() {
            if !keep(chunk[i]) {
                return start + i + 1;
            }
        }
        end = start;
    }
    0
}

/// Ctrl+Right: start of the next word.
pub fn next_word_boundary(buf: &Buffer, pos: usize) -> usize {
    let end_of_run = scan_forward(buf, pos, is_word);
    scan_forward(buf, end_of_run, is_space)
}

/// Ctrl+Left: start of the current or previous word.
pub fn prev_word_boundary(buf: &Buffer, pos: usize) -> usize {
    let after_spaces = scan_backward(buf, pos, is_space);
    scan_backward(buf, after_spaces, is_word)
}

/// Offset of the first non-blank character on `pos`'s line, or the line's end.
pub fn first_non_blank(buf: &Buffer, pos: usize) -> usize {
    let line = buf.line_of_byte(pos);
    let (start, end) = buf.line_span(line);
    let p = scan_forward(buf, start, |b| b == b' ' || b == b'\t');
    p.min(end)
}

/// Home: first non-blank, then column 0 on a second press.
pub fn line_home(buf: &Buffer, pos: usize) -> usize {
    let line = buf.line_of_byte(pos);
    let start = buf.line_start(line);
    let f = first_non_blank(buf, pos);
    if pos == f {
        start
    } else {
        f
    }
}

/// End: last character of the line content, excluding the terminator.
pub fn line_end(buf: &Buffer, pos: usize) -> usize {
    let line = buf.line_of_byte(pos);
    buf.line_span(line).1
}

/// The word (or whitespace run, or punctuation run) containing `pos`.
pub fn word_at(buf: &Buffer, pos: usize) -> (usize, usize) {
    let len = buf.len();
    if len == 0 {
        return (0, 0);
    }
    let probe = if pos >= len { buf.prev_char(len) } else { pos };
    let b = match buf.byte_at(probe) {
        Some(b) => b,
        None => return (probe, probe),
    };
    let keep: fn(u8) -> bool = if is_word(b) {
        is_word
    } else if is_space(b) {
        is_space
    } else {
        is_punct
    };
    (
        scan_backward(buf, probe, keep),
        scan_forward(buf, probe, keep),
    )
}

/// Horizontal position of `pos` measured in characters from the line start.
///
/// Used for the status bar and for vertical-movement column memory.
pub fn column_of(buf: &Buffer, pos: usize) -> usize {
    let line = buf.line_of_byte(pos);
    let start = buf.line_start(line);
    let mut n = 0usize;
    let mut p = start;
    let mut chunk = [0u8; 8192];
    while p < pos {
        let want = (pos - p).min(chunk.len());
        let got = buf.read_prefix(p, &mut chunk[..want]);
        if got == 0 {
            break;
        }
        for &b in &chunk[..got] {
            // Count only lead bytes: continuation bytes are not characters.
            if (b & 0xC0) != 0x80 {
                n += 1;
            }
        }
        p += got;
    }
    n
}

/// Offset of the character at `column` characters into `line`, clamped to the
/// line's content end.
pub fn offset_at_column(buf: &Buffer, line: usize, column: usize) -> usize {
    let (start, end) = buf.line_span(line);
    let mut n = 0usize;
    let mut p = start;
    let mut chunk = [0u8; 8192];
    while p < end {
        let want = (end - p).min(chunk.len());
        let got = buf.read_prefix(p, &mut chunk[..want]);
        if got == 0 {
            break;
        }
        for (i, &b) in chunk[..got].iter().enumerate() {
            if (b & 0xC0) != 0x80 {
                if n == column {
                    return p + i;
                }
                n += 1;
            }
        }
        p += got;
    }
    end
}
