"""Build the approved front rig master without redrawing the character.

The character pixels come from ``source/image.png``.  The only donor pixels are
the blank glass inside the visor, stored as a tightly cropped source plate in
``source/front_visor_donor.png``.  The original source files are never modified.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

import cv2
import numpy as np


ASSETS = Path(__file__).resolve().parent
SOURCE = ASSETS / "source" / "image.png"
VISOR_DONOR = ASSETS / "source" / "front_visor_donor.png"
OUTPUT = ASSETS / "source" / "rig_master_front.png"

EXPECTED_SHA256 = {
    SOURCE: "8a12514cb17da599050ccc81033005bb58e1ebd43114826c10b0ac9af8d96d13",
    VISOR_DONOR: "b11ef0ebc5dc11c01135371b18705905e663f3f517fefbc5eb7f1c89d5ad1f6c",
}
EXPECTED_OUTPUT_SHA256 = "a886e04bb4a95f361e167db7093181333e86df64283c04f3a41fb40457b02796"

# Coordinates are in the fixed 1024 x 1536 source plate.  Every rectangle is
# outside the stable character contour: no clothing, limb, ear or tail texture
# is synthesized or inpainted here.
EXTERIOR_DECORATIONS = (
    (176, 192, 207, 222),
    (788, 193, 818, 222),
    (782, 344, 810, 394),
    (162, 329, 210, 394),
    (192, 405, 221, 435),
    (219, 497, 245, 524),
    (291, 683, 322, 720),
    (226, 779, 260, 817),
    (280, 1122, 298, 1154),
    (859, 974, 889, 1009),
    (904, 987, 934, 1021),
    (910, 1025, 942, 1059),
    (902, 1059, 937, 1093),
)

DONOR_SOURCE_ORIGIN = (260, 310)
VISOR_CENTRE = (510.0, 500.0)
VISOR_RADII = (176.0, 116.0)


def load_rgba(path: Path) -> np.ndarray:
    image = cv2.imread(str(path), cv2.IMREAD_UNCHANGED)
    if image is None or image.ndim != 3 or image.shape[2] != 4:
        raise RuntimeError(f"expected an RGBA PNG: {path}")
    return image


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_source(path: Path) -> None:
    actual = sha256(path)
    expected = EXPECTED_SHA256[path]
    if actual != expected:
        raise RuntimeError(
            f"source drifted: {path.name} has SHA-256 {actual}, expected {expected}"
        )


def retain_main_alpha_component(image: np.ndarray) -> None:
    opaque = (image[:, :, 3] > 10).astype(np.uint8)
    count, labels, stats, _ = cv2.connectedComponentsWithStats(opaque, 8)
    if count < 2:
        raise RuntimeError("front source contains no visible character")
    main = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    image[labels != main] = 0


def composite_blank_visor(image: np.ndarray, donor: np.ndarray) -> None:
    x, y = DONOR_SOURCE_ORIGIN
    right = x + donor.shape[1]
    bottom = y + donor.shape[0]
    if right > image.shape[1] or bottom > image.shape[0]:
        raise RuntimeError("visor donor does not fit the front source")

    donor_plate = np.zeros_like(image)
    donor_plate[y:bottom, x:right] = donor
    aligned = cv2.warpAffine(
        donor_plate,
        np.float32(((1, 0, 0), (0, 1, 10))),
        (image.shape[1], image.shape[0]),
        flags=cv2.INTER_CUBIC,
        borderMode=cv2.BORDER_CONSTANT,
        borderValue=(0, 0, 0, 0),
    )

    yy, xx = np.ogrid[: image.shape[0], : image.shape[1]]
    distance = (
        ((xx - VISOR_CENTRE[0]) / VISOR_RADII[0]) ** 2
        + ((yy - VISOR_CENTRE[1]) / VISOR_RADII[1]) ** 2
    )
    hard_mask = (distance <= 1.0).astype(np.float32)
    feather = cv2.GaussianBlur(hard_mask, (0, 0), 8.0)
    alpha = np.minimum(image[:, :, 3], aligned[:, :, 3]).astype(np.float32)
    blend = feather * alpha / 255.0
    image[:, :, :3] = (
        aligned[:, :, :3].astype(np.float32) * blend[:, :, None]
        + image[:, :, :3].astype(np.float32) * (1.0 - blend[:, :, None])
    ).clip(0, 255).astype(np.uint8)


def clear_exterior_decorations(image: np.ndarray) -> None:
    for left, top, right, bottom in EXTERIOR_DECORATIONS:
        image[top:bottom, left:right] = 0


def main() -> None:
    verify_source(SOURCE)
    verify_source(VISOR_DONOR)
    image = load_rgba(SOURCE)
    donor = load_rgba(VISOR_DONOR)
    if image.shape[:2] != (1536, 1024):
        raise RuntimeError(f"unexpected front source size: {image.shape[1]}x{image.shape[0]}")

    retain_main_alpha_component(image)
    composite_blank_visor(image, donor)
    clear_exterior_decorations(image)

    if not cv2.imwrite(str(OUTPUT), image):
        raise RuntimeError(f"failed to write {OUTPUT}")
    actual_output = sha256(OUTPUT)
    if actual_output != EXPECTED_OUTPUT_SHA256:
        OUTPUT.unlink(missing_ok=True)
        raise RuntimeError(
            f"non-deterministic output SHA-256 {actual_output}, "
            f"expected {EXPECTED_OUTPUT_SHA256}"
        )
    print(OUTPUT)


if __name__ == "__main__":
    main()
