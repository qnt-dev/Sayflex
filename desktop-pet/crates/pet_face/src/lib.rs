//! Процедурное лицо на визоре.
//!
//! Глаза, крестик и рот не хранятся картинками — они рисуются кодом по
//! параметрам. Отсюда берётся вся мимика: моргание, прищур, круглый глаз,
//! перескок крестика с глаза на глаз, кривая рта. Ни одно новое выражение не
//! требует нового ассета.
//!
//! Смена формы глаза — это перекрёстное затухание двух глифов, а не мгновенная
//! подмена: `EyeState::blend` держит долю перехода, и оба глифа рисуются с
//! дополняющей прозрачностью.

use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// Форма глаза.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EyeGlyph {
    /// Полумесяц — довольный прищур, базовая форма персонажа.
    Arc,
    /// Полный круг — удивление.
    Circle,
    /// Сжатый по вертикали овал — подозрение.
    Squint,
    /// Тонкая черта — закрытый глаз, моргание.
    Closed,
    /// Крестик.
    Cross,
    /// Наклонённый овал — злость.
    Angry,
}

/// Цвет в straight RGB; альфа задаётся отдельно при отрисовке.
#[derive(Clone, Copy, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const EYE_CYAN: Rgb = Rgb(0x7C, 0xF2, 0xE4);
pub const MARK_WHITE: Rgb = Rgb(0xF0, 0xE6, 0xFF);

#[derive(Clone, Copy, Debug)]
pub struct EyeState {
    pub glyph: EyeGlyph,
    /// Во что перетекает глаз; совпадает с `glyph`, когда переход завершён.
    pub next: EyeGlyph,
    /// 0 — целиком `glyph`, 1 — целиком `next`.
    pub blend: f32,
    /// Раскрытие: 0 — схлопнут, 1 — обычный, до 1.6 — вытаращен.
    pub openness: f32,
    /// Смещение зрачка вслед за курсором, в долях полуразмера глаза.
    pub gaze: (f32, f32),
    pub color: Rgb,
}

impl Default for EyeState {
    fn default() -> Self {
        Self {
            glyph: EyeGlyph::Arc,
            next: EyeGlyph::Arc,
            blend: 0.0,
            openness: 1.0,
            gaze: (0.0, 0.0),
            color: EYE_CYAN,
        }
    }
}

impl EyeState {
    /// Запускает переход к новой форме.
    pub fn morph_to(&mut self, glyph: EyeGlyph) {
        if self.glyph == glyph && self.blend == 0.0 {
            return;
        }
        // если предыдущий переход не доигран, текущая форма — то, что видно
        if self.blend >= 1.0 {
            self.glyph = self.next;
        }
        self.next = glyph;
        self.blend = 0.0;
    }

    /// Продвигает переход. `speed` — долей в секунду.
    pub fn advance(&mut self, dt: f32, speed: f32) {
        if self.glyph == self.next {
            self.blend = 0.0;
            return;
        }
        self.blend = (self.blend + dt * speed).min(1.0);
        if self.blend >= 1.0 {
            self.glyph = self.next;
            self.blend = 0.0;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FaceState {
    pub left: EyeState,
    pub right: EyeState,
    /// −1 — уголки вниз, 0 — черта, +1 — улыбка.
    pub mouth_curve: f32,
    /// Ширина рта в долях базовой.
    pub mouth_width: f32,
    /// Раскрытие рта: 0 — линия, 1 — овал.
    pub mouth_open: f32,
    pub mouth_color: Rgb,
    /// Общая яркость лица: гаснет во сне, вспыхивает при реакции.
    pub glow: f32,
}

impl Default for FaceState {
    fn default() -> Self {
        Self {
            left: EyeState::default(),
            right: EyeState {
                glyph: EyeGlyph::Cross,
                next: EyeGlyph::Cross,
                color: MARK_WHITE,
                ..EyeState::default()
            },
            mouth_curve: 1.0,
            mouth_width: 1.0,
            mouth_open: 0.0,
            mouth_color: EYE_CYAN,
            glow: 1.0,
        }
    }
}

impl FaceState {
    /// Переставляет крестик на другой глаз. Тот глаз, где крестик был,
    /// возвращается к обычной форме.
    pub fn set_cross_on_left(&mut self, on_left: bool) {
        if on_left {
            self.left.morph_to(EyeGlyph::Cross);
            self.left.color = MARK_WHITE;
            self.right.morph_to(EyeGlyph::Arc);
            self.right.color = EYE_CYAN;
        } else {
            self.right.morph_to(EyeGlyph::Cross);
            self.right.color = MARK_WHITE;
            self.left.morph_to(EyeGlyph::Arc);
            self.left.color = EYE_CYAN;
        }
    }

    pub fn advance(&mut self, dt: f32) {
        self.left.advance(dt, 6.0);
        self.right.advance(dt, 6.0);
    }
}

/// Геометрия визора в пикселях исходного арта.
#[derive(Clone, Copy, Debug)]
pub struct Visor {
    pub width: f32,
    pub height: f32,
    /// Центры глаз и рта в долях рамки визора.
    pub left_eye: (f32, f32),
    pub right_eye: (f32, f32),
    pub mouth: (f32, f32),
}

/// Рисует лицо в собственный слой размера визора при текущем масштабе.
///
/// Возвращает premultiplied RGBA — вызывающий накладывает его на визор.
#[must_use]
pub fn render(face: &FaceState, visor: Visor, scale: f32) -> Pixmap {
    let width = ((visor.width * scale).round() as u32).max(1);
    let height = ((visor.height * scale).round() as u32).max(1);
    let mut pixmap = Pixmap::new(width, height).expect("ненулевой размер визора");

    // Базовый размер глаза привязан к визору, поэтому лицо масштабируется
    // вместе с персонажем без отдельной настройки.
    let eye_radius = visor.width * scale * 0.115;
    let mouth_width = visor.width * scale * 0.30 * face.mouth_width;

    draw_eye(
        &mut pixmap,
        (
            visor.left_eye.0 * width as f32,
            visor.left_eye.1 * height as f32,
        ),
        eye_radius,
        &face.left,
        face.glow,
    );
    draw_eye(
        &mut pixmap,
        (
            visor.right_eye.0 * width as f32,
            visor.right_eye.1 * height as f32,
        ),
        eye_radius,
        &face.right,
        face.glow,
    );
    draw_mouth(
        &mut pixmap,
        (visor.mouth.0 * width as f32, visor.mouth.1 * height as f32),
        mouth_width,
        face,
    );
    pixmap
}

/// Лицо для строгого профиля: виден один глаз и короткий рот. В правом
/// профиле ближним считается левый глаз персонажа, в левом — правый.
#[must_use]
pub fn render_profile(face: &FaceState, visor: Visor, scale: f32, facing_right: bool) -> Pixmap {
    let width = ((visor.width * scale).round() as u32).max(1);
    let height = ((visor.height * scale).round() as u32).max(1);
    let mut pixmap = Pixmap::new(width, height).expect("ненулевой размер визора");
    let eye = if facing_right {
        &face.left
    } else {
        &face.right
    };
    let eye_position = if facing_right {
        visor.left_eye
    } else {
        (1.0 - visor.left_eye.0, visor.left_eye.1)
    };
    let mouth_position = if facing_right {
        visor.mouth
    } else {
        (1.0 - visor.mouth.0, visor.mouth.1)
    };
    let eye_radius = visor.height * scale * 0.10;
    draw_eye(
        &mut pixmap,
        (
            eye_position.0 * width as f32,
            eye_position.1 * height as f32,
        ),
        eye_radius,
        eye,
        face.glow,
    );
    draw_mouth(
        &mut pixmap,
        (
            mouth_position.0 * width as f32,
            mouth_position.1 * height as f32,
        ),
        visor.width * scale * 0.34 * face.mouth_width,
        face,
    );
    pixmap
}

fn paint_for(color: Rgb, alpha: f32) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(
        color.0,
        color.1,
        color.2,
        (alpha.clamp(0.0, 1.0) * 255.0) as u8,
    );
    paint.anti_alias = true;
    paint
}

fn draw_eye(pixmap: &mut Pixmap, centre: (f32, f32), radius: f32, eye: &EyeState, glow: f32) {
    let centre = (
        centre.0 + eye.gaze.0 * radius * 0.35,
        centre.1 + eye.gaze.1 * radius * 0.35,
    );
    if eye.glyph == eye.next || eye.blend <= 0.0 {
        draw_glyph(pixmap, centre, radius, eye.glyph, eye, glow);
        return;
    }
    // перекрёстное затухание: старая форма тает, новая проявляется
    draw_glyph(
        pixmap,
        centre,
        radius,
        eye.glyph,
        eye,
        glow * (1.0 - eye.blend),
    );
    draw_glyph(pixmap, centre, radius, eye.next, eye, glow * eye.blend);
}

fn draw_glyph(
    pixmap: &mut Pixmap,
    centre: (f32, f32),
    radius: f32,
    glyph: EyeGlyph,
    eye: &EyeState,
    alpha: f32,
) {
    let paint = paint_for(eye.color, alpha);
    let open = eye.openness.clamp(0.05, 1.8);
    let (cx, cy) = centre;

    match glyph {
        EyeGlyph::Cross => {
            let arm = radius * 0.78;
            let mut builder = PathBuilder::new();
            builder.move_to(cx - arm, cy - arm);
            builder.line_to(cx + arm, cy + arm);
            builder.move_to(cx + arm, cy - arm);
            builder.line_to(cx - arm, cy + arm);
            if let Some(path) = builder.finish() {
                let stroke = Stroke {
                    width: radius * 0.46,
                    line_cap: tiny_skia::LineCap::Round,
                    ..Stroke::default()
                };
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        EyeGlyph::Closed => {
            let mut builder = PathBuilder::new();
            builder.move_to(cx - radius * 0.85, cy);
            builder.quad_to(cx, cy + radius * 0.18, cx + radius * 0.85, cy);
            if let Some(path) = builder.finish() {
                let stroke = Stroke {
                    width: (radius * 0.30).max(1.0),
                    line_cap: tiny_skia::LineCap::Round,
                    ..Stroke::default()
                };
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
        EyeGlyph::Arc => {
            // полумесяц: полукруг с плоским низом, как в исходном арте
            let mut builder = PathBuilder::new();
            let rx = radius;
            let ry = radius * open;
            builder.move_to(cx - rx, cy + ry * 0.42);
            builder.cubic_to(
                cx - rx,
                cy - ry * 1.15,
                cx + rx,
                cy - ry * 1.15,
                cx + rx,
                cy + ry * 0.42,
            );
            builder.close();
            if let Some(path) = builder.finish() {
                pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        EyeGlyph::Circle | EyeGlyph::Squint | EyeGlyph::Angry => {
            let (scale_x, scale_y, tilt): (f32, f32, f32) = match glyph {
                EyeGlyph::Circle => (1.0, 1.0, 0.0),
                EyeGlyph::Squint => (1.05, 0.34, 0.0),
                // Angry: приплюснутый овал с наклоном к переносице.
                _ => (1.0, 0.62, -24.0),
            };
            let rx = radius * scale_x;
            let ry = radius * scale_y * open;
            if let Some(path) =
                PathBuilder::from_oval(tiny_skia::Rect::from_ltrb(-rx, -ry, rx, ry).unwrap_or(
                    tiny_skia::Rect::from_ltrb(-1.0, -1.0, 1.0, 1.0).expect("единичный овал"),
                ))
            {
                let transform = Transform::from_rotate(tilt).post_translate(cx, cy);
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
        }
    }
}

fn draw_mouth(pixmap: &mut Pixmap, centre: (f32, f32), width: f32, face: &FaceState) {
    let paint = paint_for(face.mouth_color, face.glow);
    let (cx, cy) = centre;
    let half = width / 2.0;
    // Кривизна задаётся одним числом: положительная — улыбка, ноль — черта,
    // отрицательная — уголки вниз.
    let bend = face.mouth_curve.clamp(-1.0, 1.0) * half * 0.62;

    if face.mouth_open > 0.02 {
        let mut builder = PathBuilder::new();
        let depth = half * 0.85 * face.mouth_open;
        builder.move_to(cx - half, cy);
        builder.quad_to(cx, cy + bend + depth, cx + half, cy);
        builder.quad_to(cx, cy - depth * 0.25, cx - half, cy);
        builder.close();
        if let Some(path) = builder.finish() {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        return;
    }

    let mut builder = PathBuilder::new();
    builder.move_to(cx - half, cy);
    builder.quad_to(cx, cy + bend, cx + half, cy);
    if let Some(path) = builder.finish() {
        let stroke = Stroke {
            width: (half * 0.24).max(1.0),
            line_cap: tiny_skia::LineCap::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::{EyeGlyph, FaceState, Visor, render, render_profile};

    fn visor() -> Visor {
        Visor {
            width: 272.0,
            height: 140.0,
            left_eye: (0.32, 0.39),
            right_eye: (0.74, 0.44),
            mouth: (0.52, 0.73),
        }
    }

    fn lit_pixels(face: &FaceState, scale: f32) -> usize {
        render(face, visor(), scale)
            .pixels()
            .iter()
            .filter(|pixel| pixel.alpha() > 8)
            .count()
    }

    #[test]
    fn face_scales_with_the_character() {
        let face = FaceState::default();
        let small = render(&face, visor(), 0.2);
        let large = render(&face, visor(), 0.4);
        // округление размера визора допускает расхождение в один пиксель
        assert!(large.width().abs_diff(small.width() * 2) <= 1);
        // вдвое больший визор даёт примерно вчетверо больше засвеченных пикселей
        let ratio = lit_pixels(&face, 0.4) as f32 / lit_pixels(&face, 0.2).max(1) as f32;
        assert!(
            (3.0..5.2).contains(&ratio),
            "лицо не масштабируется: {ratio}"
        );
    }

    #[test]
    fn closing_the_eyes_reduces_lit_area() {
        let open = FaceState::default();
        let mut closed = FaceState::default();
        closed.left.glyph = EyeGlyph::Closed;
        closed.left.next = EyeGlyph::Closed;
        closed.right.glyph = EyeGlyph::Closed;
        closed.right.next = EyeGlyph::Closed;
        assert!(lit_pixels(&closed, 0.5) < lit_pixels(&open, 0.5));
    }

    #[test]
    fn cross_swaps_between_eyes() {
        let mut face = FaceState::default();
        face.set_cross_on_left(true);
        // переход стартует и завершается за конечное время
        for _ in 0..40 {
            face.advance(1.0 / 60.0);
        }
        assert_eq!(face.left.glyph, EyeGlyph::Cross);
        assert_eq!(face.right.glyph, EyeGlyph::Arc);

        face.set_cross_on_left(false);
        for _ in 0..40 {
            face.advance(1.0 / 60.0);
        }
        assert_eq!(face.right.glyph, EyeGlyph::Cross);
        assert_eq!(face.left.glyph, EyeGlyph::Arc);
    }

    #[test]
    fn morphing_never_leaves_the_blend_out_of_range() {
        let mut face = FaceState::default();
        face.left.morph_to(EyeGlyph::Circle);
        for _ in 0..200 {
            face.advance(1.0 / 60.0);
            assert!((0.0..=1.0).contains(&face.left.blend));
        }
        assert_eq!(face.left.glyph, EyeGlyph::Circle);
    }

    #[test]
    fn profile_face_uses_one_eye_and_keeps_the_requested_size() {
        let face = FaceState::default();
        let full = render(&face, visor(), 0.5);
        let profile = render_profile(&face, visor(), 0.5, true);
        assert_eq!((profile.width(), profile.height()), (136, 70));
        let full_lit = full
            .pixels()
            .iter()
            .filter(|pixel| pixel.alpha() > 8)
            .count();
        let profile_lit = profile
            .pixels()
            .iter()
            .filter(|pixel| pixel.alpha() > 8)
            .count();
        assert!(profile_lit > 0);
        assert!(profile_lit < full_lit);
    }
}
