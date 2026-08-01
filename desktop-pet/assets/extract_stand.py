"""
Разбор стоящей позы на части для цикла ходьбы.

Ноги в этом арте разделены видимым просветом, поэтому их можно вырезать
автоматически по этому просвету и дальше вращать вокруг бедра. Корпус при этом
остаётся целым, а места, откуда ноги ушли, затягиваются решением уравнения
Лапласа — иначе при взмахе ноги сквозь неё была бы видна дыра.

Рамка визора и линия отреза бёдер откалиброваны по координатной сетке: это
разовая привязка к конкретному арту, автоматически такие границы не находятся.

На выходе:
  stand_body.png       корпус, ноги затянуты, лицо снято
  stand_leg_left.png   левая нога с точкой вращения
  stand_leg_right.png  правая нога
  stand.json           геометрия: визор, якоря частиц, бёдра, линия земли
"""

import json
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.ndimage import binary_dilation, binary_opening

ALPHA_FLOOR = 10
GLOW_MARGIN = 14
RELAX_STEPS = 400

EXPECTED_SIZE = (625, 1105)

# Всё снято по координатной сетке _grid_stand.png.
VISOR = (130, 232, 300, 178)
ANCHORS = {"left_eye": (0.283, 0.410), "right_eye": (0.740, 0.494), "mouth": (0.510, 0.764)}

# Откуда исходят частицы: уши и хвост.
PARTICLE_ANCHORS = {
    "left_ear": (95, 130, 74),
    "right_ear": (455, 130, 74),
    "tail": (555, 835, 88),
}

# Линия отреза бёдер и вертикальная граница между ногами.
HIP_LINE = 815
LEG_SPLIT_X = 268
# Правее этой границы начинается хвост — он тоже ниже линии бёдер, но ногой
# не является и вращаться вокруг бедра не должен.
LEG_RIGHT_LIMIT = 480
# Насколько подол куртки перекрывает срез бедра. Без нахлёста при взмахе ноги
# между корпусом и бедром появлялась бы щель.
BODY_OVERLAP = 42
# Точки вращения ног — примерно центр бедра.
HIPS = {"left": (168, 828), "right": (372, 828)}
GROUND = 1090

# Манжеты карманов: отсюда выходят голографические кисти. Сняты по той же
# координатной сетке, что и визор. Долями размера спрайта их задавать нельзя —
# загрузчик обрезает пустые поля, и доли поехали бы вместе с обрезкой.
POCKETS = {"left": (123, 678), "right": (424, 678)}


def inpaint_laplace(channel: np.ndarray, hole: np.ndarray, steps: int) -> np.ndarray:
    result = channel.astype(np.float32).copy()
    border = binary_dilation(hole, np.ones((3, 3))) & ~hole
    if border.any():
        result[hole] = float(result[border].mean())
    for _ in range(steps):
        neighbours = np.zeros_like(result)
        neighbours[1:, :] += result[:-1, :]
        neighbours[:-1, :] += result[1:, :]
        neighbours[:, 1:] += result[:, :-1]
        neighbours[:, :-1] += result[:, 1:]
        counts = np.zeros_like(result)
        counts[1:, :] += 1.0
        counts[:-1, :] += 1.0
        counts[:, 1:] += 1.0
        counts[:, :-1] += 1.0
        result[hole] = (neighbours / counts)[hole]
    return np.clip(result, 0.0, 255.0)


def erase_face(rgba: np.ndarray) -> np.ndarray:
    x, y, w, h = VISOR
    inside = np.zeros(rgba.shape[:2], dtype=bool)
    inside[y : y + h, x : x + w] = True
    opaque = rgba[..., 3] > ALPHA_FLOOR
    value = rgba[..., :3].astype(np.int16).max(axis=2)
    glass = float(np.median(value[inside & opaque]))
    lit = inside & opaque & (value > glass + GLOW_MARGIN)
    face = binary_dilation(binary_opening(lit, np.ones((3, 3))), np.ones((3, 3)), iterations=3)
    face &= inside & opaque

    out = rgba.copy()
    for channel in range(3):
        out[..., channel] = inpaint_laplace(rgba[..., channel], face, RELAX_STEPS).astype(np.uint8)
    print(f"снято пикселей лица: {int(face.sum())}")
    return out


def main() -> None:
    base = Path(__file__).parent
    image = Image.open(base / "standing.png").convert("RGBA")
    if image.size != EXPECTED_SIZE:
        raise SystemExit(f"калибровка под {EXPECTED_SIZE}, а спрайт {image.size}")
    rgba = erase_face(np.asarray(image))
    opaque = rgba[..., 3] > ALPHA_FLOOR

    # Ноги: всё ниже линии бёдер, разделённое просветом по вертикали.
    below = np.zeros(rgba.shape[:2], dtype=bool)
    below[HIP_LINE:, :] = True
    legs = below & opaque
    legs[:, LEG_RIGHT_LIMIT:] = False

    for side, limit in (
        ("left", slice(None, LEG_SPLIT_X)),
        ("right", slice(LEG_SPLIT_X, LEG_RIGHT_LIMIT)),
    ):
        mask = np.zeros_like(legs)
        mask[:, limit] = legs[:, limit]
        ys, xs = np.nonzero(mask)
        if len(xs) == 0:
            raise SystemExit(f"нога {side} не найдена")
        x0, x1 = int(xs.min()), int(xs.max()) + 1
        y0, y1 = int(ys.min()), int(ys.max()) + 1
        cut = rgba[y0:y1, x0:x1].copy()
        cut[..., 3] = np.where(mask[y0:y1, x0:x1], cut[..., 3], 0)
        Image.fromarray(cut, "RGBA").save(base / f"stand_leg_{side}.png")
        HIPS[side] = (HIPS[side][0], HIPS[side][1], x0, y0)
        print(f"нога {side}: {x1 - x0}x{y1 - y0} в ({x0}, {y0})")

    # Корпус: ноги просто отрезаются, без инпейнта.
    #
    # Раньше я затягивал их место Лапласом и гасил альфу вниз — получалось
    # серое размытое пятно поверх настоящих ног, они выглядели выцветшими.
    # Инпейнт здесь не нужен: низ куртки свисает ниже линии бёдер и сам
    # закрывает стык при взмахе ноги.
    #
    # Хвост висит ниже той же линии, но ногой не является — его сохраняем.
    body = rgba.copy()
    cut = np.zeros(rgba.shape[:2], dtype=bool)
    cut[HIP_LINE + BODY_OVERLAP :, :LEG_RIGHT_LIMIT] = True
    body[cut, 3] = 0
    Image.fromarray(body, "RGBA").save(base / "stand_body.png")
    print(f"низ корпуса отрезан ниже {HIP_LINE + BODY_OVERLAP}, хвост сохранён")

    (base / "stand.json").write_text(
        json.dumps(
            {
                "sprite": "stand_body.png",
                "source_size": list(EXPECTED_SIZE),
                "visor": {"x": VISOR[0], "y": VISOR[1], "w": VISOR[2], "h": VISOR[3]},
                "anchors": ANCHORS,
                "particle_anchors": {
                    name: {"x": x, "y": y, "r": r} for name, (x, y, r) in PARTICLE_ANCHORS.items()
                },
                "legs": {
                    side: {
                        "sprite": f"stand_leg_{side}.png",
                        "pivot_x": hip[0],
                        "pivot_y": hip[1],
                        "origin_x": hip[2],
                        "origin_y": hip[3],
                    }
                    for side, hip in HIPS.items()
                },
                "pockets": {
                    side: {"x": point[0], "y": point[1]} for side, point in POCKETS.items()
                },
                "ground": GROUND,
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    print(f"визор: {VISOR[2]}x{VISOR[3]} в ({VISOR[0]}, {VISOR[1]})")


if __name__ == "__main__":
    main()
