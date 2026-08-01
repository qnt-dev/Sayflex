"""Build the runtime front pose from the approved front rig master.

The approved master already contains a complete neutral character and a blank
visor.  Standing therefore uses one intact sprite: movement transitions to the
articulated profile rig before a stride becomes visible.  Keeping the front
pose whole avoids destructive pseudo-rigging and preserves the approved art.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

SOURCE_NAME = "rig_master_front.png"
SOURCE_SHA256 = "A886E04BB4A95F361E167DB7093181333E86DF64283C04F3A41FB40457B02796"
SOURCE_SIZE = (1024, 1536)
OUTPUT_SIZE = (625, 1105)
CONTENT_HEIGHT = 1000
CONTENT_TOP = 90
ALPHA_FLOOR = 10

# Coordinates in the approved 1024 x 1536 source plate.
VISOR_TOP_LEFT = (345.0, 380.0)
VISOR_BOTTOM_RIGHT = (685.0, 610.0)
PARTICLE_POINTS = {
    "left_ear": (275.0, 275.0, 74.0),
    "right_ear": (755.0, 275.0, 74.0),
    "tail": (825.0, 1085.0, 88.0),
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def approved_rgba(path: Path) -> np.ndarray:
    actual_hash = sha256(path)
    if actual_hash != SOURCE_SHA256:
        raise SystemExit(
            f"{path.name} is not the approved master: {actual_hash} != {SOURCE_SHA256}"
        )
    image = Image.open(path).convert("RGBA")
    if image.size != SOURCE_SIZE:
        raise SystemExit(f"calibrated for {SOURCE_SIZE}, source is {image.size}")
    rgba = np.asarray(image).copy()
    visible = (rgba[..., 3] > ALPHA_FLOOR).astype(np.uint8)
    count, labels, stats, _ = cv2.connectedComponentsWithStats(visible, 8)
    if count <= 1:
        raise SystemExit("approved front foreground was not found")
    component = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    keep = labels == component
    rgba[~keep] = 0
    rgba[..., 3] = np.where(keep, rgba[..., 3], 0)
    return rgba


def map_point(
    point: tuple[float, float],
    bounds: tuple[int, int, int, int],
    scale: float,
    origin_x: int,
) -> tuple[float, float]:
    return (
        origin_x + (point[0] - bounds[0]) * scale,
        CONTENT_TOP + (point[1] - bounds[1]) * scale,
    )


def main() -> None:
    base = Path(__file__).parent
    source_path = base / "source" / SOURCE_NAME
    rgba = approved_rgba(source_path)
    alpha = rgba[..., 3]
    bounds = cv2.boundingRect((alpha > ALPHA_FLOOR).astype(np.uint8))
    x, y, width, height = bounds
    scale = CONTENT_HEIGHT / height
    target_width = round(width * scale)
    origin_x = (OUTPUT_SIZE[0] - target_width) // 2

    colour = cv2.resize(
        rgba[y : y + height, x : x + width, :3],
        (target_width, CONTENT_HEIGHT),
        interpolation=cv2.INTER_LANCZOS4,
    )
    matte = cv2.resize(
        alpha[y : y + height, x : x + width],
        (target_width, CONTENT_HEIGHT),
        interpolation=cv2.INTER_LANCZOS4,
    )
    output = np.zeros((OUTPUT_SIZE[1], OUTPUT_SIZE[0], 4), dtype=np.uint8)
    target = output[CONTENT_TOP : CONTENT_TOP + CONTENT_HEIGHT, origin_x : origin_x + target_width]
    target[..., :3] = colour
    target[..., 3] = matte
    Image.fromarray(output, "RGBA").save(base / "stand_body.png")

    visor_top_left = map_point(VISOR_TOP_LEFT, bounds, scale, origin_x)
    visor_bottom_right = map_point(VISOR_BOTTOM_RIGHT, bounds, scale, origin_x)
    particles = {}
    for name, (source_x, source_y, source_radius) in PARTICLE_POINTS.items():
        mapped = map_point((source_x, source_y), bounds, scale, origin_x)
        particles[name] = {
            "x": round(mapped[0]),
            "y": round(mapped[1]),
            "r": round(source_radius * scale),
        }

    manifest = {
        "sprite": "stand_body.png",
        "source_size": list(OUTPUT_SIZE),
        "visor": {
            "x": round(visor_top_left[0]),
            "y": round(visor_top_left[1]),
            "w": round(visor_bottom_right[0] - visor_top_left[0]),
            "h": round(visor_bottom_right[1] - visor_top_left[1]),
        },
        "anchors": {
            "left_eye": [0.283, 0.410],
            "right_eye": [0.740, 0.494],
            "mouth": [0.510, 0.764],
        },
        "particle_anchors": particles,
        "legs": {},
        "pockets": {},
        "ground": 1090,
    }
    (base / "stand.json").write_text(
        json.dumps(manifest, indent=2), encoding="utf-8"
    )
    print(
        f"front master: sha256={SOURCE_SHA256} bounds={bounds} "
        f"scale={scale:.4f} visor={manifest['visor']}"
    )


if __name__ == "__main__":
    main()
