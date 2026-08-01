# Промпт для профильной ходьбы

Нужно создать **две отдельные картинки** того же персонажа, что находится в
`source/stand.png`:

1. `source/stand_three_quarter_right.png` — промежуточный разворот на 45°;
2. `source/stand_profile_right.png` — строгий профиль вправо, пригодный для
   разделения корпуса и ног на независимые слои.

Не создавайте sprite sheet и не объединяйте две позы в одну картинку. Для обеих
генераций обязательно используйте `source/stand.png` как reference image и один
и тот же seed. Профиль влево будет получен зеркальным отражением — генерировать
его отдельно не нужно.

## Общие настройки

- Размер холста: **625 × 1105 px**, вертикальный.
- Персонаж должен занимать ту же высоту и ширину кадра, что и на референсе.
- Нижняя точка обеих стоп должна находиться на той же горизонтальной линии, что
  и на `source/stand.png`.
- Фон: однородный нейтральный тёмно-серый `#202126`, без текстуры и градиента.
- Без тени, пола, частиц, светящегося шарика, текста и дополнительных объектов.
- Визор должен быть пустым, тёмным и глянцевым: глаза и рот будут отрисованы
  процедурно.
- Сохранить точный дизайн персонажа: пропорции головы и тела, форму ушей,
  капюшона, визора, куртки, карманов, хвоста и лап, расположение всех неоновых
  деталей и толщину контуров.

## Картинка 1 — переходная поза ¾ вправо

Скопируйте этот текст как основной prompt:

```text
Using the supplied reference image, reproduce the exact same chibi cyberpunk
fox-cat character with no redesign. Full body standing pose, rotated exactly
45 degrees to the character's right, a clean three-quarter right view used as a
transition between front view and side view.

Preserve the character identity pixel-consistently: identical large pointed
fox ears, identical deep hood, identical near-black glossy oval visor,
identical oversized magenta and violet jacket, identical cyan glowing seams,
zipper, pockets and small square light accents, identical short fluffy tail
with a cyan-lit tip, identical short legs and rounded paws. Preserve the same
body-to-head ratio, silhouette, outline thickness, cel shading, glossy
highlights and neon colour palette as the reference.

The character stands upright and balanced. Both complete legs and both complete
feet are visible and do not touch or overlap. The near leg is visually in front
of the far leg, but a clear strip of background separates their silhouettes so
they can be extracted as independent animation layers. Arms remain relaxed with
hands inside the jacket pockets. The tail is fully visible and does not merge
with the jacket or either leg.

The visor contains no eyes, mouth, symbols or baked facial expression; it is a
clean dark glossy surface. Exact 625 by 1105 pixel vertical canvas. Match the
reference character scale and foot baseline exactly. Character centred. Flat
uniform #202126 background. No floor, no cast shadow, no particles, no floating
orb, no text, no watermark, no scenery.
```

## Картинка 2 — строгий профиль вправо

Скопируйте этот текст как основной prompt:

```text
Using the supplied reference image, reproduce the exact same chibi cyberpunk
fox-cat character with no redesign. Full body strict orthographic side profile
facing right. Head, visor, torso, pelvis and feet are rotated exactly 90 degrees
from the front reference. This must read as a true side view, not a
three-quarter view and not a narrowed front view.

Preserve the character identity pixel-consistently: identical pointed fox ears,
deep hood, near-black glossy visor, oversized magenta and violet hooded jacket,
cyan glowing seams, zipper, pockets, light accents, short fluffy tail with a
cyan-lit tip, short legs and rounded paws. Preserve the same body-to-head ratio,
silhouette language, outline thickness, cel shading, glossy highlights and
neon palette as the reference.

The character stands upright on the same ground baseline. Show one near leg and
one far leg as two complete separate limbs. Offset the far leg slightly backward
and the near leg slightly forward while keeping the torso in a strict 90-degree
side profile. A narrow but unbroken strip of background must remain between the
two leg silhouettes from upper leg to feet. Both feet are fully visible, flat
on the same ground line, and neither foot is cropped. Do not fuse the legs into
one shape. The visible arm stays relaxed with the hand inside the jacket pocket.
The tail is completely visible behind the torso, separated from the jacket and
legs by clear negative space.

The visor is shown from the side and contains no eyes, mouth, symbols or baked
facial expression; it is a clean dark glossy surface. Exact 625 by 1105 pixel
vertical canvas. Match the reference character height, scale and foot baseline
exactly. Character centred. Flat uniform #202126 background. No perspective,
no floor, no cast shadow, no particles, no floating orb, no text, no watermark,
no scenery.
```

## Общий negative prompt

```text
front view, almost front view, back view, incorrect three-quarter profile,
perspective distortion, camera tilt, walking pose, running pose, sitting,
crouching, kneeling, floating, leaning, cropped body, cropped feet, fused legs,
overlapping legs, hidden far leg, extra limbs, missing limbs, asymmetric ear
redesign, changed hood, changed jacket, changed neon pattern, changed colours,
different character, realistic fur, photorealism, 3D render, blurry outline,
transparent background, gradient background, floor, cast shadow, particles,
floating orb, scenery, text, signature, logo, watermark, sprite sheet, multiple
characters
```

## Что проверить перед передачей файлов

1. Обе картинки действительно изображают одного персонажа, а не две похожие
   версии.
2. Профиль направлен строго вправо; грудь и визор не развёрнуты к зрителю.
3. Размер холста, рост персонажа и линия стоп совпадают с `source/stand.png`.
4. Обе ноги и обе стопы можно вырезать независимо, между ними виден фон.
5. Хвост не сливается с курткой и ногами.
6. На визоре отсутствует нарисованное лицо.
7. В изображениях нет шарика, частиц, тени, пола, текста или watermark.

Если генератор не выдержал хотя бы пункты 1–5, лучше перегенерировать картинку,
а не исправлять её изменением пропорций: любое расхождение станет заметным при
переходе `фас → ¾ → профиль`.
