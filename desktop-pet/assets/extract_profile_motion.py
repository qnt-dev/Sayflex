"""Build the articulated right/left profile rig from the arms-out source.

The generated source is a single composited illustration, not a true layered
file.  This script turns it into a deterministic runtime rig: background,
arms and legs are segmented once at build time; the application only blends
small prepared PNG layers and never performs image analysis while running.

Outputs:
  profile_motion_{right,left}_body.png
  profile_motion_{right,left}_{arm,leg}_{far,near}.png
  profile_motion_{right,left}.json
"""

import json
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

SOURCE_SIZE = (944, 1667)
OUTPUT_SIZE = (625, 1105)
CONTENT_HEIGHT = 1000
CONTENT_TOP = 90
GRABCUT_RECT = (80, 65, 760, 1450)

# Calibrated against source/profile_arms_right.png.  The first limb in each
# pair is visually farther from the viewer and is rendered behind the torso.
ARM_FAR = np.array(
    [
        [305, 660], [420, 675], [403, 790], [360, 900], [326, 1015],
        [275, 1085], [200, 1058], [215, 965], [242, 875], [255, 760],
    ],
    dtype=np.int32,
)
ARM_NEAR = np.array(
    [
        [500, 660], [590, 680], [611, 785], [657, 862], [727, 918],
        [795, 966], [785, 1035], [735, 1060], [674, 1018], [623, 950],
        [550, 904], [520, 795],
    ],
    dtype=np.int32,
)
LEG_FAR = np.array(
    [
        [330, 1010], [505, 1005], [533, 1230], [505, 1340], [535, 1415],
        [515, 1495], [285, 1495], [275, 1405], [315, 1315],
    ],
    dtype=np.int32,
)
LEG_NEAR = np.array(
    [
        [460, 1000], [635, 1000], [650, 1235], [628, 1340], [710, 1400],
        [720, 1495], [480, 1495], [455, 1400], [475, 1320],
    ],
    dtype=np.int32,
)

# The jacket exists underneath the shoulder caps.  Keeping this central area
# in the body layer prevents transparent cracks when an arm rotates away.
TORSO_CORE = np.array(
    [[305, 650], [565, 650], [620, 1045], [300, 1045]], dtype=np.int32
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
    count, labels, stats, _ = cv2.connectedComponentsWithStats(binary, 8)
    if count <= 1:
        raise RuntimeError("motion profile foreground was not found")
    component = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    binary = np.where(labels == component, 255, 0).astype(np.uint8)
    binary = cv2.morphologyEx(binary, cv2.MORPH_CLOSE, np.ones((3, 3), np.uint8))
    return cv2.GaussianBlur(binary, (0, 0), 0.7)


def selection(alpha: np.ndarray, polygon: np.ndarray) -> np.ndarray:
    matte = np.zeros_like(alpha)
    cv2.fillPoly(matte, [polygon], 255)
    return cv2.bitwise_and(alpha, matte)


def runtime_layer(
    rgb: np.ndarray,
    alpha: np.ndarray,
    bounds: tuple[int, int, int, int],
) -> tuple[np.ndarray, float, int, int]:
    x, y, width, height = bounds
    scale = CONTENT_HEIGHT / height
    target_width = round(width * scale)
    origin_x = (OUTPUT_SIZE[0] - target_width) // 2
    origin_y = CONTENT_TOP
    colour = cv2.resize(
        rgb[y : y + height, x : x + width],
        (target_width, CONTENT_HEIGHT),
        interpolation=cv2.INTER_LANCZOS4,
    )
    matte = cv2.resize(
        alpha[y : y + height, x : x + width],
        (target_width, CONTENT_HEIGHT),
        interpolation=cv2.INTER_LANCZOS4,
    )
    output = np.zeros((OUTPUT_SIZE[1], OUTPUT_SIZE[0], 4), dtype=np.uint8)
    output[origin_y : origin_y + CONTENT_HEIGHT, origin_x : origin_x + target_width, :3] = colour
    output[origin_y : origin_y + CONTENT_HEIGHT, origin_x : origin_x + target_width, 3] = matte
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
    source_path = base / "source" / "profile_arms_right.png"
    bgr = cv2.imread(str(source_path), cv2.IMREAD_COLOR)
    if bgr is None:
        raise SystemExit(f"cannot read {source_path}")
    actual_size = (bgr.shape[1], bgr.shape[0])
    if actual_size != SOURCE_SIZE:
        raise SystemExit(f"calibrated for {SOURCE_SIZE}, source is {actual_size}")

    alpha = foreground_mask(bgr)
    bounds = cv2.boundingRect((alpha > 10).astype(np.uint8))
    rgb = cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB)
    arm_far = selection(alpha, ARM_FAR)
    arm_near = selection(alpha, ARM_NEAR)
    leg_far = selection(alpha, LEG_FAR)
    leg_near = selection(alpha, LEG_NEAR)

    torso = np.zeros_like(alpha)
    cv2.fillPoly(torso, [TORSO_CORE], 255)
    movable = cv2.bitwise_or(
        cv2.bitwise_or(arm_far, arm_near), cv2.bitwise_or(leg_far, leg_near)
    )
    # Remove limbs outside the central jacket only.  The retained shoulder
    # caps hide the cut edge during the modest rotation used by the gait.
    removable = cv2.bitwise_and(movable, cv2.bitwise_not(torso))
    body = cv2.bitwise_and(alpha, cv2.bitwise_not(removable))

    layers = {
        "body": body,
        "arm_far": arm_far,
        "arm_near": arm_near,
        "leg_far": leg_far,
        "leg_near": leg_near,
    }
    transformed: dict[str, np.ndarray] = {}
    scale = 1.0
    origin_x = origin_y = 0
    for name, matte in layers.items():
        layer, scale, origin_x, origin_y = runtime_layer(rgb, matte, bounds)
        transformed[name] = layer
        Image.fromarray(layer, "RGBA").save(base / f"profile_motion_right_{name}.png")
        Image.fromarray(np.flip(layer, axis=1).copy(), "RGBA").save(
            base / f"profile_motion_left_{name}.png"
        )

    mapped = lambda point: map_point(point, bounds, scale, origin_x, origin_y)
    visor_top_left = mapped((478.0, 315.0))
    visor_bottom_right = mapped((665.0, 650.0))
    pivots = {
        "arm_far": mapped((330.0, 700.0)),
        "arm_near": mapped((540.0, 700.0)),
        "leg_far": mapped((420.0, 1030.0)),
        "leg_near": mapped((535.0, 1030.0)),
    }

    def limb(filename: str, pivot: tuple[float, float]) -> dict[str, float | str]:
        return {
            "sprite": filename,
            "pivot_x": round(pivot[0], 2),
            "pivot_y": round(pivot[1], 2),
            "origin_x": 0,
            "origin_y": 0,
        }

    manifest = {
        "sprite": "profile_motion_right_body.png",
        "source_size": list(OUTPUT_SIZE),
        "visor": {
            "x": round(visor_top_left[0]),
            "y": round(visor_top_left[1]),
            "w": round(visor_bottom_right[0] - visor_top_left[0]),
            "h": round(visor_bottom_right[1] - visor_top_left[1]),
        },
        "anchors": {
            "left_eye": [0.50, 0.54],
            "right_eye": [0.50, 0.54],
            "mouth": [0.66, 0.74],
        },
        "particle_anchors": {},
        "legs": {
            "left": limb("profile_motion_right_leg_far.png", pivots["leg_far"]),
            "right": limb("profile_motion_right_leg_near.png", pivots["leg_near"]),
        },
        "arms": {
            "far": limb("profile_motion_right_arm_far.png", pivots["arm_far"]),
            "near": limb("profile_motion_right_arm_near.png", pivots["arm_near"]),
        },
        "pockets": {},
        "ground": 1090,
    }
    (base / "profile_motion_right.json").write_text(
        json.dumps(manifest, indent=2), encoding="utf-8"
    )

    left = json.loads(json.dumps(manifest))
    left["sprite"] = "profile_motion_left_body.png"
    left["visor"]["x"] = OUTPUT_SIZE[0] - manifest["visor"]["x"] - manifest["visor"]["w"]
    for group, names in (("legs", ("left", "right")), ("arms", ("far", "near"))):
        for name in names:
            right_name = left[group][name]["sprite"]
            left[group][name]["sprite"] = right_name.replace("_right_", "_left_")
            left[group][name]["pivot_x"] = round(
                OUTPUT_SIZE[0] - manifest[group][name]["pivot_x"], 2
            )
    (base / "profile_motion_left.json").write_text(
        json.dumps(left, indent=2), encoding="utf-8"
    )
    print(f"motion profile: bounds={bounds}, scale={scale:.4f}, pivots={pivots}")


if __name__ == "__main__":
    main()
