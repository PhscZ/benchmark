#!/usr/bin/env python3
"""dupefinder - find exact and near-duplicate images in a folder.

Read-only: this tool never deletes, renames, moves or modifies any file.

Method
------
1.  Every file is hashed with SHA-256 (streamed, 1 MiB chunks) so that
    byte-identical files are found with certainty.
2.  Every decodable image is squashed to a 32x32 RGB thumbnail and reduced to a
    compact signature:
      * a 63-bit DCT perceptual hash (pHash) of the *brightness/contrast
        normalised* grayscale thumbnail -> structure, resolution-independent,
        survives re-encoding, metadata stripping, format and size changes;
      * a 4x4 grid of block chromaticities (r, g, b normalised by their sum)
        -> brightness invariant, but distinguishes structurally identical
        images whose colours actually differ;
      * the mean luminance -> a very loose gate that rejects e.g. a solid black
        image matching a solid white one.
3.  Candidates are retrieved with a Burkhard-Keller tree over the pHash using
    the Hamming metric, which finds *all* pairs within the radius implied by the
    threshold (no false negatives from bucketing), so no full pairwise
    comparison is needed.
4.  Groups are grown greedily in "best original first" order: the highest
    ranked unassigned image becomes the retained original and every remaining
    image that matches *that original directly* joins its group.  Matches are
    therefore never chained through an intermediate image.

See README.md for the full rationale, defaults and limitations.
"""

from __future__ import annotations

import argparse
import hashlib
import math
import os
import stat as stat_module
import sys
import time
from dataclasses import dataclass
from typing import Iterator, Sequence

import numpy as np
from PIL import Image, ImageOps

# --------------------------------------------------------------------------
# Tunables (documented in README.md)
# --------------------------------------------------------------------------

IMAGE_EXTENSIONS = frozenset({".jpg", ".jpeg", ".png", ".webp"})

HASH_SIZE = 32                  # thumbnail edge used for the perceptual hash
DCT_SIZE = 8                    # low frequency block kept from the 2-D DCT
HASH_BITS = DCT_SIZE * DCT_SIZE - 1     # 63 bits, the DC coefficient is dropped
PHASH_DEADZONE = 0.02           # fraction of the strongest coefficient, see _phash

CHROMA_GRID = 4                 # the thumbnail is split into 4x4 colour blocks

DEFAULT_THRESHOLD = 0.90        # pHash similarity; higher == stricter
DEFAULT_COLOR_GATE = 0.20       # max RMS distance between block chromaticities
LUMINANCE_GATE = 4.0            # max ratio between mean luminances (both +FLOOR)
LUMINANCE_FLOOR = 8.0

EXIF_ORIENTATION = 274
SWAPPING_ORIENTATIONS = frozenset({5, 6, 7, 8})

# Transparency is resolved by compositing the image onto an opaque white
# background before any pixel is measured.  See README.md ("Transparency").
ALPHA_BACKGROUND = (255, 255, 255)

READ_CHUNK = 1 << 20
MAX_WARNINGS = 50


def _dct_matrix(n: int) -> np.ndarray:
    """Orthonormal DCT-II matrix of size n (rows = frequency, cols = sample)."""
    k = np.arange(n).reshape(-1, 1)
    x = np.arange(n).reshape(1, -1)
    matrix = np.cos(math.pi * (2 * x + 1) * k / (2 * n)) * math.sqrt(2.0 / n)
    matrix[0] *= math.sqrt(0.5)
    return matrix


_DCT = _dct_matrix(HASH_SIZE)


# --------------------------------------------------------------------------
# Signatures
# --------------------------------------------------------------------------


@dataclass(frozen=True, slots=True, eq=False)


class Signature:
    """Compact per-image comparison data.  No decoded pixels are retained."""

    path: str                   # absolute path
    size: int                   # file size in bytes
    width: int                  # width after EXIF orientation normalisation
    height: int                 # height after EXIF orientation normalisation
    digest: bytes               # SHA-256 of the raw file bytes
    phash: int                  # 63-bit perceptual hash
    chroma: np.ndarray          # (CHROMA_GRID**2, 3) block chromaticities, float32
    luminance: float

    @property
    def area(self) -> int:
        return self.width * self.height

    @property
    def dimensions(self) -> str:
        return f"{self.width}x{self.height}"


def rank_key(sig: Signature) -> tuple[int, int, str]:
    """Sort key selecting the image to keep: largest area, then largest file,
    then lexicographically smallest full path."""
    return (-sig.area, -sig.size, sig.path)


def sha256_file(path: str) -> bytes:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while True:
            chunk = handle.read(READ_CHUNK)
            if not chunk:
                break
            digest.update(chunk)
    return digest.digest()


def _has_alpha(img: Image.Image) -> bool:
    if img.mode in ("RGBA", "LA", "PA"):
        return True
    if img.mode == "P" and "transparency" in img.info:
        return True
    return img.mode == "RGB" and "transparency" in img.info


def load_thumbnail(path: str) -> tuple[np.ndarray, int, int]:
    """Decode `path` and return (32x32x3 uint8 RGB array, width, height).

    * The reported width/height are the true pixel dimensions from the file
      header, swapped when the EXIF orientation says the image is rotated by a
      quarter turn.
    * JPEGs are decoded with Pillow's `draft()` at the smallest scale that is
      still >= 32x32, which keeps peak memory bounded for huge images.
    * EXIF orientation is applied before anything is measured.
    * Transparency is composited onto `ALPHA_BACKGROUND`.
    """
    with Image.open(path) as img:
        width, height = img.size
        orientation = img.getexif().get(EXIF_ORIENTATION, 1)
        if orientation in SWAPPING_ORIENTATIONS:
            width, height = height, width

        img.draft("RGB", (HASH_SIZE, HASH_SIZE))
        img = ImageOps.exif_transpose(img)
        if _has_alpha(img):
            rgba = img.convert("RGBA")
            background = Image.new("RGBA", rgba.size, ALPHA_BACKGROUND + (255,))
            img = Image.alpha_composite(background, rgba)
        img = img.convert("RGB")

        # Integer box-reduction first: cheap and it keeps the final LANCZOS
        # resize on a small image (deterministic for a given input).
        factor = max(1, min(img.size) // (HASH_SIZE * 2))
        if factor > 1:
            img = img.reduce(factor)
        thumb = img.resize((HASH_SIZE, HASH_SIZE), Image.Resampling.LANCZOS)
        pixels = np.asarray(thumb, dtype=np.uint8)

    return pixels, width, height


def _chroma_grid(pixels: np.ndarray) -> np.ndarray:
    """Mean chromaticity of each CHROMA_GRID x CHROMA_GRID block of the thumbnail.

    Chromaticity is the colour triple divided by its own sum, so it is
    invariant to brightness and contrast scaling.  Keeping it per block (rather
    than as a single global average) is what separates images whose colours
    differ only in part of the frame - e.g. a red and a blue version of the
    same logo - from images that merely received a global tint or exposure
    change.  Blocks that are pure black are achromatic by definition.
    """
    rgb = pixels.reshape(HASH_SIZE, HASH_SIZE, 3).astype(np.float64)
    step = HASH_SIZE // CHROMA_GRID
    blocks = rgb.reshape(CHROMA_GRID, step, CHROMA_GRID, step, 3).mean(axis=(1, 3))
    blocks = blocks.reshape(-1, 3)
    sums = blocks.sum(axis=1, keepdims=True)
    safe = np.where(sums > 0.0, sums, 1.0)
    chroma = np.where(sums > 0.0, blocks / safe, 1.0 / 3.0)
    return chroma.astype(np.float32)


def signature_for(path: str, size: int) -> Signature:
    """Build the full signature of one file.  Raises on unreadable/corrupt input."""
    digest = sha256_file(path)
    pixels, width, height = load_thumbnail(path)
    if width <= 0 or height <= 0:
        raise ValueError("image has zero width or height")

    rgb = pixels.reshape(-1, 3).astype(np.float64)
    mean_r, mean_g, mean_b = rgb.mean(axis=0)
    luminance = 0.299 * mean_r + 0.587 * mean_g + 0.114 * mean_b

    return Signature(
        path=path,
        size=size,
        width=width,
        height=height,
        digest=digest,
        phash=_phash(pixels),
        chroma=_chroma_grid(pixels),
        luminance=luminance,
    )


def _phash(pixels: np.ndarray) -> int:
    """63-bit DCT perceptual hash of a 32x32 RGB thumbnail.

    The grayscale thumbnail is mean/std normalised first, which makes the hash
    invariant to affine brightness and contrast changes (the DC coefficient is
    dropped as well).

    Coefficients are compared against the median plus a small deadzone
    (PHASH_DEADZONE of the strongest coefficient).  Without it the comparison
    is unstable on images with large flat areas: dozens of coefficients sit
    exactly on the median, so a re-encode flips them all at once (measured:
    34 of 63 bits between a transparent PNG and the same shape flattened onto
    white).  With the deadzone those near-ties are all 0 in both images.

    Flat images have no structure at all; they hash to 0 and are separated by
    the colour and luminance gates instead.
    """
    gray = pixels.astype(np.float64) @ np.array([0.299, 0.587, 0.114])
    std = gray.std()
    if std > 1e-6:
        gray = (gray - gray.mean()) / std
    else:
        gray = np.zeros_like(gray)

    coeffs = _DCT @ gray @ _DCT.T
    block = coeffs[:DCT_SIZE, :DCT_SIZE].ravel()[1:]        # drop DC
    threshold = np.median(block) + PHASH_DEADZONE * np.abs(block).max()
    bits = block > threshold
    packed = np.packbits(bits).tobytes()                    # 63 bits, zero padded
    return int.from_bytes(packed, "big") >> (len(packed) * 8 - HASH_BITS)


# --------------------------------------------------------------------------
# Similarity
# --------------------------------------------------------------------------


def hamming_distance(a: Signature, b: Signature) -> int:
    return (a.phash ^ b.phash).bit_count()


def similarity(a: Signature, b: Signature) -> float:
    return 1.0 - hamming_distance(a, b) / HASH_BITS


def chroma_distance(a: Signature, b: Signature) -> float:
    """Root-mean-square distance between the per-block chromaticities."""
    delta = a.chroma - b.chroma
    return float(np.sqrt(np.mean(np.sum(delta * delta, axis=1))))


def luminance_ratio(a: Signature, b: Signature) -> float:
    low = min(a.luminance, b.luminance) + LUMINANCE_FLOOR
    high = max(a.luminance, b.luminance) + LUMINANCE_FLOOR
    return high / low


def perceptual_match(
    a: Signature,
    b: Signature,
    threshold: float,
    color_gate: float,
) -> bool:
    """True when `b` may be reported as a duplicate of the retained original `a`.

    A `color_gate` of 0 disables the colour test; the luminance gate is fixed.
    """
    if similarity(a, b) < threshold:
        return False
    if color_gate > 0.0 and chroma_distance(a, b) > color_gate:
        return False
    if luminance_ratio(a, b) > LUMINANCE_GATE:
        return False
    return True


# --------------------------------------------------------------------------
# Candidate index: Burkhard-Keller tree over the pHash, Hamming metric
# --------------------------------------------------------------------------


class _BKNode:
    __slots__ = ("key", "items", "children")

    def __init__(self, key: int, item: int) -> None:
        self.key = key
        self.items: list[int] = [item]
        self.children: dict[int, _BKNode] = {}


class BKTree:
    """Metric tree.  `search` returns every stored item within `radius`, so the
    candidate filter cannot hide a true match (unlike hash bucketing)."""

    __slots__ = ("_root", "size")

    def __init__(self) -> None:
        self._root: _BKNode | None = None
        self.size = 0

    def add(self, key: int, item: int) -> None:
        self.size += 1
        if self._root is None:
            self._root = _BKNode(key, item)
            return
        node = self._root
        while True:
            distance = (key ^ node.key).bit_count()
            if distance == 0:
                node.items.append(item)
                return
            child = node.children.get(distance)
            if child is None:
                node.children[distance] = _BKNode(key, item)
                return
            node = child

    def search(self, key: int, radius: int) -> list[int]:
        root = self._root
        if root is None:
            return []
        found: list[int] = []
        stack = [root]
        while stack:
            node = stack.pop()
            distance = (key ^ node.key).bit_count()
            if distance <= radius:
                found.extend(node.items)
            low = distance - radius
            if low < 0:
                low = 0
            high = distance + radius
            for child_distance, child in node.children.items():
                if low <= child_distance <= high:
                    stack.append(child)
        return found


def search_radius(threshold: float) -> int:
    """Largest Hamming distance whose similarity can still reach `threshold`."""
    return math.ceil(HASH_BITS * (1.0 - threshold))


# --------------------------------------------------------------------------
# Grouping
# --------------------------------------------------------------------------


@dataclass(slots=True)


class Group:
    original: int
    members: list[int]


def build_groups(
    signatures: Sequence[Signature],
    threshold: float,
    color_gate: float,
) -> list[Group]:
    """Star-shaped groups: every member matches the group's original directly."""
    count = len(signatures)
    radius = search_radius(threshold)
    tree = BKTree()
    for index, sig in enumerate(signatures):
        tree.add(sig.phash, index)

    order = sorted(range(count), key=lambda i: rank_key(signatures[i]))
    assigned = bytearray(count)
    groups: list[Group] = []

    for index in order:
        if assigned[index]:
            continue
        assigned[index] = 1
        original = signatures[index]
        members: list[int] = []
        for candidate in tree.search(original.phash, radius):
            if assigned[candidate]:
                continue
            if perceptual_match(original, signatures[candidate], threshold, color_gate):
                assigned[candidate] = 1
                members.append(candidate)
        if members:
            members.sort(key=lambda i: rank_key(signatures[i]))
            groups.append(Group(original=index, members=members))

    return groups


# --------------------------------------------------------------------------
# Filesystem walk
# --------------------------------------------------------------------------


@dataclass


class Stats:
    files_seen: int = 0
    processed: int = 0
    unsupported: int = 0
    hardlink_alias: int = 0
    unreadable: int = 0
    dir_symlinks: int = 0
    unreadable_dirs: int = 0

    @property
    def skipped(self) -> int:
        return self.unsupported + self.hardlink_alias + self.unreadable


class Console:
    """Progress + warnings on stderr, capped so a broken tree cannot flood."""

    def __init__(self, progress: bool) -> None:
        self.stream = sys.stderr
        self.tty = bool(getattr(self.stream, "isatty", lambda: False)())
        self.progress_enabled = progress
        self.warning_count = 0
        self.suppressed = 0
        self._last_emit = 0.0
        self._dirty = False

    def warn(self, message: str) -> None:
        if self.warning_count >= MAX_WARNINGS:
            self.suppressed += 1
            return
        self.warning_count += 1
        self._clear()
        print(f"warning: {message}", file=self.stream, flush=True)

    def status(self, message: str) -> None:
        """One-off progress note (no counters yet, e.g. while listing files)."""
        if not self.progress_enabled:
            return
        self._clear()
        self.stream.write(f"  {message}\n")
        self.stream.flush()

    def update(self, done: int, total: int, label: str) -> None:
        if not self.progress_enabled or total <= 0:
            return
        now = time.monotonic()
        interval = 0.1 if self.tty else 2.0
        if done < total and now - self._last_emit < interval:
            return
        self._last_emit = now
        percent = 100.0 * done / total
        text = f"  {done}/{total} ({percent:5.1f}%)  {_shorten(label, 60)}"
        if self.tty:
            self.stream.write("\r" + text.ljust(90)[:110])
            self._dirty = True
        else:
            self.stream.write(text + "\n")
        self.stream.flush()

    def finish(self) -> None:
        if self._dirty:
            self._clear()

    def _clear(self) -> None:
        if self._dirty:
            self.stream.write("\r" + " " * 110 + "\r")
            self.stream.flush()
            self._dirty = False


def _shorten(text: str, limit: int) -> str:
    return text if len(text) <= limit else "..." + text[-(limit - 3):]


def _is_directory_link(entry: os.DirEntry) -> bool:
    """True for directory symlinks and (on Windows) directory junctions.

    `is_symlink` misses NTFS junctions, which are also reparse points and can
    point at an ancestor folder, so they are detected separately.
    """
    if entry.is_symlink():
        return entry.is_dir(follow_symlinks=True)
    is_junction = getattr(os.path, "isjunction", None)
    return bool(is_junction and is_junction(entry.path))


def iter_files(
    root: str,
    recursive: bool,
    console: Console,
    stats: Stats,
) -> Iterator[tuple[str, os.stat_result]]:
    """Yield (path, stat) for regular files.  Directory symlinks and junctions
    are never followed; entries are sorted so traversal order is deterministic."""
    stack = [root]
    visited: set[str] = set()
    while stack:
        current = stack.pop()
        real = os.path.realpath(current)
        if real in visited:            # reached twice (junction/loop): scan once
            continue
        visited.add(real)
        try:
            with os.scandir(current) as entries:
                items = sorted(entries, key=lambda entry: entry.name)
        except OSError as exc:
            stats.unreadable_dirs += 1
            console.warn(f"cannot list folder {current}: {exc}")
            continue

        for entry in items:
            path = entry.path
            try:
                if _is_directory_link(entry):
                    stats.dir_symlinks += 1
                    continue
                if entry.is_dir(follow_symlinks=False):
                    if recursive:
                        stack.append(path)
                    continue
                info = os.stat(path)
            except OSError as exc:
                stats.files_seen += 1
                stats.unreadable += 1
                console.warn(f"cannot read {path}: {exc}")
                continue
            if not stat_module.S_ISREG(info.st_mode):
                continue
            stats.files_seen += 1
            yield path, info


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def human_bytes(count: int) -> str:
    value = float(count)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024.0 or unit == "TiB":
            return f"{int(value)} B" if unit == "B" else f"{value:.2f} {unit}"
        value /= 1024.0
    return f"{value:.2f} TiB"


def format_report(
    root: str,
    recursive: bool,
    threshold: float,
    color_gate: float,
    stats: Stats,
    signatures: Sequence[Signature],
    groups: Sequence[Group],
    console: Console,
) -> str:
    lines: list[str] = []
    add = lines.append

    add("Image duplicate scan")
    add("====================")
    add(f"Folder           : {root}")
    add(f"Recursive        : {'yes' if recursive else 'no'}")
    add("Formats          : JPEG, PNG, WebP")
    add(f"pHash threshold  : {threshold:.4g} "
        f"(higher = stricter; matches when similarity >= threshold, "
        f"i.e. Hamming distance <= {search_radius(threshold)} of {HASH_BITS} bits)")
    add(f"Colour gate      : {color_gate:.4g} "
        f"(max RMS block-chromaticity distance; rejects same-shape, other-colour images)")
    add("")

    add("Scan")
    add("----")
    add(f"Files scanned        : {stats.files_seen:,}")
    add(f"Images processed     : {stats.processed:,}")
    add(f"Files skipped        : {stats.skipped:,}")
    add(f"  unsupported format : {stats.unsupported:,}")
    add(f"  hard-link alias    : {stats.hardlink_alias:,}")
    add(f"  unreadable/corrupt : {stats.unreadable:,}")
    if stats.dir_symlinks:
        add(f"Directory symlinks/junctions not followed : {stats.dir_symlinks:,}")
    if stats.unreadable_dirs:
        add(f"Folders that could not be listed: {stats.unreadable_dirs:,}")
    if console.suppressed:
        add(f"Warnings suppressed after {MAX_WARNINGS}: {console.suppressed:,}")
    add("")

    duplicate_files = sum(len(group.members) for group in groups)
    group_twins = [_digest_twins(signatures, group) for group in groups]
    exact_files = sum(
        1
        for group, twins in zip(groups, group_twins)
        for member in group.members
        if len(twins[signatures[member].digest]) > 1
    )
    perceptual_files = duplicate_files - exact_files
    duplicate_bytes = sum(
        signatures[member].size for group in groups for member in group.members
    )

    add("Results")
    add("-------")
    if stats.processed == 0:
        add("No images to compare." if stats.files_seen else "Folder contains no files.")
        return "\n".join(lines)

    add(f"Duplicate groups : {len(groups):,}")
    add(f"Duplicate files  : {duplicate_files:,} "
        f"({exact_files:,} byte-identical, {perceptual_files:,} perceptual)")
    add("")
    add(f"Total duplicate size: {duplicate_bytes:,} bytes ({human_bytes(duplicate_bytes)})")
    add("  Potential savings based on logical file sizes (the sum of the duplicate")
    add("  copies, excluding the retained original of each group).  This is not")
    add("  guaranteed disk space recovered: hard links, filesystem compression or")
    add("  existing block-level deduplication can make the real figure smaller.")

    if not groups:
        add("")
        add("No duplicates found.")
        return "\n".join(lines)

    for number, (group, twins) in enumerate(zip(groups, group_twins), start=1):
        original = signatures[group.original]
        group_bytes = sum(signatures[m].size for m in group.members)
        add("")
        add(f"Group {number} of {len(groups)} "
            f"({len(group.members)} duplicate{'s' if len(group.members) != 1 else ''}, "
            f"{group_bytes:,} bytes, {human_bytes(group_bytes)})")
        add(f"  [KEEP] {original.dimensions:>11}  {original.size:>12,} B  {original.path}")
        for member in group.members:
            sig = signatures[member]
            same_bytes = [i for i in twins.get(sig.digest, ()) if i != member]
            add(f"  [DUP ] {sig.dimensions:>11}  {sig.size:>12,} B  {sig.path}")
            if same_bytes:
                twin = min(same_bytes, key=lambda i: rank_key(signatures[i]))
                add(f"         byte-identical to {signatures[twin].path} (same SHA-256)")
            else:
                distance = hamming_distance(original, sig)
                add(f"         perceptual match: distance {distance}/{HASH_BITS}, "
                    f"similarity {similarity(original, sig):.4f}")

    return "\n".join(lines)


def _digest_twins(
    signatures: Sequence[Signature],
    group: Group,
) -> dict[bytes, list[int]]:
    """SHA-256 -> group members (retained original included) sharing that digest."""
    twins: dict[bytes, list[int]] = {}
    for index in (group.original, *group.members):
        twins.setdefault(signatures[index].digest, []).append(index)
    return twins


# --------------------------------------------------------------------------
# Entry point
# --------------------------------------------------------------------------


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="dupefinder",
        description="Find exact and near-duplicate JPEG/PNG/WebP images in a folder. "
                    "Read-only: nothing is ever deleted, renamed or modified.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Threshold semantics\n"
            "  The perceptual hash is 63 bits.  A pair matches when at least\n"
            "  THRESHOLD of those bits agree, i.e. when the Hamming distance is at\n"
            "  most ceil(63 * (1 - THRESHOLD)).\n"
            "  HIGHER VALUES ARE STRICTER (fewer, more certain duplicates):\n"
            "    1.00  only visually identical images (0 differing bits)\n"
            "    0.90  default, at most 7 differing bits\n"
            "    0.80  loose, at most 13 differing bits - expect false positives\n"
            "  Byte-identical files are always reported regardless of the threshold.\n"
            "\n"
            "Exit status\n"
            "  0  scan completed (with or without duplicates)\n"
            "  2  invalid usage or unreadable folder\n"
            "  130 interrupted\n"
        ),
    )
    parser.add_argument("folder", help="folder to scan")
    parser.add_argument("-r", "--recursive", action="store_true",
                        help="also scan subfolders (directory symlinks are never followed)")
    parser.add_argument("-t", "--threshold", type=float, default=DEFAULT_THRESHOLD,
                        metavar="F",
                        help=f"perceptual similarity threshold in (0, 1]; higher is "
                             f"stricter (default: {DEFAULT_THRESHOLD})")
    parser.add_argument("--color-gate", type=float, default=DEFAULT_COLOR_GATE,
                        metavar="F",
                        help="max RMS distance between the per-block chromaticities of "
                             f"two images in [0, 1.414]; 0 disables colour checking "
                             f"(default: {DEFAULT_COLOR_GATE})")
    parser.add_argument("--no-progress", action="store_true",
                        help="do not print progress on stderr")
    args = parser.parse_args(argv)

    if not 0.0 < args.threshold <= 1.0:
        parser.error("--threshold must be in (0, 1]")
    if not 0.0 <= args.color_gate <= math.sqrt(2.0):
        parser.error("--color-gate must be in [0, 1.414]")
    return args


def _configure_streams() -> None:
    """Make output robust for Unicode paths.

    A console keeps its own encoding (UTF-8 on Windows consoles, the locale on
    POSIX); redirected streams are switched to UTF-8 so the report can be saved
    and read as UTF-8.  Unencodable characters are replaced, never fatal.
    """
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            if getattr(stream, "isatty", lambda: False)():
                reconfigure(errors="replace")
            else:
                reconfigure(encoding="utf-8", errors="replace")
        except (ValueError, OSError):
            pass


def main(argv: Sequence[str] | None = None) -> int:
    _configure_streams()
    args = parse_args(argv)

    root = os.path.normpath(os.path.abspath(args.folder))
    if not os.path.isdir(root):
        print(f"error: not a folder: {args.folder}", file=sys.stderr)
        return 2

    console = Console(progress=not args.no_progress)
    stats = Stats()
    started = time.monotonic()

    console.status("listing files...")
    files = list(iter_files(root, args.recursive, console, stats))
    console.status(f"{len(files):,} files found")

    signatures: list[Signature] = []
    inodes: dict[tuple[int, int], str] = {}

    for index, (path, info) in enumerate(files, start=1):
        console.update(index - 1, len(files), path)
        extension = os.path.splitext(path)[1].lower()
        if extension not in IMAGE_EXTENSIONS:
            stats.unsupported += 1
            continue

        inode = (info.st_dev, info.st_ino)
        if info.st_ino and inode in inodes:
            stats.hardlink_alias += 1
            console.warn(f"skipping hard link {path} (same file as {inodes[inode]})")
            continue

        try:
            signature = signature_for(path, info.st_size)
        except Exception as exc:            # corrupt/unsupported: skip and continue
            stats.unreadable += 1
            console.warn(f"skipping {path}: {type(exc).__name__}: {exc}")
            continue

        if info.st_ino:
            inodes[inode] = path
        signatures.append(signature)
        stats.processed += 1

    console.update(len(files), len(files), "done")
    console.finish()

    groups = build_groups(signatures, args.threshold, args.color_gate)

    report = format_report(
        root=root,
        recursive=args.recursive,
        threshold=args.threshold,
        color_gate=args.color_gate,
        stats=stats,
        signatures=signatures,
        groups=groups,
        console=console,
    )
    print(report)

    if console.warning_count or console.suppressed:
        print(f"({console.warning_count + console.suppressed} warning(s) on stderr)",
              file=sys.stderr)
    elapsed = time.monotonic() - started
    print(f"Scan finished in {elapsed:.2f}s.", file=sys.stderr)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        raise SystemExit(130)
