# Промпт для стоящей позы

Нужен **тот же персонаж**, что на `source/pet-b.png`, но стоящий в полный рост.
Прогоните через тот же сервис, которым делали текущие арты, и положите файл
сюда как `creature_standing.png`.

## Главное

Если сервис умеет работать с референсом — **обязательно приложите
`source/pet-b.png` как reference image**. Независимая генерация по одному
тексту почти наверняка изменит форму ушей, число неоновых вставок, рисунок
капюшона и пропорции, и персонаж перестанет быть тем же.

## Текст промпта

```
Full-body standing pose of a small chibi cyberpunk fox-cat creature wearing an
oversized hooded jacket, front view, symmetrical, standing upright on two short
legs, feet flat on the ground, legs slightly apart, arms relaxed at the sides.

Same character as reference: large pointed fox ears with glowing cyan inserts
and small tech panels, deep hood pulled over the head, dark near-black glossy
oval visor instead of a face, magenta and violet jacket with glowing cyan seams,
zipper and small square light accents, short fluffy tail with a cyan gradient
tip, rounded paws with pink pads.

Neon cyberpunk illustration, saturated violet and magenta palette with cyan
accents, clean bold outlines, cel shading, glossy highlights.

Plain dark grey background, no floor, no scenery, no text.
Vertical composition, character centred, full body visible with a small margin.
```

## Негативный промпт

```
sitting, crouching, kneeling, floating, back view, side profile, cropped legs,
cut off feet, multiple characters, text, watermark, busy background, floor,
shadow on background
```

## Что критично для дальнейшей обработки

1. **Ноги видны целиком, ступни не обрезаны.** Из подобранных лап цикл ходьбы
   не построить — именно из-за этого текущий арт не ходит.
2. **Ноги слегка расставлены.** Если они сомкнуты, я не смогу разделить их на
   отдельные слои.
3. **Визор пустой или почти пустой.** Лицо всё равно рисуется процедурно, и
   запечённое я буду стирать. Чем меньше на визоре нарисовано, тем чище выйдет.
4. **Фон ровный тёмно-серый**, как в текущих исходниках. Скрипт разбора
   рассчитан именно на такой фон.
5. **Тот же масштаб деталей.** Персонаж должен занимать кадр примерно так же,
   как на `pet-b.png`, иначе толщина контуров не совпадёт с сидящей позой.

## Что я сделаю дальше сам

- вырежу фон и разложу на слои по связным компонентам;
- отделю бедро, голень и ступню каждой ноги;
- дорисую то, что скрыто за ногами, решением уравнения Лапласа;
- сниму рамку визора и точки привязки;
- соберу цикл ходьбы с переносом веса, покачиванием корпуса и тенью,
  реагирующей на опорную ногу.

От вас — только файл.
