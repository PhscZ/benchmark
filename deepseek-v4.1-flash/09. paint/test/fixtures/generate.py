#!/usr/bin/env python3
"""Generate test fixtures for the raster editor.

Independent of the JavaScript implementation: expected pixel values are computed
here with numpy directly from the formulas documented in README.md, so the JS
implementation is checked against a second implementation rather than against
itself.

Run:  python test/fixtures/generate.py
"""
from __future__ import annotations

import json
import os

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))


# --------------------------------------------------------------------------
# EXIF orientation fixtures
# --------------------------------------------------------------------------
def quadrant_image() -> Image.Image:
    """8x4 stored image with four distinct quadrants (asymmetric)."""
    a = np.zeros((4, 8, 4), dtype=np.uint8)
    a[0:2, 0:4] = (255, 0, 0, 255)      # top-left    red
    a[0:2, 4:8] = (0, 255, 0, 255)      # top-right   green
    a[2:4, 0:4] = (0, 0, 255, 255)      # bottom-left blue
    a[2:4, 4:8] = (255, 255, 0, 255)    # bottom-right yellow
    return Image.fromarray(a, "RGBA").convert("RGB")


def exif_bytes(orientation: int) -> bytes:
    exif = Image.Exif()
    exif[0x0112] = orientation  # Orientation
    return exif.tobytes()


def write_oriented(name: str, orientation: int) -> None:
    img = quadrant_image()
    path = os.path.join(HERE, name)
    kw = dict(exif=exif_bytes(orientation))
    if name.endswith(".jpg"):
        img.save(path, format="JPEG", quality=100, subsampling=0, **kw)
    elif name.endswith(".webp"):
        img.save(path, format="WEBP", quality=100, lossless=True, **kw)
    elif name.endswith(".png"):
        img.save(path, format="PNG", **kw)
    else:
        raise ValueError(name)
    print("wrote", name)


# --------------------------------------------------------------------------
# Transparency fixture
# --------------------------------------------------------------------------
def write_alpha_png() -> None:
    a = np.zeros((4, 4, 4), dtype=np.uint8)
    a[0, 0] = (255, 0, 0, 0)        # fully transparent red
    a[0, 3] = (0, 255, 0, 255)
    a[3, 0] = (0, 0, 255, 255)
    a[3, 3] = (255, 255, 0, 255)
    a[1, 1] = (255, 255, 255, 128)  # half transparent
    a[2, 2] = (10, 20, 30, 200)
    Image.fromarray(a, "RGBA").save(os.path.join(HERE, "alpha.png"))
    print("wrote alpha.png")


def write_drawing_fixtures() -> None:
    """Opaque canvas for brush/shape tests."""
    solid = np.full((64, 64, 4), 255, dtype=np.uint8)
    solid[..., 0:3] = (32, 32, 32)
    Image.fromarray(solid, "RGBA").save(os.path.join(HERE, "solid.png"))

    """Left half transparent, right half opaque red: checkerboard + alpha tests."""
    half = np.zeros((64, 64, 4), dtype=np.uint8)
    half[:, 32:] = (255, 0, 0, 255)
    Image.fromarray(half, "RGBA").save(os.path.join(HERE, "alpha64.png"))

    """Four quadrants, no EXIF metadata: exact geometry checks."""
    quadrant_image().save(os.path.join(HERE, "quadrants.png"))
    print("wrote solid.png, alpha64.png, quadrants.png")


# --------------------------------------------------------------------------
# Colour-operation fixture (independent reference implementation)
# --------------------------------------------------------------------------
LUMA = (0.2126, 0.7152, 0.0722)


def clamp_u8(v: np.ndarray) -> np.ndarray:
    """Clamp to 0..255 with half-up rounding (matches JS Math.round)."""
    return np.clip(np.floor(v + 0.5), 0, 255).astype(np.uint8)


def apply_adjustments(rgb: np.ndarray, brightness: int, contrast: int,
                      grayscale: bool, invert: bool) -> np.ndarray:
    """Documented pipeline: brightness -> contrast -> grayscale -> invert.

    brightness k in [-100, 100]:  v' = v + k * 2.55
    contrast   k in [-100, 100]:  f = (100 + k) / 100 ; v' = (v - 127.5) * f + 127.5
    grayscale:                    Y = 0.2126R + 0.7152G + 0.0722B (Rec.709), all channels
    invert:                       v' = 255 - v
    Alpha is never modified.
    """
    out = rgb.astype(np.float64)
    if brightness:
        out = out + brightness * 2.55
    if contrast:
        f = (100.0 + contrast) / 100.0
        out = (out - 127.5) * f + 127.5
    out = clamp_u8(out)
    if grayscale:
        y = (out[..., 0] * LUMA[0] + out[..., 1] * LUMA[1] + out[..., 2] * LUMA[2])
        y = clamp_u8(y)
        out = np.stack([y, y, y], axis=-1)
    if invert:
        out = (255 - out).astype(np.uint8)
    return out


def write_color_fixture() -> None:
    w, h = 16, 8
    rng = np.random.default_rng(20260922)
    rgb = rng.integers(0, 256, size=(h, w, 3), dtype=np.uint8)
    # anchor values that exercise clamping and rounding boundaries
    anchors = [(0, 0, 0), (255, 255, 255), (127, 128, 129), (1, 2, 3),
               (254, 128, 0), (10, 200, 90), (128, 128, 128), (255, 0, 255)]
    for i, c in enumerate(anchors):
        rgb[0, i] = c
    alpha = np.full((h, w, 1), 255, dtype=np.uint8)
    alpha[1, 0] = 0
    alpha[1, 1] = 128
    alpha[2, 2] = 37

    rgba = np.concatenate([rgb, alpha], axis=-1)
    Image.fromarray(rgba, "RGBA").save(os.path.join(HERE, "color_fixture.png"))

    cases = [
        ("identity", dict(brightness=0, contrast=0, grayscale=False, invert=False)),
        ("brightness+25", dict(brightness=25, contrast=0, grayscale=False, invert=False)),
        ("brightness+10-halfstep", dict(brightness=10, contrast=0, grayscale=False, invert=False)),
        ("brightness-40", dict(brightness=-40, contrast=0, grayscale=False, invert=False)),
        ("brightness+100", dict(brightness=100, contrast=0, grayscale=False, invert=False)),
        ("contrast+30", dict(brightness=0, contrast=30, grayscale=False, invert=False)),
        ("contrast-50", dict(brightness=0, contrast=-50, grayscale=False, invert=False)),
        ("contrast+100", dict(brightness=0, contrast=100, grayscale=False, invert=False)),
        ("brightness+15/contrast-20", dict(brightness=15, contrast=-20, grayscale=False, invert=False)),
        ("grayscale", dict(brightness=0, contrast=0, grayscale=True, invert=False)),
        ("invert", dict(brightness=0, contrast=0, grayscale=False, invert=True)),
        ("grayscale+invert", dict(brightness=0, contrast=0, grayscale=True, invert=True)),
        ("brightness+40/contrast+50/grayscale", dict(brightness=40, contrast=50, grayscale=True, invert=False)),
    ]

    payload = {
        "image": "color_fixture.png",
        "width": w,
        "height": h,
        "alpha": alpha[..., 0].reshape(-1).tolist(),
        "input": rgb.reshape(-1).tolist(),
        "formulas": {
            "brightness": "v' = clamp(round(v + k * 2.55)), k in [-100,100], neutral 0",
            "contrast": "f = (100 + k) / 100 ; v' = clamp(round((v - 127.5) * f + 127.5)), k in [-100,100], neutral 0",
            "grayscale": "Y = clamp(round(0.2126R + 0.7152G + 0.0722B)); R=G=B=Y (Rec.709)",
            "invert": "v' = 255 - v",
            "order": "brightness -> contrast -> grayscale -> invert; alpha untouched",
        },
        "cases": [],
    }
    for name, opts in cases:
        expected = apply_adjustments(rgb, **opts)
        payload["cases"].append({
            "name": name,
            "opts": opts,
            "expected": expected.reshape(-1).tolist(),
        })

    with open(os.path.join(HERE, "color_fixture.expected.json"), "w") as fh:
        json.dump(payload, fh, separators=(",", ":"))
    print("wrote color_fixture.png + color_fixture.expected.json")


# --------------------------------------------------------------------------
# Error-path fixtures
# --------------------------------------------------------------------------
def write_bad_files() -> None:
    with open(os.path.join(HERE, "unsupported.bmp"), "wb") as fh:
        Image.fromarray(np.zeros((4, 4, 3), np.uint8), "RGB").save(fh, "BMP")
    print("wrote unsupported.bmp")

    with open(os.path.join(HERE, "corrupt.jpg"), "wb") as fh:
        fh.write(b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00")
        fh.write(b"not a real jpeg payload" * 8)
    print("wrote corrupt.jpg")


if __name__ == "__main__":
    for ext in ("jpg", "png", "webp"):
        write_oriented(f"orient1.{ext}", 1)
        write_oriented(f"orient3.{ext}", 3)
        write_oriented(f"orient6.{ext}", 6)
        write_oriented(f"orient8.{ext}", 8)
    write_alpha_png()
    write_drawing_fixtures()
    write_color_fixture()
    write_bad_files()
