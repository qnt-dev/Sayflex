"""
Разбор исходного арта на отдельные ассеты.

Зверёк, шарик и запечённые квадратики-частицы физически не соприкасаются,
поэтому разделяются по связным компонентам без всякой ручной разметки.
Частицы при этом выбрасываются: в рантайме они живые, а нарисованные ездили
бы жёстко вместе с телом.

Порог альфы 10, а не 0: в исходнике есть одиночные пиксели с альфой 1-4,
из-за которых обрезка по alpha > 0 захватывает весь холст 1024x1536.
"""

from pathlib import Path

import numpy as np
from PIL import Image
from scipy.ndimage import label

ALPHA_FLOOR = 10
# Всё мельче этой доли самой крупной компоненты считается частицей.
PARTICLE_FRACTION = 0.02


def main() -> None:
    base = Path(__file__).parent
    image = Image.open(base / "image.png").convert("RGBA")
    rgba = np.asarray(image)

    tagged, count = label(rgba[..., 3] > ALPHA_FLOOR)
    sizes = np.bincount(tagged.ravel())
    sizes[0] = 0
    order = np.argsort(sizes)[::-1]
    largest = sizes[order[0]]

    creature, orb = order[0], order[1]
    particles = [i for i in order[2:] if 0 < sizes[i] < largest * PARTICLE_FRACTION]

    for name, component in (("creature", creature), ("orb", orb)):
        mask = tagged == component
        ys, xs = np.nonzero(mask)
        x0, x1 = int(xs.min()), int(xs.max()) + 1
        y0, y1 = int(ys.min()), int(ys.max()) + 1

        cut = rgba[y0:y1, x0:x1].copy()
        cut[..., 3] = np.where(mask[y0:y1, x0:x1], cut[..., 3], 0)
        Image.fromarray(cut, "RGBA").save(base / f"{name}.png")
        print(f"{name}.png: {x1 - x0}x{y1 - y0}, {int(sizes[component])} непрозрачных пикселей")

    print(f"отброшено запечённых частиц: {len(particles)}")


if __name__ == "__main__":
    main()
