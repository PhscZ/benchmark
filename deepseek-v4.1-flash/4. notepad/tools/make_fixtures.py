#!/usr/bin/env python3
"""Deterministic fixture generator for the notepad editor benchmark.

Usage:
    python tools/make_fixtures.py

Writes seven fixture files into ./fixtures/ and prints, for every fixture,
the exact byte size, the line count, the SHA-256 and (where applicable) the
token occurrence counts.

The generator is fully seeded, so every run produces byte-identical output
(re-runnable / idempotent).  All files use LF endings unless stated otherwise.
"""

from __future__ import annotations

import hashlib
import os
import random
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURES = os.path.join(ROOT, "fixtures")

SEED = 20250921

TARGET_100MIB = 104_857_600  # 100 MiB, exact
MILLION_LINES = 1_000_000
NEEDLE_EVERY = 100_000
LONG_LINE_BYTES = 200_000
LONG_LINE_COUNT = 50
SEARCHABLE_TOKEN_COUNT = 5_000
UNICODE_TOKEN_COUNT = 100

SEARCHABLE_TOKEN = "SEARCHABLE_TOKEN"
UNICODE_TOKEN = "Ünïcödé_Töken"
NEEDLE_MARKER = "NEEDLE_MARKER"

# --------------------------------------------------------------------------
# word material
# --------------------------------------------------------------------------

WORDS = (
    "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima "
    "mike november oscar papa quebec romeo sierra tango uniform victor whiskey "
    "xray yankee zulu editor buffer cursor selection scroll viewport gutter "
    "token render highlight syntax indent window keyboard search replace "
    "document column offset unicode encoding newline buffer caret mark"
).split()

PROSE_WORDS = (
    "the quick brown fox jumps over a lazy dog while the editor keeps redrawing "
    "text buffer lines columns rows selection cursor viewport scrollbar gutter "
    "syntax highlight token keyword string comment literal operator identifier "
    "indent whitespace newline carriage encoding unicode grapheme cluster "
    "document window frame render pass paint callback event keyboard mouse "
    "search replace find match needle offset byte character glyph shaping "
    "cache invalidation layout wrap word soft hard scroll horizontal vertical "
    "profiler latency throughput memory allocation arena borrow lifetime trait "
    "generic monomorphise iterator adapter closure future poll wake runtime "
    "network socket packet header payload checksum sequence acknowledgment "
    "protocol handshake negotiation certificate authority signature digest"
).split()

EXOTIC_LINES = (
    "Пример текста на русском языке для проверки кодировки.",
    "Ελληνικό κείμενο δοκιμής για έλεγχο κωδικοποίησης.",
    "这是一段中文测试文本，用于检验多字节字符的字节偏移。",
    "日本語のテスト文です。マルチバイト文字を確認します。",
    "한국어 테스트 문장입니다. 다중 바이트 문자를 확인합니다.",
    "Emoji status: 😀 🚀 🌟 🎉 done.",
    "Accented Latin: café naïve résumé Größe Ångström.",
)


# --------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------


class ByteWriter:
    """Buffered binary writer that tracks exact byte and line counts."""

    def __init__(self, path: str) -> None:
        self.path = path
        self._f = open(path, "wb")
        self._buf = bytearray()
        self.total_bytes = 0
        self.line_count = 0

    def line(self, text: str) -> int:
        raw = text.encode("utf-8") + b"\n"
        self._buf += raw
        self.total_bytes += len(raw)
        self.line_count += 1
        if len(self._buf) >= 1 << 20:
            self._f.write(self._buf)
            self._buf.clear()
        return len(raw)

    def close(self) -> None:
        if self._buf:
            self._f.write(self._buf)
            self._buf.clear()
        self._f.close()


def short_ascii_line(rng: random.Random) -> str:
    """A 20-60 character pure-ASCII line."""
    target = rng.randint(22, 60)
    s = rng.choice(WORDS)
    while True:
        cand = s + " " + rng.choice(WORDS)
        if len(cand) > target or len(cand) > 60:
            break
        s = cand
    if len(s) < 20:
        s = s + " " + "abcdefghijklmnopqrstuvwxyz"[: 20 - len(s) - 1]
    return s


def prose_line(rng: random.Random) -> str:
    """A prose-like pure-ASCII line."""
    n = rng.randint(6, 18)
    words = [rng.choice(PROSE_WORDS) for _ in range(n)]
    words[0] = words[0].capitalize()
    return " ".join(words) + "."


def exotic_line(rng: random.Random, index: int) -> str:
    """A line containing multi-byte UTF-8; categories cycle deterministically."""
    core = EXOTIC_LINES[index % len(EXOTIC_LINES)]
    lead = " ".join(rng.choice(PROSE_WORDS) for _ in range(rng.randint(1, 4)))
    tail = " ".join(rng.choice(PROSE_WORDS) for _ in range(rng.randint(1, 4)))
    return f"{lead.capitalize()} {core} {tail}."


def searchable_token_line(rng: random.Random) -> str:
    lead = " ".join(rng.choice(PROSE_WORDS) for _ in range(rng.randint(0, 8)))
    trail = " ".join(rng.choice(PROSE_WORDS) for _ in range(rng.randint(0, 8)))
    return f"{lead} {SEARCHABLE_TOKEN} {trail}".strip()


def unicode_token_line(rng: random.Random) -> str:
    lead = " ".join(rng.choice(PROSE_WORDS) for _ in range(rng.randint(0, 6)))
    core = EXOTIC_LINES[rng.randrange(len(EXOTIC_LINES))]
    return f"{lead} {UNICODE_TOKEN} {core}".strip()


_LONG_PHRASE = (
    "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor "
)


def long_line(index: int) -> str:
    """A single ASCII line of exactly LONG_LINE_BYTES + index bytes."""
    size = LONG_LINE_BYTES + index
    reps, rem = divmod(size, len(_LONG_PHRASE))
    return _LONG_PHRASE * reps + _LONG_PHRASE[:rem]


def sha256(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def scan_lines(path: str, tokens: tuple[str, ...]):
    """Re-read `path` line by line (binary) and collect verification facts.

    Returns a dict with byte size, line count, per-token occurrence and
    line counts, and min/max line byte length.
    """
    token_bytes = [t.encode("utf-8") for t in tokens]
    occ = [0] * len(tokens)
    on_lines = [0] * len(tokens)
    size = 0
    lines = 0
    min_len = None
    max_len = 0
    long_lines = 0
    with open(path, "rb") as f:
        while True:
            raw = f.readline()
            if not raw:
                break
            size += len(raw)
            lines += 1
            body = raw[:-1] if raw.endswith(b"\n") else raw
            n = len(body)
            if min_len is None or n < min_len:
                min_len = n
            if n > max_len:
                max_len = n
            if n >= LONG_LINE_BYTES:
                long_lines += 1
            for i, tb in enumerate(token_bytes):
                c = body.count(tb)
                if c:
                    occ[i] += c
                    on_lines[i] += 1
    return {
        "bytes": size,
        "lines": lines,
        "occ": occ,
        "on_lines": on_lines,
        "min_len": min_len,
        "max_len": max_len,
        "long_lines": long_lines,
    }


def report(path: str, tokens: tuple[str, ...] = ()) -> dict:
    stats = scan_lines(path, tokens)
    print(f"== {os.path.relpath(path, ROOT).replace(os.sep, '/')}")
    print(f"   bytes      : {stats['bytes']}")
    print(f"   lines      : {stats['lines']}")
    if stats["min_len"] is not None:
        print(f"   line bytes : min {stats['min_len']} max {stats['max_len']}")
    for i, tok in enumerate(tokens):
        print(
            f"   {tok!r}: {stats['occ'][i]} occurrence(s) on "
            f"{stats['on_lines'][i]} line(s)"
        )
    print(f"   sha256     : {sha256(path)}")
    return stats


# --------------------------------------------------------------------------
# fixtures
# --------------------------------------------------------------------------


def build_million_lines(path: str) -> None:
    rng = random.Random(SEED + 1)
    w = ByteWriter(path)
    markers = 0
    for i in range(1, MILLION_LINES + 1):
        if i % NEEDLE_EVERY == 0:
            s = f"{NEEDLE_MARKER} row {i:07d}"
            markers += 1
        else:
            s = short_ascii_line(rng)
        assert 20 <= len(s) <= 60, (i, len(s), s)
        assert s.isascii(), (i, s)
        w.line(s)
    w.close()
    assert markers == MILLION_LINES // NEEDLE_EVERY, markers
    assert w.line_count >= MILLION_LINES, w.line_count


def build_large_100mb(path: str) -> None:
    rng = random.Random(SEED + 2)
    w = ByteWriter(path)

    token_at = [(TARGET_100MIB * k) // SEARCHABLE_TOKEN_COUNT
                for k in range(1, SEARCHABLE_TOKEN_COUNT + 1)]
    unicode_at = [(TARGET_100MIB * k) // UNICODE_TOKEN_COUNT
                  for k in range(1, UNICODE_TOKEN_COUNT + 1)]
    long_at = [(TARGET_100MIB * k) // LONG_LINE_COUNT
               for k in range(1, LONG_LINE_COUNT + 1)]

    ti = ui = li = 0
    exotic_index = 0
    exotic_lines = 0
    emitted_searchable = 0
    emitted_unicode = 0
    emitted_long = 0

    def emit_pending() -> None:
        nonlocal ti, ui, li, exotic_index, exotic_lines
        nonlocal emitted_searchable, emitted_unicode, emitted_long
        while ti < len(token_at) and token_at[ti] <= w.total_bytes:
            w.line(searchable_token_line(rng))
            emitted_searchable += 1
            ti += 1
        while ui < len(unicode_at) and unicode_at[ui] <= w.total_bytes:
            w.line(unicode_token_line(rng))
            emitted_unicode += 1
            ui += 1
        while li < len(long_at) and long_at[li] <= w.total_bytes:
            w.line(long_line(li))
            emitted_long += 1
            li += 1

    while w.total_bytes < TARGET_100MIB:
        if rng.random() < 0.10:
            w.line(exotic_line(rng, exotic_index))
            exotic_index += 1
            exotic_lines += 1
        else:
            w.line(prose_line(rng))
        emit_pending()
    emit_pending()
    w.close()

    assert w.total_bytes >= TARGET_100MIB, w.total_bytes
    assert emitted_searchable == SEARCHABLE_TOKEN_COUNT, emitted_searchable
    assert emitted_unicode == UNICODE_TOKEN_COUNT, emitted_unicode
    assert emitted_long == LONG_LINE_COUNT, emitted_long
    assert exotic_lines >= 10_000, exotic_lines


def build_small_utf8(path: str) -> None:
    body = (
        "Small UTF-8 fixture for the notepad benchmark\n"
        "TODO: accents -> café, naïve, résumé, Ångström, Größe\n"
        "TODO: CJK -> 你好世界 日本語 한국어 中文测试\n"
        "TODO: emoji -> 😀 🚀 🌟 🎉 (4-byte UTF-8)\n"
        "tab\tseparated\tcolumns\there\n"
        "trailing spaces follow:   \n"
        "Mixed: ASCII + Ελληνικά + Русский + 中文 + émoji 🎉\twith a tab\n"
        "combining: e\u0301 vs é  |  zwj family: 👨‍👩‍👧‍👦\n"
    )
    data = body.encode("utf-8")
    with open(path, "wb") as f:
        f.write(data)
    assert len(data) < 4096, len(data)


def build_crlf(path: str) -> None:
    lines = [
        "First line is plain ASCII.",
        "TODO: verify CRLF handling in the editor.",
        "Another ASCII line with some content here.",
        "Non-ASCII line: café — naïve — 日本語 — Привет",
        "TODO: second marker for the CRLF fixture.",
        "Last line, still CRLF terminated.",
    ]
    data = b"".join(line.encode("utf-8") + b"\r\n" for line in lines)
    with open(path, "wb") as f:
        f.write(data)
    assert len(data) < 2048, len(data)
    assert data.count(b"\n") == len(lines), data.count(b"\n")
    assert data.count(b"\r\n") == len(lines)
    assert b"\n" not in data.replace(b"\r\n", b""), "bare LF found"


def build_invalid_utf8(path: str) -> None:
    data = (
        b"ASCII header line\n"
        b"lone continuation bytes: \x80\x81\x82\n"
        b"truncated 2-byte sequence: \xc3\n"
        b"truncated 3-byte sequence: \xe2\x82\n"
        b"truncated 4-byte sequence: \xf0\x9f\x98\n"
        b"overlong 2-byte encoding: \xc0\xaf\n"
        b"overlong 3-byte encoding: \xe0\x80\xaf\n"
        b"overlong 4-byte encoding: \xf0\x80\x80\xaf\n"
        b"stray byte: \xff\n"
        b"stray bytes: \xfe\xff\n"
        b"tail line\n"
    )
    with open(path, "wb") as f:
        f.write(data)
    assert len(data) < 1024, len(data)
    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        pass
    else:  # pragma: no cover
        raise AssertionError("invalid_utf8.bin decoded as valid UTF-8")


def build_empty(path: str) -> None:
    with open(path, "wb") as f:
        f.write(b"")


def build_bom(path: str) -> None:
    with open(path, "wb") as f:
        f.write(b"\xef\xbb\xbfHello BOM\n")


# --------------------------------------------------------------------------


def main() -> int:
    os.makedirs(FIXTURES, exist_ok=True)

    p1 = os.path.join(FIXTURES, "million_lines.txt")
    p2 = os.path.join(FIXTURES, "large_100mb.txt")
    p3 = os.path.join(FIXTURES, "small_utf8.txt")
    p4 = os.path.join(FIXTURES, "crlf.txt")
    p5 = os.path.join(FIXTURES, "invalid_utf8.bin")
    p6 = os.path.join(FIXTURES, "empty.txt")
    p7 = os.path.join(FIXTURES, "bom_utf8.txt")

    build_million_lines(p1)
    build_large_100mb(p2)
    build_small_utf8(p3)
    build_crlf(p4)
    build_invalid_utf8(p5)
    build_empty(p6)
    build_bom(p7)

    s1 = report(p1, (NEEDLE_MARKER,))
    s2 = report(p2, (SEARCHABLE_TOKEN, UNICODE_TOKEN))
    s3 = report(p3, ("TODO",))
    s4 = report(p4, ("TODO",))
    report(p5)
    report(p6)
    report(p7)

    # hard acceptance checks
    assert s1["lines"] >= MILLION_LINES, s1["lines"]
    assert 20 <= s1["min_len"] and s1["max_len"] <= 60, (s1["min_len"], s1["max_len"])
    assert s1["occ"][0] == MILLION_LINES // NEEDLE_EVERY, s1["occ"]
    assert s1["bytes"] >= 30 * 1024 * 1024, s1["bytes"]
    assert s2["bytes"] >= TARGET_100MIB, s2["bytes"]
    assert s2["occ"][0] == SEARCHABLE_TOKEN_COUNT, s2["occ"]
    assert s2["on_lines"][0] == SEARCHABLE_TOKEN_COUNT, s2["on_lines"]
    assert s2["occ"][1] == UNICODE_TOKEN_COUNT, s2["occ"]
    assert s2["on_lines"][1] == UNICODE_TOKEN_COUNT, s2["on_lines"]
    assert s2["long_lines"] >= LONG_LINE_COUNT, s2["long_lines"]
    assert s3["occ"][0] == 3, s3["occ"]
    assert s3["bytes"] < 4096, s3["bytes"]
    assert s4["occ"][0] == 2, s4["occ"]
    assert s4["bytes"] < 2048, s4["bytes"]

    print()
    print("ALL FIXTURES OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
