"""Prepare authored walk-cycle frames from generated contact sheets.

The supplied PNG files contain a baked white/checkerboard background.  This
script removes that background deterministically, keeps each candidate pack
separate, orders it as contact/down/passing/up for both legs, and writes every
frame onto one shared source canvas.  Runtime code can therefore change packs
without changing scale, ground contact, or window geometry.

Run from the desktop-pet workspace:

    python assets/prepare_generated_walk.py
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import cv2
import numpy as np
from PIL import Image


ROOT = Path(__file__).resolve().parent
SOURCE = ROOT / "source"
OUTPUT = ROOT
QA_OUTPUT = ROOT.parent / "dist" / "qa" / "generated-walk"

CANVAS_WIDTH = 896
CANVAS_HEIGHT = 1105
GROUND_Y = 1090
TARGET_HEIGHT = 980
TORSO_X = 480
# A complete authored cycle contains two steps.  The painted contact poses are
# roughly 250 source pixels apart per step, so the body should travel twice
# that distance before the eight-frame sequence repeats.  Keeping this value
# in the asset manifest prevents a procedural leg rig from accidentally
# driving the authored art at almost double speed.
CYCLE_TRAVEL = 500
# Authored frames own the hips and legs only.  The stable profile torso and
# two independent arm layers are rendered at runtime, so baked arms can never
# blink or repeat the same lead on both steps.  The jacket is drawn over this
# overlap and hides the deliberately soft cut.
LOWER_CUT_Y = 690
LOWER_BLEND = 22
TAIL_EXCLUSION_X = 340
TAIL_EXCLUSION_Y = 880

PHASES = (
    "contact_a",
    "down_a",
    "passing_a",
    "up_a",
    "contact_b",
    "down_b",
    "passing_b",
    "up_b",
)


@dataclass(frozen=True)
class Pack:
    name: str
    source: str
    rows: tuple[int, ...]
    order: tuple[int, ...]
    has_alpha: bool = False


# The generators produced useful poses in the wrong temporal order.  These
# mappings are deliberately explicit: source art is never silently reordered
# by a similarity heuristic whose result could change with a library update.
PACKS = (
    Pack("a", "generated_walk_a.png", (3, 3, 3), (0, 4, 1, 6, 3, 5, 7, 2)),
    Pack("b", "generated_walk_b.png", (8,), (1, 0, 2, 3, 6, 7, 5, 4)),
    Pack("c", "generated_walk_c.png", (5, 5, 4), (0, 5, 6, 7, 9, 13, 11, 4)),
    Pack("d", "walk_cycle_d_alpha-v2.png", (4, 4), tuple(range(8)), True),
)


def split_cells(image: np.ndarray, columns_per_row: tuple[int, ...]) -> list[np.ndarray]:
    height, width = image.shape[:2]
    cells: list[np.ndarray] = []
    for row, columns in enumerate(columns_per_row):
        y0 = round(row * height / len(columns_per_row))
        y1 = round((row + 1) * height / len(columns_per_row))
        for column in range(columns):
            x0 = round(column * width / columns)
            x1 = round((column + 1) * width / columns)
            cells.append(image[y0:y1, x0:x1].copy())
    return cells


def largest_component(mask: np.ndarray) -> np.ndarray:
    count, labels, stats, _ = cv2.connectedComponentsWithStats(mask, 8)
    if count <= 1:
        raise RuntimeError("foreground segmentation produced no component")
    label = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    return np.asarray(labels == label, dtype=np.uint8)


def remove_baked_background(cell_rgb: np.ndarray) -> np.ndarray:
    """Return straight-alpha RGBA without regenerating or repainting the art."""

    bgr = cv2.cvtColor(cell_rgb, cv2.COLOR_RGB2BGR)
    hsv = cv2.cvtColor(bgr, cv2.COLOR_BGR2HSV)
    saturation = hsv[:, :, 1]
    value = hsv[:, :, 2]

    # Both supplied backgrounds are neutral.  Saturated violet/cyan pixels and
    # the dark visor/clothes give GrabCut unambiguous seeds; the remaining edge
    # pixels are solved as probable foreground so antialiasing is retained.
    mask = np.full(cell_rgb.shape[:2], cv2.GC_PR_BGD, dtype=np.uint8)
    mask[(saturation > 35) | (value < 125)] = cv2.GC_PR_FGD
    mask[(saturation > 70) | (value < 70)] = cv2.GC_FGD
    border = max(3, min(cell_rgb.shape[:2]) // 50)
    mask[:border, :] = cv2.GC_BGD
    mask[-border:, :] = cv2.GC_BGD
    mask[:, :border] = cv2.GC_BGD
    mask[:, -border:] = cv2.GC_BGD

    background_model = np.zeros((1, 65), dtype=np.float64)
    foreground_model = np.zeros((1, 65), dtype=np.float64)
    cv2.grabCut(
        bgr,
        mask,
        None,
        background_model,
        foreground_model,
        6,
        cv2.GC_INIT_WITH_MASK,
    )
    binary = np.asarray(
        np.isin(mask, (cv2.GC_FGD, cv2.GC_PR_FGD)), dtype=np.uint8
    )
    binary = largest_component(binary)
    binary = cv2.morphologyEx(binary, cv2.MORPH_CLOSE, np.ones((3, 3), np.uint8))

    # A narrow signed-distance ramp produces stable sub-pixel edges.  No image
    # synthesis is involved: RGB samples remain exactly those supplied by the
    # user, only the baked neutral backing is removed.
    inside = cv2.distanceTransform(binary, cv2.DIST_L2, 5)
    outside = cv2.distanceTransform(1 - binary, cv2.DIST_L2, 5)
    alpha = np.clip((inside - outside + 1.15) / 2.30, 0.0, 1.0)
    rgba = np.dstack((cell_rgb, np.rint(alpha * 255.0).astype(np.uint8)))
    return rgba


def content_bounds(rgba: np.ndarray, threshold: int = 20) -> tuple[int, int, int, int]:
    ys, xs = np.where(rgba[:, :, 3] > threshold)
    if not len(xs):
        raise RuntimeError("frame contains no opaque pixels")
    return int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1


def torso_centre_x(rgba: np.ndarray) -> float:
    x0, y0, x1, y1 = content_bounds(rgba)
    height = y1 - y0
    # The tail and feet are intentionally excluded.  This anchors the ribcage,
    # not the changing silhouette, and prevents the body sliding sideways.
    region = rgba[y0 : y0 + round(height * 0.64), x0:x1, 3] > 40
    ys, xs = np.where(region)
    if not len(xs):
        return (x0 + x1) * 0.5
    return x0 + float(np.median(xs))


def place_on_shared_canvas(rgba: np.ndarray, scale: float) -> np.ndarray:
    x0, y0, x1, y1 = content_bounds(rgba)
    cropped = rgba[y0:y1, x0:x1]
    target_size = (
        max(1, round(cropped.shape[1] * scale)),
        max(1, round(cropped.shape[0] * scale)),
    )
    resized = np.asarray(
        Image.fromarray(cropped, "RGBA").resize(target_size, Image.Resampling.LANCZOS)
    )

    source_torso = (torso_centre_x(rgba) - x0) * scale
    left = round(TORSO_X - source_torso)
    top = GROUND_Y - resized.shape[0]
    if left < 0 or left + resized.shape[1] > CANVAS_WIDTH or top < 0:
        raise RuntimeError(
            f"normalized frame does not fit canvas: left={left}, top={top}, "
            f"size={resized.shape[1]}x{resized.shape[0]}"
        )
    canvas = np.zeros((CANVAS_HEIGHT, CANVAS_WIDTH, 4), dtype=np.uint8)
    canvas[top : top + resized.shape[0], left : left + resized.shape[1]] = resized
    return canvas


def visor_rect(rgba: np.ndarray) -> dict[str, int]:
    """Locate the dark profile visor in the upper half of a normalized frame."""

    x0, y0, x1, y1 = content_bounds(rgba)
    rgb = rgba[:, :, :3]
    hsv = cv2.cvtColor(rgb, cv2.COLOR_RGB2HSV)
    value = hsv[:, :, 2]
    saturation = hsv[:, :, 1]
    search = np.zeros(rgba.shape[:2], dtype=np.uint8)
    upper = y0 + round((y1 - y0) * 0.405)
    # In a right-facing profile the visor is the dark opening ahead of the
    # ribcage.  Excluding the rear half of the hood prevents the tall black ear
    # insert from being merged into the face rectangle.
    search[y0:upper, max(x0, TORSO_X + 25) : x1] = 1
    dark = np.asarray(
        (value < 58) & (saturation > 18) & (rgba[:, :, 3] > 80) & (search > 0),
        dtype=np.uint8,
    )
    dark = cv2.morphologyEx(dark, cv2.MORPH_CLOSE, np.ones((3, 3), np.uint8))
    count, labels, stats, _ = cv2.connectedComponentsWithStats(dark, 8)
    candidates = []
    for label in range(1, count):
        x, y, w, h, area = (int(value) for value in stats[label])
        if area >= 250 and h >= 30 and w >= 20:
            candidates.append((area, x, y, w, h))
    if not candidates:
        raise RuntimeError("could not locate profile visor")
    _, x, y, w, h = max(candidates)
    pad_x = max(2, round(w * 0.05))
    pad_y = max(2, round(h * 0.04))
    return {
        "x": max(0, x - pad_x),
        "y": max(0, y - pad_y),
        "w": min(CANVAS_WIDTH - x + pad_x, w + pad_x * 2),
        "h": min(CANVAS_HEIGHT - y + pad_y, h + pad_y * 2),
    }


def mirror(rgba: np.ndarray) -> np.ndarray:
    return np.ascontiguousarray(rgba[:, ::-1])


def authored_lower_body(rgba: np.ndarray) -> np.ndarray:
    """Keep authored hips/legs while excluding baked torso, arms and tail."""

    height, width = rgba.shape[:2]
    ys = np.arange(height, dtype=np.float32)[:, None]
    xs = np.arange(width, dtype=np.float32)[None, :]
    vertical = np.clip(
        (ys - (LOWER_CUT_Y - LOWER_BLEND)) / float(LOWER_BLEND), 0.0, 1.0
    )
    # The right-facing tail occupies the upper-left quadrant.  Below this
    # height the boots are allowed to use the complete stride width.
    tail = (ys < TAIL_EXCLUSION_Y) & (xs < TAIL_EXCLUSION_X)
    keep = np.where(tail, 0.0, vertical)
    lower = rgba.copy()
    lower[:, :, 3] = np.rint(lower[:, :, 3].astype(np.float32) * keep).astype(
        np.uint8
    )
    return lower


def write_qa_preview(pack: Pack, frames: list[np.ndarray]) -> None:
    """Write the exact runtime frames as a contact sheet and looping GIF."""

    QA_OUTPUT.mkdir(parents=True, exist_ok=True)
    preview_size = (224, 276)
    backdrop = (244, 247, 250, 255)
    previews = []
    for frame in frames:
        sprite = Image.fromarray(frame, "RGBA").resize(
            preview_size, Image.Resampling.LANCZOS
        )
        preview = Image.new("RGBA", preview_size, backdrop)
        preview.alpha_composite(sprite)
        previews.append(preview.convert("RGB"))

    sheet = Image.new("RGB", (preview_size[0] * 4, preview_size[1] * 2), backdrop[:3])
    for index, preview in enumerate(previews):
        sheet.paste(
            preview,
            ((index % 4) * preview_size[0], (index // 4) * preview_size[1]),
        )
    sheet.save(QA_OUTPUT / f"pack-{pack.name}-runtime.png", optimize=True)
    previews[0].save(
        QA_OUTPUT / f"pack-{pack.name}-runtime.gif",
        save_all=True,
        append_images=previews[1:],
        duration=110,
        loop=0,
        optimize=False,
        disposal=2,
    )


def prepare(pack: Pack) -> None:
    source_path = SOURCE / pack.source
    source = np.asarray(Image.open(source_path).convert("RGBA" if pack.has_alpha else "RGB"))
    cells = split_cells(source, pack.rows)
    extracted = {}
    for index in sorted(set(pack.order)):
        cell = cells[index]
        extracted[index] = cell if pack.has_alpha else remove_baked_background(cell)
    ordered = [extracted[index] for index in pack.order]
    heights = [content_bounds(frame)[3] - content_bounds(frame)[1] for frame in ordered]
    scale = TARGET_HEIGHT / float(np.median(heights))

    frames = []
    qa_frames = []
    for index, (phase, source_index, frame) in enumerate(
        zip(PHASES, pack.order, ordered, strict=True)
    ):
        right = place_on_shared_canvas(frame, scale)
        left = mirror(right)
        right_name = f"walk_{pack.name}_right_{index:02}.png"
        left_name = f"walk_{pack.name}_left_{index:02}.png"
        Image.fromarray(right, "RGBA").save(OUTPUT / right_name, optimize=True)
        Image.fromarray(left, "RGBA").save(OUTPUT / left_name, optimize=True)
        qa_frames.append(right)
        right_visor = visor_rect(right)
        left_visor = {
            **right_visor,
            "x": CANVAS_WIDTH - right_visor["x"] - right_visor["w"],
        }
        entry = {
                "phase": phase,
                "source_index": source_index,
                "right": right_name,
                "left": left_name,
                "right_visor": right_visor,
                "left_visor": left_visor,
            }
        if pack.name == "d":
            right_lower = authored_lower_body(right)
            left_lower = mirror(right_lower)
            right_lower_name = f"walk_{pack.name}_lower_right_{index:02}.png"
            left_lower_name = f"walk_{pack.name}_lower_left_{index:02}.png"
            Image.fromarray(right_lower, "RGBA").save(
                OUTPUT / right_lower_name, optimize=True
            )
            Image.fromarray(left_lower, "RGBA").save(
                OUTPUT / left_lower_name, optimize=True
            )
            entry["right_lower"] = right_lower_name
            entry["left_lower"] = left_lower_name
        frames.append(entry)

    manifest = {
        "source_size": [CANVAS_WIDTH, CANVAS_HEIGHT],
        "ground": GROUND_Y,
        "cycle_travel": CYCLE_TRAVEL,
        "face_anchors": {
            "eye": [0.52, 0.52],
            "mouth": [0.66, 0.76],
        },
        "frames": frames,
    }
    (OUTPUT / f"walk_cycle_{pack.name}.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    write_qa_preview(pack, qa_frames)
    print(
        f"prepared pack {pack.name}: order={pack.order}, "
        f"source scale={scale:.3f}, output={OUTPUT}"
    )


def main() -> None:
    for pack in PACKS:
        prepare(pack)


if __name__ == "__main__":
    main()
