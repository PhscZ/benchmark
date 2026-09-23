# dupefinder

Find exact and near-duplicate JPEG / PNG / WebP images in a folder tree.

* **Read-only.** It never deletes, renames, moves or modifies anything.
* Finds **byte-identical** files *and* images that only look the same: different
  compression, metadata, container format, resolution, EXIF orientation, or a
  slightly changed brightness/colour.
* Reports groups with a **retained original** clearly marked, plus the size that
  deleting the other copies *would* free.
* Deterministic: the same files and settings always produce the same report.

---

## 1. Requirements

* Python **3.10 or newer** (uses `int.bit_count()`, `dataclasses(slots=True)`).
  Verified on CPython 3.13.3 / Windows 10.
* Pillow and NumPy, pinned in `requirements.txt` (Pillow 11.2.1, NumPy 2.3.0).
  No other dependencies; no compiled extensions of its own.

## 2. Setup

```bash
# Windows (cmd / PowerShell)
py -3 -m venv .venv
.venv\Scripts\python -m pip install --upgrade pip
.venv\Scripts\python -m pip install -r requirements.txt

# Linux / macOS
python3 -m venv .venv
.venv/bin/python -m pip install --upgrade pip
.venv/bin/python -m pip install -r requirements.txt
```

Or install into the current environment: `pip install -r requirements.txt`.

## 3. Run

```bash
python dupefinder.py <folder> [options]
```

| Option | Meaning |
| --- | --- |
| `folder` | Folder to scan (required). Spaces and Unicode are fine. |
| `-r`, `--recursive` | Also scan subfolders. Directory symlinks/junctions are **never** followed. |
| `-t F`, `--threshold F` | Perceptual similarity threshold, `(0, 1]`, default **0.90**. **Higher = stricter.** |
| `--color-gate F` | Max RMS block-chromaticity distance, default **0.20** (`0` disables colour checking). |
| `--no-progress` | Suppress the progress line on stderr. |

Examples:

```bash
python dupefinder.py "D:\Photos"
python dupefinder.py "D:\Photos" --recursive
python dupefinder.py "/home/me/图片" -r -t 0.95          # very strict
python dupefinder.py "/home/me/图片" -r -t 0.80          # loose, more false positives
python dupefinder.py "D:\Photos" -r --no-progress > report.txt
```

The report goes to **stdout**, progress and warnings go to **stderr**, so
`> report.txt` keeps the report clean. Output is UTF-8 encoded; on a Windows
console that cannot render a path character it is replaced instead of crashing.

Exit status: `0` scan completed (with or without duplicates), `2` bad usage or
unreadable folder, `130` interrupted.

## 4. Output

```
Duplicate groups : 2
Duplicate files  : 8 (2 byte-identical, 6 perceptual)

Total duplicate size: 82,646 bytes (80.71 KiB)
  Potential savings based on logical file sizes (the sum of the duplicate
  copies, excluding the retained original of each group).  This is not
  guaranteed disk space recovered: hard links, filesystem compression or
  existing block-level deduplication can make the real figure smaller.

Group 1 of 2 (7 duplicates, 80,616 bytes, 78.73 KiB)
  [KEEP]     400x300        21,609 B  C:\pics\rotated_exif.jpg
  [DUP ]     400x300         9,111 B  C:\pics\brighter.png
         perceptual match: distance 4/63, similarity 0.9365
  [DUP ]     400x300         8,711 B  C:\pics\base.png
         byte-identical to C:\pics\exact_copy.png (same SHA-256)
  [DUP ]     400x300         7,834 B  C:\pics\recompressed.jpg
         perceptual match: distance 1/63, similarity 0.9841
```

* `[KEEP]` is the retained original of the group, `[DUP ]` a duplicate of it.
* Dimensions are the **EXIF-normalised** pixel dimensions (a portrait photo
  stored rotated with an orientation tag is reported upright).
* `distance` is the Hamming distance of the 63-bit perceptual hash; `similarity`
  is `1 - distance / 63`. Both are measured **against the retained original**.
* `byte-identical to <path>` means the two files have the same SHA-256; it is
  reported even if that twin is not the retained original.
* `Total duplicate size` sums the logical sizes of the `[DUP ]` files only (the
  retained original of each group is excluded). Hard-linked aliases are counted
  once, so the real saving can be smaller.

Scan summary counters: `Files scanned` = every regular file encountered;
`Images processed` = files that decoded and were hashed; `Files skipped` =
`unsupported format` + `hard-link alias` + `unreadable/corrupt`
(so `scanned = processed + skipped`).

## 5. How it works

### 5.1 Exact duplicates

Every file is hashed with SHA-256 (streamed in 1 MiB chunks). Identical digests
mean identical bytes.

### 5.2 Perceptual signature (one compact record per image, no pixels kept)

Each decodable image is squashed to a 32x32 RGB thumbnail and reduced to:

1. **63-bit perceptual hash (pHash)** — the 8x8 low-frequency block of the 2-D
   DCT of the *mean/std-normalised* grayscale thumbnail, DC coefficient dropped,
   each remaining coefficient compared against `median + 2% of the strongest
   coefficient`. Resolution-independent, survives re-encoding, metadata
   stripping, format conversion, scaling, and affine brightness/contrast change.
2. **4x4 grid of block chromaticities** — `(r, g, b) / (r + g + b)` per block, so
   each value is brightness-invariant but still localised. Compared as an RMS
   distance.
3. **Mean luminance** — used only for a very loose gate.

### 5.3 Matching rule

Two images match when **all** of these hold:

| Test | Default | Meaning |
| --- | --- | --- |
| `similarity = 1 - hamming/63 >= threshold` | 0.90 → at most 7 differing bits | structure |
| RMS block-chromaticity distance `<= color-gate` | 0.20 | colour layout |
| mean-luminance ratio `<= 4x` | fixed | not black vs white |

The threshold is the only knob that needs tuning: **higher is stricter**
(`1.0` = bit-identical hashes only, `0.90` = default, `0.80` = loose).

### 5.4 Why the extra colour and luminance tests

The pHash works on *grayscale* structure, so two images with the same shape but
completely different colours (a red and a blue version of the same logo) have
nearly identical hashes. The 4x4 chromaticity grid rejects those while still
accepting a global tint or exposure change, because chromaticity is scale
invariant. The luminance ratio additionally rejects the degenerate case of a
solid black image "matching" a solid white one (both hash to all-zero bits).
Neither test looks for *similar* colours in unrelated images — structure does
that work — they only reject pairs whose colours genuinely disagree.

### 5.5 Transparency

Images with an alpha channel (`RGBA`, `LA`, palette + `transparency`) are
composited onto an **opaque white** background *before* any pixel is measured.
Consequences, all deliberate:

* a transparent PNG and the same artwork flattened onto white (JPEG or PNG)
  compare as duplicates — this is the common real-world case;
* two images that differ only in the colour *behind* fully transparent pixels
  compare as duplicates;
* an image flattened onto a dark background is a *different-looking* image and
  is not reported as a duplicate of the transparent original (measured on the
  test corpus: white flattening 5/63 bits, similarity 0.921; black flattening
  18/63 bits, similarity 0.714).
* JPEG has no alpha channel, so nothing changes for opaque inputs.

### 5.6 EXIF orientation

`ImageOps.exif_transpose()` is applied before hashing, and the reported
dimensions come from the file header with width/height swapped for orientations
5-8. A photo stored rotated with an orientation tag therefore matches its
upright copy (measured: 2/63 bits apart at most in testing).

### 5.7 Grouping

1. Images are sorted by the ranking rule **largest pixel area → largest file
   size → lexicographically smallest full path**.
2. Walking that order, the first not-yet-assigned image becomes a group's
   retained original; every remaining image that matches **that original
   directly** joins the group. Then the walk continues.

This gives **star-shaped** groups: each listed duplicate is within the threshold
of its own group's retained original, matches are never chained through an
intermediate image (`A~B`, `B~C`, but `A!~C` yields `{A,B}` and `C` is not
reported), and every file belongs to at most one group. Because the order is a
total order (paths are unique), the result is deterministic.

### 5.8 Candidate filtering (why it is not O(n²))

All hashes are inserted into a **Burkhard-Keller tree** over the 63-bit hash
with the Hamming metric. For each prospective original the tree is queried with
radius `ceil(63 * (1 - threshold))` (7 bits at the default), which returns
*every* stored image inside that radius — unlike hash-bucket/LSH schemes this
cannot hide a true match. Only those candidates get the full similarity test, so
the number of full comparisons is roughly `n * (matches + small)`, not `n²/2`.

## 6. Performance and limitations

* Roughly **6-9 ms per image** on the development machine (AMD FX-8300),
  dominated by JPEG/PNG decoding; single-threaded. Measured: 2 000 images in
  12.8 s, 360 images in 3.4 s.
* Memory is **compact and incremental**: images are decoded one at a time and
  dropped immediately; only ~400 bytes of retained comparison data per image
  (path, dimensions, size, 32-byte digest, 63-bit hash, 16 chromaticity triples)
  are kept. Measured peak Python allocation for a 360-image scan: 3.6 MB. Huge
  JPEGs are additionally decoded with Pillow's `draft()` at a reduced scale.
* **Not handled:** cropping, watermarks, arbitrary rotation, mirroring, and
  heavy blur are outside the model — such pairs may or may not be reported, and
  nothing is guaranteed. Only quarter-turn EXIF rotation is normalised.
* Unrelated images that share *both* a coarse structure and a colour layout can
  still collide; raise `-t` towards 0.95 if you see that.
* Very large libraries (≫10⁵ images) make the BK-tree queries and the report
  itself slower; the algorithm stays sub-quadratic but is not parallelised.
* Pillow's decompression-bomb guard applies: images above ~89 MPixels raise a
  warning/error and are skipped as unreadable.

## 7. Safety and edge cases

* Opens files read-only (`rb`) and never writes, moves or deletes anything.
  Verified by comparing size/mtime of every file before and after a scan.
* **Hard links to the same underlying file are counted once** (`st_dev`/`st_ino`
  where the filesystem provides them; the later path is reported as a skipped
  `hard-link alias`). On filesystems without inode numbers this dedup is skipped.
* Directory **symlinks and NTFS junctions are never followed**; a directory
  reached twice is scanned once, so link loops cannot hang the scan.
* Unsupported, unreadable or corrupted files produce a warning on stderr and are
  skipped; the scan always continues. At most 50 warnings are printed, the rest
  are counted in the summary.
* Empty folders, folders with no images, and folders with no duplicates are all
  handled and reported.
* Progress is shown on stderr: an updating line on a terminal, one line every
  ~2 s when redirected.
