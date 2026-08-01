//! Голографические кисти.
//!
//! В арте руки персонажа лежат в карманах, кистей нет ни в одном пикселе —
//! показать «класс», «фак» или хлопок нечем. Дорисовывать их спрайтами значило
//! бы генерировать новую картинку под каждый жест.
//!
//! Поэтому кисть рисуется процедурно, тем же неоном, что и лицо: она
//! проявляется из кармана как голограмма и складывается в любой жест сразу.
//! Вся форма задаётся пятью числами — насколько выпрямлен каждый палец, — и
//! между жестами можно плавно перетекать, потому что это просто интерполяция
//! пяти чисел.

use tiny_skia::{LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// Насколько выпрямлен каждый палец: большой, указательный, средний,
/// безымянный, мизинец. 0 — прижат к ладони, 1 — вытянут.
pub type Fingers = [f32; 5];

/// Готовые жесты. Между любыми двумя можно перетечь [`HandPose::blend_to`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Кисть спрятана в карман.
    Hidden,
    /// Раскрытая ладонь — приветствие и хлопок.
    Open,
    /// Кулак.
    Fist,
    /// Большой палец вверх.
    ThumbsUp,
    /// Средний палец.
    MiddleFinger,
    /// Указательный — тычок в сторону курсора.
    Point,
    /// Знак мира: указательный и средний.
    Peace,
}

impl Gesture {
    /// Раскладка пальцев жеста.
    #[must_use]
    pub const fn fingers(self) -> Fingers {
        match self {
            Self::Hidden | Self::Fist => [0.0, 0.0, 0.0, 0.0, 0.0],
            Self::Open => [1.0, 1.0, 1.0, 1.0, 1.0],
            Self::ThumbsUp => [1.0, 0.0, 0.0, 0.0, 0.0],
            Self::MiddleFinger => [0.0, 0.0, 1.0, 0.0, 0.0],
            Self::Point => [0.0, 1.0, 0.0, 0.0, 0.0],
            Self::Peace => [0.0, 1.0, 1.0, 0.0, 0.0],
        }
    }

    /// Поворот кисти под жест, радианы. «Класс» показывают вертикально,
    /// остальное — как держат руку.
    #[must_use]
    pub const fn angle(self) -> f32 {
        match self {
            Self::ThumbsUp => -0.30,
            Self::MiddleFinger => 0.0,
            _ => 0.12,
        }
    }

    /// Виден ли жест вообще. Спрятанную кисть рисовать не нужно.
    #[must_use]
    pub const fn visible(self) -> bool {
        !matches!(self, Self::Hidden)
    }
}

/// Текущее состояние кисти: куда идёт и насколько дошла.
#[derive(Clone, Copy, Debug)]
pub struct HandPose {
    fingers: Fingers,
    angle: f32,
    /// Проявленность голограммы: 0 — кисти нет, 1 — полностью материализована.
    materialise: f32,
    target: Gesture,
}

impl Default for HandPose {
    fn default() -> Self {
        Self::new()
    }
}

impl HandPose {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fingers: [0.0; 5],
            angle: 0.0,
            materialise: 0.0,
            target: Gesture::Hidden,
        }
    }

    /// Задаёт, к какому жесту стремиться. Мгновенной подмены не происходит:
    /// пальцы разгибаются, а сама кисть проявляется или тает.
    pub fn blend_to(&mut self, gesture: Gesture) {
        self.target = gesture;
    }

    #[must_use]
    pub const fn gesture(&self) -> Gesture {
        self.target
    }

    /// Насколько кисть сейчас видна. По этому же числу решается, тратить ли
    /// кадр на её отрисовку.
    #[must_use]
    pub const fn visibility(&self) -> f32 {
        self.materialise
    }

    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.materialise < 0.01
    }

    pub fn update(&mut self, dt: f32) {
        // Пальцы складываются быстрее, чем кисть проявляется: жест успевает
        // сформироваться до того, как его станет хорошо видно.
        let goal = self.target.fingers();
        let rate = (dt * 12.0).min(1.0);
        for (current, wanted) in self.fingers.iter_mut().zip(goal) {
            *current += (wanted - *current) * rate;
        }
        self.angle += (self.target.angle() - self.angle) * (dt * 8.0).min(1.0);

        let wanted = if self.target.visible() { 1.0 } else { 0.0 };
        self.materialise += (wanted - self.materialise) * (dt * 7.0).min(1.0);
        if self.materialise < 0.002 {
            self.materialise = 0.0;
        }
    }
}

/// Цвет голограммы. Отдельным типом, чтобы кисти светились в тон облаку
/// частиц, а не жили своей палитрой.
#[derive(Clone, Copy, Debug)]
pub struct Glow {
    pub core: (u8, u8, u8),
    pub halo: (u8, u8, u8),
}

impl Default for Glow {
    fn default() -> Self {
        Self {
            core: (216, 252, 255),
            halo: (58, 214, 255),
        }
    }
}

// Геометрия кисти в долях холста.
//
// Ладонь — сплошная масса, пальцы выезжают из-под неё вверх. Порядок важен:
// ладонь рисуется поверх пальцев, поэтому прижатый палец просто прячется за
// ней, и кулак получается сам собой. Первый вариант, где ладонь была таким же
// штрихом, как пальцы, читался как пучок палок — жест «класс» в нём было не
// узнать.

/// Пропорция холста: высота к ширине. Запас сверху — под вытянутые пальцы.
const ASPECT: f32 = 1.25;
/// Ладонь: отрезок по вертикали в долях высоты холста и толщина в долях
/// ширины. Пальцы должны быть заметно длиннее ладони, иначе она их поглощает —
/// на первой сборке ладонь была вдвое толще и от жестов оставалось пятно.
const PALM_TOP: f32 = 0.60;
const PALM_BOTTOM: f32 = 0.72;
const PALM_WIDTH: f32 = 0.40;
/// Где на холсте находится запястье, в долях высоты. По этой точке кисть
/// приставляется к карману — иначе при смене жеста она бы «плавала».
pub const WRIST_Y: f32 = PALM_BOTTOM + PALM_WIDTH / 2.0 / ASPECT;
/// Длина пальцев в долях высоты холста: указательный, средний, безымянный,
/// мизинец.
const FINGER_LENGTH: [f32; 4] = [0.36, 0.44, 0.36, 0.28];
/// Большой палец длиннее среднего: он единственный, чем читается «класс», и
/// при равной длине тонул в ореоле ладони.
const THUMB_LENGTH: f32 = 0.46;
const FINGER_WIDTH: f32 = 0.11;
/// Насколько палец веером расходится в стороны на раскрытой ладони.
const FAN: f32 = 0.26;

/// Рисует кисть. Возвращает `None`, если рисовать нечего.
///
/// `size` — ширина холста в пикселях; высота берётся с запасом под пальцы.
/// `mirrored` — левая кисть: та же геометрия, отражённая по горизонтали.
#[must_use]
pub fn render(pose: &HandPose, size: u32, mirrored: bool, glow: Glow) -> Option<Pixmap> {
    if pose.is_hidden() || size == 0 {
        return None;
    }
    let width = size.max(8);
    let height = (width as f32 * ASPECT) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let (w, h) = (width as f32, height as f32);

    let centre = 0.5 * w;
    let palm_top = PALM_TOP * h;
    let palm_bottom = PALM_BOTTOM * h;
    let palm_half = PALM_WIDTH * w / 2.0;

    // Проявление снизу вверх: сначала ладонь, потом пальцы. Так голограмма
    // читается как выезжающая из кармана, а не как включённая лампочка.
    let grow = pose.materialise.clamp(0.0, 1.0);
    let alpha = grow * grow;

    // Пальцы: прямые штрихи вверх от верхнего края ладони. Прижатый палец
    // имеет нулевую длину и целиком уходит под ладонь.
    let mut fingers = PathBuilder::new();
    for (index, length) in FINGER_LENGTH.iter().enumerate() {
        let extension = pose.fingers[index + 1];
        if extension < 0.02 {
            continue;
        }
        let spread = (index as f32 - 1.5) / 1.5;
        let base = (centre + spread * palm_half * 0.62, palm_top);
        let reach = length * h * extension * grow;
        fingers.move_to(base.0, base.1);
        fingers.line_to(base.0 + spread * reach * FAN, base.1 - reach);
    }

    // Большой палец уходит вверх и наружу от ребра ладони — именно этот угол
    // делает «класс» узнаваемым с одного взгляда.
    let thumb_extension = pose.fingers[0];
    if thumb_extension >= 0.02 {
        let base = (
            centre - palm_half * 0.85,
            palm_top + (palm_bottom - palm_top) * 0.25,
        );
        let reach = THUMB_LENGTH * h * thumb_extension * grow;
        fingers.move_to(base.0, base.1);
        fingers.line_to(base.0 - reach * 0.28, base.1 - reach * 0.96);
    }

    // Ладонь: короткий очень толстый штрих от запястья к основанию пальцев.
    // Скруглённые концы дают форму без отдельного контура.
    let mut wrist = PathBuilder::new();
    wrist.move_to(centre, palm_bottom);
    wrist.line_to(centre, palm_bottom - (palm_bottom - palm_top) * grow);

    let pivot = (centre, palm_bottom + palm_half);
    let mut transform = Transform::from_rotate_at(
        pose.angle.to_degrees() * if mirrored { -1.0 } else { 1.0 },
        pivot.0,
        pivot.1,
    );
    if mirrored {
        // Отражение вокруг вертикальной оси холста.
        transform = Transform::from_row(-1.0, 0.0, 0.0, 1.0, w, 0.0).pre_concat(transform);
    }

    // Три прохода на слой: широкий тусклый ореол, средний и узкое яркое ядро.
    // Так же сделано свечение лица, и кисть садится в общий стиль без подгонки.
    // У ладони ореол уже: при общих множителях он расплывался на всю кисть и
    // от жеста оставалось светящееся пятно.
    let mut glowing = |path: &tiny_skia::Path, thickness: f32, spread: f32| {
        for (colour, factor, opacity) in [
            (glow.halo, spread, 0.26),
            (glow.halo, 1.0 + (spread - 1.0) * 0.35, 0.50),
            (glow.core, 0.58, 0.96),
        ] {
            let mut paint = Paint {
                anti_alias: true,
                ..Paint::default()
            };
            paint.set_color_rgba8(
                colour.0,
                colour.1,
                colour.2,
                (alpha * opacity * 255.0) as u8,
            );
            let stroke = Stroke {
                width: thickness * factor,
                line_cap: LineCap::Round,
                ..Stroke::default()
            };
            pixmap.stroke_path(path, &paint, &stroke, transform, None);
        }
    };

    if let Some(path) = fingers.finish() {
        glowing(&path, FINGER_WIDTH * w, 2.1);
    }
    // Ладонь последней: она перекрывает основания пальцев, поэтому прижатые в
    // ней тонут, а вытянутые растут из цельной формы.
    if let Some(path) = wrist.finish() {
        glowing(&path, palm_half * 2.0, 1.35);
    }

    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::{Gesture, Glow, HandPose, render};

    #[test]
    fn gestures_differ_in_finger_layout() {
        assert_ne!(Gesture::ThumbsUp.fingers(), Gesture::MiddleFinger.fingers());
        assert_eq!(Gesture::Open.fingers(), [1.0; 5]);
        assert_eq!(Gesture::Fist.fingers(), [0.0; 5]);
        // «Фак» — только средний, и никакой другой.
        let rude = Gesture::MiddleFinger.fingers();
        assert_eq!(rude[2], 1.0);
        assert!(rude.iter().enumerate().all(|(i, v)| i == 2 || *v == 0.0));
    }

    #[test]
    fn hidden_hand_is_not_drawn() {
        let pose = HandPose::new();
        assert!(pose.is_hidden());
        assert!(render(&pose, 64, false, Glow::default()).is_none());
    }

    #[test]
    fn hand_materialises_and_dissolves() {
        let mut pose = HandPose::new();
        pose.blend_to(Gesture::Open);
        for _ in 0..120 {
            pose.update(1.0 / 60.0);
        }
        assert!(pose.visibility() > 0.9, "кисть должна проявиться");
        assert!(render(&pose, 64, false, Glow::default()).is_some());

        pose.blend_to(Gesture::Hidden);
        for _ in 0..600 {
            pose.update(1.0 / 60.0);
        }
        assert!(pose.is_hidden(), "кисть должна раствориться без остатка");
    }

    #[test]
    fn fingers_reach_the_requested_gesture() {
        let mut pose = HandPose::new();
        pose.blend_to(Gesture::ThumbsUp);
        for _ in 0..300 {
            pose.update(1.0 / 60.0);
        }
        assert_eq!(pose.gesture(), Gesture::ThumbsUp);
        let drawn = render(&pose, 96, false, Glow::default()).expect("жест виден");
        // Что-то действительно нарисовано, а не пустой холст.
        assert!(drawn.pixels().iter().any(|pixel| pixel.alpha() > 0));
    }

    #[test]
    fn mirrored_hand_differs_from_the_right_one() {
        let mut pose = HandPose::new();
        pose.blend_to(Gesture::ThumbsUp);
        for _ in 0..300 {
            pose.update(1.0 / 60.0);
        }
        let right = render(&pose, 96, false, Glow::default()).expect("правая");
        let left = render(&pose, 96, true, Glow::default()).expect("левая");
        assert_ne!(
            right.data(),
            left.data(),
            "левая кисть должна быть зеркальной"
        );
    }
}
