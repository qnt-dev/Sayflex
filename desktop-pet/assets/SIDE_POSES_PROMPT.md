# Промпт для поворота: вид в 3/4 и профиль

Нужны **две** картинки того же персонажа, что на `source/stand.png`, в тех же
пропорциях и в той же стойке, но развёрнутого. С ними появится настоящий
поворот: фас → 3/4 → профиль, и обратно; для левой стороны я зеркалю профиль и
3/4, поэтому генерировать нужно только разворот **вправо**.

Кладите файлы сюда как `source/stand_three_quarter.png` и
`source/stand_profile.png`.

## Главное

**Обязательно приложите `source/stand.png` как reference image.** Без референса
сервис почти наверняка изменит форму ушей, число неоновых вставок и пропорции
куртки, и при повороте персонаж будет «дёргаться» — на каждом кадре чуть другое
существо. Это заметно сильнее, чем любая неточность позы.

Если сервис умеет фиксировать seed — используйте один и тот же для обеих
картинок.

## Картинка 1 — вид в 3/4 (разворот вправо)

```
Same chibi cyberpunk fox-cat creature as the reference image, full body standing
pose, turned three-quarters to the right, head and body rotated about 45 degrees
away from the viewer, standing upright on two short legs, feet flat on the
ground, legs slightly apart with a visible gap between them, arms relaxed at the
sides with hands in the jacket pockets.

Identical character to the reference: large pointed fox ears with glowing cyan
inserts and small tech panels, deep hood pulled over the head, dark near-black
glossy oval visor instead of a face, magenta and violet oversized hooded jacket
with glowing cyan seams and small square light accents, short fluffy tail with a
cyan gradient tip, rounded paws.

Neon cyberpunk illustration, saturated violet and magenta palette with cyan
accents, clean bold outlines, cel shading, glossy highlights. Same line weight
and same level of detail as the reference.

Plain dark grey background, no floor, no scenery, no text, no shadow.
Vertical composition, character centred, full body visible with a small margin,
character occupies the same fraction of the frame as in the reference.
```

## Картинка 2 — строгий профиль (разворот вправо)

```
Same chibi cyberpunk fox-cat creature as the reference image, full body standing
pose, strict side profile facing right, body rotated 90 degrees, only one side of
the character visible, standing upright on two short legs, feet flat on the
ground, near leg and far leg both visible with a gap between them, arm relaxed at
the side with hand in the jacket pocket.

Identical character to the reference: large pointed fox ears with glowing cyan
inserts, deep hood pulled over the head, dark near-black glossy visor seen from
the side, magenta and violet oversized hooded jacket with glowing cyan seams,
short fluffy tail with a cyan gradient tip clearly visible behind the body,
rounded paws.

Neon cyberpunk illustration, saturated violet and magenta palette with cyan
accents, clean bold outlines, cel shading, glossy highlights. Same line weight
and same level of detail as the reference.

Plain dark grey background, no floor, no scenery, no text, no shadow.
Vertical composition, character centred, full body visible with a small margin,
character occupies the same fraction of the frame as in the reference.
```

## Негативный промпт (для обеих)

```
front view, back view, sitting, crouching, kneeling, floating, cropped legs,
cut off feet, closed legs, legs touching, hidden tail, multiple characters,
text, watermark, busy background, floor, shadow on background, different
character design, changed ear shape
```

## Что критично для разбора

1. **Ступни целиком в кадре, ноги расставлены.** Ноги я режу по просвету между
   ними; если они сомкнуты, отдельных слоёв не выйдет и цикл ходьбы не
   построится. Это ровно та причина, по которой не ходила сидячая поза.
2. **Хвост виден.** В профиле он должен читаться за корпусом, а не сливаться с
   ним — иначе при повороте он будет исчезать.
3. **Рост и положение ступней те же, что в `stand.png`.** Персонаж не должен
   «подпрыгивать» при смене ракурса. Если рост разойдётся на десяток пикселей,
   я подгоню при разборе, но чем ближе, тем чище.
4. **Визор пустой или почти пустой.** Лицо рисуется процедурно, запечённое я
   стираю; чем меньше на визоре нарисовано, тем меньше следов останется.
5. **Фон ровный тёмно-серый**, без пола и без тени — скрипт разбора рассчитан
   именно на такой.

## Что дальше делаю я

- разложу обе позы теми же скриптами, что и фас: вырежу фон, отделю ноги по
  просвету, сниму рамку визора и якоря частиц, соберу манифест;
- привяжу три ракурса к направлению движения: стоит — фас, поворачивает —
  3/4, идёт или бежит вбок — профиль; влево всё то же зеркально;
- сделаю переход между ракурсами по времени, а не мгновенной подменой, чтобы
  разворот читался как разворот;
- цикл ходьбы и бега соберу для каждого ракурса отдельно — в профиле ноги
  ходят с большей амплитудой, чем в фасе.

От вас — только два файла.
