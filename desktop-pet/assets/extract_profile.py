"""Prepare the right-facing profile as layered runtime assets.

The generated source has a baked dark background and overlapping legs.  The
background is removed with GrabCut.  The visible near leg is kept as one layer;
the far leg is kept as a second, rear layer.  The jacket hem hides their cut
line, so moving the layers never exposes a rectangular hole in the body.

Outputs:
  profile_right_body.png / profile_left_body.png
  profile_right_leg_far.png / profile_left_leg_far.png
  profile_right_leg_near.png / profile_left_leg_near.png
  profile_right.json / profile_left.json
"""

import json
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

SOURCE_SIZE = (943, 1667)
OUTPUT_SIZE = (625, 1105)
CONTENT_HEIGHT = 1000
CONTENT_TOP = 90

# Coordinates calibrated against source/stand_profile_right.png.
GRABCUT_RECT = (110, 70, 620, 1450)
LEGS_START_Y = 1015
LEGS_START_X = 375
NEAR_LEG = np.array(
    [[390, 1015], [515, 1015], [545, 1285], [510, 1340],
     [600, 1395], [600, 1498], [375, 1498], [375, 1300], [395, 1235]],
    dtype=np.int32,
)
FAR_LEG = np.array(
    [[495, 1015], [620, 1015], [610, 1305], [585, 1340],
     [675, 1390], [675, 1460], [500, 1460], [485, 1380], [495, 1280]],
    dtype=np.int32,
)


def foreground_mask(image: np.ndarray) -> np.ndarray:
    height, width = image.shape[:2]
    mask = np.full((height, width), cv2.GC_BGD, dtype=np.uint8)
    background = np.zeros((1, 65), dtype=np.float64)
    foreground = np.zeros((1, 65), dtype=np.float64)
    cv2.grabCut(
        image,
        mask,
        GRABCUT_RECT,
        background,
        foreground,
        8,
        cv2.GC_INIT_WITH_RECT,
    )
    binary = np.where(
        (mask == cv2.GC_FGD) | (mask == cv2.GC_PR_FGD), 255, 0
    ).astype(np.uint8)

    # Only the largest connected component is the character.  This also
    # discards isolated generator noise without nibbling at the soft outline.
    count, labels, stats, _ = cv2.connectedComponentsWithStats(binary, 8)
    if count <= 1:
        raise RuntimeError("profile foreground was not found")
    component = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    binary = np.where(labels == component, 255, 0).astype(np.uint8)
    binary = cv2.morphologyEx(binary, cv2.MORPH_CLOSE, np.ones((3, 3), np.uint8))
    return cv2.GaussianBlur(binary, (0, 0), 0.7)


def polygon_mask(alpha: np.ndarray, polygon: np.ndarray) -> np.ndarray:
    selection = np.zeros_like(alpha)
    cv2.fillPoly(selection, [polygon], 255)
    return cv2.bitwise_and(alpha, selection)


def place_on_runtime_canvas(
    rgb: np.ndarray,
    alpha: np.ndarray,
    bounds: tuple[int, int, int, int],
) -> tuple[np.ndarray, float, int, int]:
    x, y, width, height = bounds
    scale = CONTENT_HEIGHT / height
    target_width = round(width * scale)
    target_height = CONTENT_HEIGHT
    origin_x = (OUTPUT_SIZE[0] - target_width) // 2
    origin_y = CONTENT_TOP

    colour = cv2.resize(
        rgb[y : y + height, x : x + width],
        (target_width, target_height),
        interpolation=cv2.INTER_LANCZOS4,
    )
    matte = cv2.resize(
        alpha[y : y + height, x : x + width],
        (target_width, target_height),
        interpolation=cv2.INTER_LANCZOS4,
    )
    output = np.zeros((OUTPUT_SIZE[1], OUTPUT_SIZE[0], 4), dtype=np.uint8)
    output[origin_y : origin_y + target_height, origin_x : origin_x + target_width, :3] = colour
    output[origin_y : origin_y + target_height, origin_x : origin_x + target_width, 3] = matte
    return output, scale, origin_x, origin_y


def map_point(
    point: tuple[float, float],
    bounds: tuple[int, int, int, int],
    scale: float,
    origin_x: int,
    origin_y: int,
) -> tuple[float, float]:
    return (
        origin_x + (point[0] - bounds[0]) * scale,
        origin_y + (point[1] - bounds[1]) * scale,
    )


def main() -> None:
    base = Path(__file__).parent
    source_path = base / "source" / "stand_profile_right.png"
    bgr = cv2.imread(str(source_path), cv2.IMREAD_COLOR)
    if bgr is None:
        raise SystemExit(f"cannot read {source_path}")
    if (bgr.shape[1], bgr.shape[0]) != SOURCE_SIZE:
        raise SystemExit(
            f"calibrated for {SOURCE_SIZE}, source is {(bgr.shape[1], bgr.shape[0])}"
        )

    alpha = foreground_mask(bgr)
    bounds = cv2.boundingRect((alpha > 10).astype(np.uint8))
    rgb = cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB)
    near = polygon_mask(alpha, NEAR_LEG)
    far = polygon_mask(alpha, FAR_LEG)
    body = alpha.copy()
    rows, columns = np.ogrid[: alpha.shape[0], : alpha.shape[1]]
    body[(rows >= LEGS_START_Y) & (columns >= LEGS_START_X)] = 0

    transformed = {}
    scale = 1.0
    origin_x = origin_y = 0
    for name, matte in (("body", body), ("leg_far", far), ("leg_near", near)):
        layer, scale, origin_x, origin_y = place_on_runtime_canvas(rgb, matte, bounds)
        transformed[name] = layer
        Image.fromarray(layer, "RGBA").save(base / f"profile_right_{name}.png")
        Image.fromarray(np.flip(layer, axis=1).copy(), "RGBA").save(
            base / f"profile_left_{name}.png"
        )

    def mapped(point: tuple[float, float]) -> tuple[float, float]:
        return map_point(point, bounds, scale, origin_x, origin_y)

    far_pivot = mapped((550.0, 1025.0))
    near_pivot = mapped((450.0, 1025.0))
    manifest = {
        "sprite": "profile_right_body.png",
        "source_size": list(OUTPUT_SIZE),
        "visor": {"x": 369, "y": 273, "w": 131, "h": 204},
        "anchors": {
            # Only one eye is rendered in profile; these coordinates keep the
            # manifest compatible with the common face schema.
            "left_eye": [0.50, 0.43],
            "right_eye": [0.50, 0.43],
            "mouth": [0.66, 0.72],
        },
        "particle_anchors": {
            "left_ear": {"x": 318, "y": 191, "r": 55},
            "right_ear": {"x": 383, "y": 227, "r": 48},
            "tail": {"x": 204, "y": 807, "r": 62},
        },
        # The renderer draws the first leg behind the second one.
        "legs": {
            "left": {
                "sprite": "profile_right_leg_far.png",
                "pivot_x": round(far_pivot[0], 2),
                "pivot_y": round(far_pivot[1], 2),
                "origin_x": 0,
                "origin_y": 0,
            },
            "right": {
                "sprite": "profile_right_leg_near.png",
                "pivot_x": round(near_pivot[0], 2),
                "pivot_y": round(near_pivot[1], 2),
                "origin_x": 0,
                "origin_y": 0,
            },
        },
        "pockets": {},
        "ground": 1090,
    }
    (base / "profile_right.json").write_text(
        json.dumps(manifest, indent=2), encoding="utf-8"
    )

    left = json.loads(json.dumps(manifest))
    left["sprite"] = "profile_left_body.png"
    left["visor"]["x"] = OUTPUT_SIZE[0] - manifest["visor"]["x"] - manifest["visor"]["w"]
    for anchor in left["particle_anchors"].values():
        anchor["x"] = OUTPUT_SIZE[0] - anchor["x"]
    for side, filename in (("left", "profile_left_leg_far.png"),
                           ("right", "profile_left_leg_near.png")):
        left["legs"][side]["sprite"] = filename
        left["legs"][side]["pivot_x"] = round(
            OUTPUT_SIZE[0] - manifest["legs"][side]["pivot_x"], 2
        )
    (base / "profile_left.json").write_text(
        json.dumps(left, indent=2), encoding="utf-8"
    )
    print(
        f"profile: bounds={bounds}, scale={scale:.4f}, "
        f"near_pivot={near_pivot}, far_pivot={far_pivot}"
    )


if __name__ == "__main__":
    main()
