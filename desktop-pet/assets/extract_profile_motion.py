"""Build the articulated profile rig from the approved profile master.

The approved body source is a flattened RGBA illustration, not a layered rig.
This build-time step removes faint generated backdrop residue, keeps the
character's largest alpha component, and combines it with the separately
approved two-arm source.
The jacket and pelvis remain in the body layer.  The tail is emitted as one
continuous articulated layer with an overlap under the jacket, so it can wag
without pulling the torso mesh or opening a transparent seam at its root.

Runtime output is normalized to the same 625 x 1105 coordinate system used by
the front pose.  No image analysis or resampling happens in the application.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

SOURCE_NAME = "rig_master_profile_right.png"
SOURCE_SHA256 = "0043535F4DFE4C26DC1908FEB78D28B4B7EABD89B4099CCDA26CBBEA618FE81F"
SOURCE_SIZE = (1024, 1536)
ARMS_SOURCE_NAME = "profile_arms_right.png"
ARMS_SOURCE_SHA256 = "5F2E7DAE42CE8D420CBC3266F041E1231EDA0D540EF2BAAFB45091A757D64CB2"
ARMS_SOURCE_SIZE = (944, 1667)
ARMS_GRABCUT_RECT = (80, 65, 760, 1450)
OUTPUT_SIZE = (625, 1105)
CONTENT_HEIGHT = 1000
CONTENT_TOP = 90
ALPHA_FLOOR = 10

# Calibrated against source/rig_master_profile_right.png.  Regions overlap the
# body around their pivots; the retained core below hides the overlap after a
# conservative rotation without exposing a transparent crack. The flattened
# master does not contain a complete far arm: that region is the hood cords
# plus one hand, so it deliberately remains part of the torso.
BODY_ARM_CUTOUT = np.array(
    [
        [295, 500], [475, 520], [495, 650], [475, 760], [450, 865],
        [460, 955], [345, 970], [300, 880], [300, 755], [315, 625],
    ],
    dtype=np.int32,
)

# The strict side master contains only one complete sleeve.  The approved arm
# master was generated specifically with two unobstructed limbs and a complete
# jacket underneath them.  Their masks stay independent all the way to runtime:
# far arm -> torso -> near arm is the same depth order used by the walk refs.
ARMS_FAR = np.array(
    [
        [305, 660], [420, 675], [403, 790], [360, 900], [326, 1015],
        [275, 1085], [200, 1058], [215, 965], [242, 875], [255, 760],
    ],
    dtype=np.int32,
)
ARMS_NEAR = np.array(
    [
        [545, 675], [610, 680], [615, 785], [665, 855], [735, 915],
        [795, 966], [785, 1035], [735, 1060], [674, 1018], [623, 950],
        [555, 904], [535, 800],
    ],
    dtype=np.int32,
)
LEG_FAR_THIGH = np.array(
    [
        [370, 930], [550, 930], [550, 1030], [505, 1145], [325, 1145],
        [330, 1040], [350, 970],
    ],
    dtype=np.int32,
)
LEG_FAR_SHIN = np.array(
    [
        [310, 1030], [515, 1030], [515, 1195], [490, 1285], [285, 1285],
        [290, 1190],
    ],
    dtype=np.int32,
)
LEG_FAR_FOOT = np.array(
    [[270, 1200], [520, 1200], [535, 1378], [245, 1378], [245, 1280]],
    dtype=np.int32,
)
LEG_NEAR_THIGH = np.array(
    [[505, 930], [665, 930], [700, 1025], [695, 1145], [515, 1145], [505, 1010]],
    dtype=np.int32,
)
LEG_NEAR_SHIN = np.array(
    [[510, 1030], [705, 1030], [720, 1200], [730, 1290], [535, 1290], [520, 1180]],
    dtype=np.int32,
)
LEG_NEAR_FOOT = np.array(
    [[525, 1200], [735, 1200], [790, 1280], [790, 1378], [530, 1378]],
    dtype=np.int32,
)

# The tail passes in front of the far leg in the flattened master.  Keep it out
# of both leg mattes and render it between the far and near leg at runtime.
TAIL = np.array(
    [
        [100, 945], [265, 900], [405, 920], [450, 985], [410, 1085],
        [315, 1155], [135, 1160], [75, 1080],
    ],
    dtype=np.int32,
)

# Only tiny joint caps are restored after limb subtraction.  The previous
# broad protected polygons accidentally copied the complete static arm and
# upper legs back into the body layer, masking the articulated motion.
BODY_CORES = (
    np.array([[335, 505], [425, 515], [430, 595], [330, 590]], dtype=np.int32),
    np.array([[575, 525], [645, 535], [650, 605], [570, 605]], dtype=np.int32),
)

# A flattened illustration has no pixels for the jacket hidden behind the
# sleeve or for the pelvis hidden behind the thighs. These deliberately narrow
# backing regions are filled once at build time from surrounding jacket/pants
# colours. They cover articulation gaps without preserving a second static
# copy of the moving limb.
BODY_BACKINGS = (
    np.array(
        [
            [380, 535], [475, 535], [515, 640], [515, 835],
            [480, 950], [405, 950], [410, 820], [425, 690],
        ],
        dtype=np.int32,
    ),
    np.array(
        [
            [335, 890], [655, 890], [690, 1010], [610, 1040],
            [520, 1015], [440, 1040], [325, 1010],
        ],
        dtype=np.int32,
    ),
)


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
        raise SystemExit("approved profile foreground was not found")
    component = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    keep = labels == component
    rgba[~keep] = 0
    rgba[..., 3] = np.where(keep, rgba[..., 3], 0)
    return rgba


def approved_arm_rgba(path: Path) -> np.ndarray:
    actual_hash = sha256(path)
    if actual_hash != ARMS_SOURCE_SHA256:
        raise SystemExit(
            f"{path.name} is not the approved arm master: "
            f"{actual_hash} != {ARMS_SOURCE_SHA256}"
        )
    image = cv2.imread(str(path), cv2.IMREAD_COLOR)
    if image is None:
        raise SystemExit(f"cannot read approved arm master: {path}")
    actual_size = (image.shape[1], image.shape[0])
    if actual_size != ARMS_SOURCE_SIZE:
        raise SystemExit(f"arm extraction calibrated for {ARMS_SOURCE_SIZE}, source is {actual_size}")

    mask = np.full(image.shape[:2], cv2.GC_BGD, dtype=np.uint8)
    background = np.zeros((1, 65), dtype=np.float64)
    foreground = np.zeros((1, 65), dtype=np.float64)
    cv2.grabCut(
        image,
        mask,
        ARMS_GRABCUT_RECT,
        background,
        foreground,
        8,
        cv2.GC_INIT_WITH_RECT,
    )
    alpha = np.where(
        (mask == cv2.GC_FGD) | (mask == cv2.GC_PR_FGD), 255, 0
    ).astype(np.uint8)
    count, labels, stats, _ = cv2.connectedComponentsWithStats(alpha, 8)
    if count <= 1:
        raise SystemExit("approved arm foreground was not found")
    component = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    alpha = np.where(labels == component, 255, 0).astype(np.uint8)
    alpha = cv2.morphologyEx(alpha, cv2.MORPH_CLOSE, np.ones((3, 3), np.uint8))
    alpha = cv2.GaussianBlur(alpha, (0, 0), 0.7)
    rgb = cv2.cvtColor(image, cv2.COLOR_BGR2RGB)
    return np.dstack((rgb, alpha))


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
    target = output[origin_y : origin_y + CONTENT_HEIGHT, origin_x : origin_x + target_width]
    target[..., :3] = colour
    target[..., 3] = matte
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


def shift_layer(layer: np.ndarray, dx: float, dy: float) -> np.ndarray:
    matrix = np.array([[1.0, 0.0, dx], [0.0, 1.0, dy]], dtype=np.float32)
    return cv2.warpAffine(
        layer,
        matrix,
        OUTPUT_SIZE,
        flags=cv2.INTER_LANCZOS4,
        borderMode=cv2.BORDER_CONSTANT,
        borderValue=(0, 0, 0, 0),
    )


def main() -> None:
    base = Path(__file__).parent
    source_path = base / "source" / SOURCE_NAME
    rgba = approved_rgba(source_path)
    alpha = rgba[..., 3]
    bounds = cv2.boundingRect((alpha > ALPHA_FLOOR).astype(np.uint8))
    rgb = rgba[..., :3]

    arm_rgba = approved_arm_rgba(base / "source" / ARMS_SOURCE_NAME)
    arm_alpha = arm_rgba[..., 3]
    arm_bounds = cv2.boundingRect((arm_alpha > ALPHA_FLOOR).astype(np.uint8))
    arm_rgb = arm_rgba[..., :3]
    arm_far_source = selection(arm_alpha, ARMS_FAR)
    arm_near_source = selection(arm_alpha, ARMS_NEAR)

    body_arm_cutout = selection(alpha, BODY_ARM_CUTOUT)
    leg_far_thigh = selection(alpha, LEG_FAR_THIGH)
    leg_far_shin = selection(alpha, LEG_FAR_SHIN)
    leg_far_foot = selection(alpha, LEG_FAR_FOOT)
    leg_near_thigh = selection(alpha, LEG_NEAR_THIGH)
    leg_near_shin = selection(alpha, LEG_NEAR_SHIN)
    leg_near_foot = selection(alpha, LEG_NEAR_FOOT)
    leg_far = cv2.bitwise_or(cv2.bitwise_or(leg_far_thigh, leg_far_shin), leg_far_foot)
    leg_near = cv2.bitwise_or(cv2.bitwise_or(leg_near_thigh, leg_near_shin), leg_near_foot)
    tail = selection(alpha, TAIL)
    limb_occluders = cv2.bitwise_or(body_arm_cutout, tail)
    leg_far = cv2.bitwise_and(leg_far, cv2.bitwise_not(limb_occluders))
    leg_near = cv2.bitwise_and(leg_near, cv2.bitwise_not(body_arm_cutout))
    movable = np.zeros_like(alpha)
    for matte in (
        body_arm_cutout,
        leg_far,
        leg_near,
        tail,
    ):
        movable = cv2.bitwise_or(movable, matte)
    protected = np.zeros_like(alpha)
    for core in BODY_CORES:
        cv2.fillPoly(protected, [core], 255)
    removable = cv2.bitwise_and(movable, cv2.bitwise_not(protected))
    body = cv2.bitwise_and(alpha, cv2.bitwise_not(removable))

    backing = np.zeros_like(alpha)
    for region in BODY_BACKINGS:
        cv2.fillPoly(backing, [region], 255)
    backing = cv2.bitwise_and(backing, alpha)
    backing_holes = cv2.bitwise_and(removable, backing)
    body_rgb = cv2.inpaint(rgb, backing_holes, 5.0, cv2.INPAINT_TELEA)
    body = cv2.bitwise_or(body, backing)

    layers = {
        "body": (body_rgb, body),
        "tail": (rgb, tail),
        "leg_far": (rgb, leg_far),
        "leg_near": (rgb, leg_near),
    }
    transformed: dict[str, np.ndarray] = {}
    scale = 1.0
    origin_x = origin_y = 0
    for name, (layer_rgb, matte) in layers.items():
        layer, scale, origin_x, origin_y = runtime_layer(layer_rgb, matte, bounds)
        transformed[name] = layer
        Image.fromarray(layer, "RGBA").save(base / f"profile_motion_right_{name}.png")
        Image.fromarray(np.flip(layer, axis=1).copy(), "RGBA").save(
            base / f"profile_motion_left_{name}.png"
        )

    mapped = lambda point: map_point(point, bounds, scale, origin_x, origin_y)
    visor_top_left = mapped((515.0, 300.0))
    visor_bottom_right = mapped((730.0, 605.0))
    arm_pivots = {
        "arm_far": mapped((365.0, 555.0)),
        "arm_near": mapped((620.0, 555.0)),
    }
    donor_pivots = {
        "arm_far": (330.0, 700.0),
        "arm_near": (540.0, 700.0),
    }
    for name, matte in (("arm_far", arm_far_source), ("arm_near", arm_near_source)):
        layer, donor_scale, donor_origin_x, donor_origin_y = runtime_layer(
            arm_rgb, matte, arm_bounds
        )
        donor_pivot = map_point(
            donor_pivots[name], arm_bounds, donor_scale, donor_origin_x, donor_origin_y
        )
        target_pivot = arm_pivots[name]
        layer = shift_layer(
            layer,
            target_pivot[0] - donor_pivot[0],
            target_pivot[1] - donor_pivot[1],
        )
        transformed[name] = layer
        Image.fromarray(layer, "RGBA").save(base / f"profile_motion_right_{name}.png")
        Image.fromarray(np.flip(layer, axis=1).copy(), "RGBA").save(
            base / f"profile_motion_left_{name}.png"
        )
    tail_pivot = mapped((410.0, 970.0))
    leg_joints = {
        "far": {
            "hip": mapped((430.0, 885.0)),
            "knee": mapped((375.0, 1110.0)),
            "ankle": mapped((385.0, 1265.0)),
        },
        "near": {
            "hip": mapped((575.0, 885.0)),
            "knee": mapped((585.0, 1115.0)),
            "ankle": mapped((610.0, 1265.0)),
        },
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
        "legs": {},
        "leg_chains": {
            "far": {
                "sprite": "profile_motion_right_leg_far.png",
                **{name: [round(point[0], 2), round(point[1], 2)] for name, point in leg_joints["far"].items()},
            },
            "near": {
                "sprite": "profile_motion_right_leg_near.png",
                **{name: [round(point[0], 2), round(point[1], 2)] for name, point in leg_joints["near"].items()},
            },
        },
        "arms": {
            "far": limb("profile_motion_right_arm_far.png", arm_pivots["arm_far"]),
            "near": limb("profile_motion_right_arm_near.png", arm_pivots["arm_near"]),
        },
        "tail": limb("profile_motion_right_tail.png", tail_pivot),
        "pockets": {},
        "ground": 1090,
    }
    (base / "profile_motion_right.json").write_text(
        json.dumps(manifest, indent=2), encoding="utf-8"
    )

    left = json.loads(json.dumps(manifest))
    left["sprite"] = "profile_motion_left_body.png"
    left["visor"]["x"] = OUTPUT_SIZE[0] - manifest["visor"]["x"] - manifest["visor"]["w"]
    for name in ("far", "near"):
        right_name = left["arms"][name]["sprite"]
        left["arms"][name]["sprite"] = right_name.replace("_right_", "_left_")
        left["arms"][name]["pivot_x"] = round(
            OUTPUT_SIZE[0] - manifest["arms"][name]["pivot_x"], 2
        )
    left["tail"]["sprite"] = left["tail"]["sprite"].replace("_right_", "_left_")
    left["tail"]["pivot_x"] = round(
        OUTPUT_SIZE[0] - manifest["tail"]["pivot_x"], 2
    )
    for name in ("far", "near"):
        left["leg_chains"][name]["sprite"] = left["leg_chains"][name]["sprite"].replace(
            "_right_", "_left_"
        )
        for joint in ("hip", "knee", "ankle"):
            left["leg_chains"][name][joint][0] = round(
                OUTPUT_SIZE[0] - manifest["leg_chains"][name][joint][0], 2
            )
    (base / "profile_motion_left.json").write_text(
        json.dumps(left, indent=2), encoding="utf-8"
    )
    print(
        f"profile master: sha256={SOURCE_SHA256} bounds={bounds} "
        f"scale={scale:.4f} arms={arm_pivots} joints={leg_joints}"
    )


if __name__ == "__main__":
    main()
