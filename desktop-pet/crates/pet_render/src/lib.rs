//! CPU-композитор для слоёного окна.
//!
//! Всё хранится в **premultiplied BGRA** — именно этот формат принимает
//! `UpdateLayeredWindow`. Прямая альфа даёт светлый ореол по краю, поэтому
//! домножение делается один раз при загрузке спрайта, а не на каждом кадре.

mod warp;

pub use warp::{WARP_COLS, WARP_ROWS, Warp};

use std::path::Path;

/// Пиксельный буфер в premultiplied BGRA, порядок строк сверху вниз.
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Полная очистка в прозрачный чёрный.
    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// Композит `src` поверх `dst` для premultiplied-данных:
    /// `dst = src + dst * (1 - src_a)`.
    #[inline]
    fn over(dst: &mut [u8], src: [u8; 4]) {
        let inv = 255 - u32::from(src[3]);
        for channel in 0..4 {
            let under = u32::from(dst[channel]) * inv;
            // деление на 255 без деления: (x + 1 + (x >> 8)) >> 8
            let under = (under + 1 + (under >> 8)) >> 8;
            dst[channel] = (u32::from(src[channel]) + under).min(255) as u8;
        }
    }

    /// Заливка прямоугольника цветом BGRA. `color` должен быть premultiplied.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
        if color[3] == 0 || w <= 0 || h <= 0 {
            return;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.width as i32);
        let y1 = (y + h).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let stride = self.width as usize * 4;
        for row in y0..y1 {
            let base = row as usize * stride;
            for column in x0..x1 {
                let offset = base + column as usize * 4;
                Self::over(&mut self.pixels[offset..offset + 4], color);
            }
        }
    }

    /// Накладывает premultiplied **RGBA**-слой — в таком виде отдаёт результат
    /// растеризатор лица. Порядок каналов меняется на BGRA при копировании.
    pub fn draw_rgba(&mut self, rgba: &[u8], width: u32, height: u32, x: i32, y: i32) {
        let sw = width as i32;
        let sh = height as i32;
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + sw).min(self.width as i32);
        let y1 = (y + sh).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let dst_stride = self.width as usize * 4;
        let src_stride = width as usize * 4;
        for row in y0..y1 {
            let src_row = (row - y) as usize * src_stride;
            let dst_row = row as usize * dst_stride;
            for column in x0..x1 {
                let src_offset = src_row + (column - x) as usize * 4;
                let alpha = rgba[src_offset + 3];
                if alpha == 0 {
                    continue;
                }
                let src = [
                    rgba[src_offset + 2],
                    rgba[src_offset + 1],
                    rgba[src_offset],
                    alpha,
                ];
                let dst_offset = dst_row + column as usize * 4;
                Self::over(&mut self.pixels[dst_offset..dst_offset + 4], src);
            }
        }
    }

    /// Заливает эллипс. Нужен для тени: рисовать её спрайтом нельзя, она
    /// меняет форму на каждом шаге походки.
    pub fn fill_ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, color: [u8; 4]) {
        if color[3] == 0 || rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let x0 = ((cx - rx).floor() as i32).max(0);
        let y0 = ((cy - ry).floor() as i32).max(0);
        let x1 = ((cx + rx).ceil() as i32).min(self.width as i32);
        let y1 = ((cy + ry).ceil() as i32).min(self.height as i32);
        let stride = self.width as usize * 4;
        for row in y0..y1 {
            let dy = (row as f32 + 0.5 - cy) / ry;
            if dy.abs() > 1.0 {
                continue;
            }
            let span = (1.0 - dy * dy).sqrt();
            let base = row as usize * stride;
            for column in x0..x1 {
                let dx = (column as f32 + 0.5 - cx) / rx;
                let distance = (dx * dx + dy * dy).sqrt();
                if distance > 1.0 {
                    continue;
                }
                // мягкий край: последняя четверть радиуса плавно гаснет
                let edge = ((1.0 - distance) / 0.28).clamp(0.0, 1.0);
                let _ = span;
                let mut tinted = color;
                let alpha = f32::from(color[3]) * edge;
                let scale = alpha / 255.0;
                for channel in 0..3 {
                    tinted[channel] = (f32::from(color[channel]) * edge) as u8;
                }
                tinted[3] = (scale * 255.0) as u8;
                let offset = base + column as usize * 4;
                Self::over(&mut self.pixels[offset..offset + 4], tinted);
            }
        }
    }

    /// Накладывает другой холст — используется при сборке контрольных листов.
    pub fn draw_canvas(&mut self, other: &Self, x: i32, y: i32) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + other.width as i32).min(self.width as i32);
        let y1 = (y + other.height as i32).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let dst_stride = self.width as usize * 4;
        let src_stride = other.width as usize * 4;
        for row in y0..y1 {
            let src_row = (row - y) as usize * src_stride;
            let dst_row = row as usize * dst_stride;
            for column in x0..x1 {
                let src_offset = src_row + (column - x) as usize * 4;
                let src = [
                    other.pixels[src_offset],
                    other.pixels[src_offset + 1],
                    other.pixels[src_offset + 2],
                    other.pixels[src_offset + 3],
                ];
                if src[3] == 0 {
                    continue;
                }
                let dst_offset = dst_row + column as usize * 4;
                Self::over(&mut self.pixels[dst_offset..dst_offset + 4], src);
            }
        }
    }

    /// Накладывает холст с общей прозрачностью. Оба буфера уже
    /// premultiplied, поэтому множитель применяется ко всем четырём каналам.
    /// Используется только во время короткого перехода фас ↔ профиль.
    pub fn draw_canvas_opacity(&mut self, other: &Self, opacity: f32) {
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }
        assert_eq!((self.width, self.height), (other.width, other.height));
        let factor = (opacity * 255.0).round() as u32;
        for (dst, src) in self
            .pixels
            .chunks_exact_mut(4)
            .zip(other.pixels.chunks_exact(4))
        {
            if src[3] == 0 {
                continue;
            }
            let scaled = [
                ((u32::from(src[0]) * factor + 127) / 255) as u8,
                ((u32::from(src[1]) * factor + 127) / 255) as u8,
                ((u32::from(src[2]) * factor + 127) / 255) as u8,
                ((u32::from(src[3]) * factor + 127) / 255) as u8,
            ];
            Self::over(dst, scaled);
        }
    }

    /// Накладывает спрайт, повёрнутый вокруг точки `pivot`.
    ///
    /// `pivot` задаётся в координатах спрайта. Идёт обратным отображением:
    /// для каждого пикселя результата точка поворачивается назад и берётся
    /// билинейная выборка — прямой поворот оставлял бы дыры.
    ///
    /// Нужно для ног: бедро остаётся на месте, а голень со ступнёй
    /// поворачивается вокруг него.
    pub fn draw_rotated(
        &mut self,
        sprite: &Sprite,
        x: i32,
        y: i32,
        pivot: (f32, f32),
        angle: f32,
        margin: i32,
    ) {
        if angle.abs() < 1e-4 && margin == 0 {
            self.draw(sprite, x, y);
            return;
        }
        let (sin, cos) = (-angle).sin_cos();
        let x0 = (x - margin).max(0);
        let y0 = (y - margin).max(0);
        let x1 = (x + sprite.width as i32 + margin).min(self.width as i32);
        let y1 = (y + sprite.height as i32 + margin).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let dst_stride = self.width as usize * 4;
        for row in y0..y1 {
            let dy = (row - y) as f32 - pivot.1;
            let dst_row = row as usize * dst_stride;
            for column in x0..x1 {
                let dx = (column - x) as f32 - pivot.0;
                let src_x = pivot.0 + dx * cos - dy * sin;
                let src_y = pivot.1 + dx * sin + dy * cos;
                let Some(src) = sprite.sample(src_x, src_y) else {
                    continue;
                };
                if src[3] == 0 {
                    continue;
                }
                let dst_offset = dst_row + column as usize * 4;
                Self::over(&mut self.pixels[dst_offset..dst_offset + 4], src);
            }
        }
    }

    /// Накладывает спрайт, деформированный сеткой.
    ///
    /// Идёт обратным отображением: для каждого пикселя результата считается,
    /// откуда его взять в исходнике, и берётся билинейная выборка. Прямое
    /// отображение оставляло бы дыры между растянутыми участками.
    ///
    /// `margin` — запас с каждой стороны в пикселях, чтобы выехавшие части не
    /// срезались краем.
    pub fn draw_warped(&mut self, sprite: &Sprite, x: i32, y: i32, warp: &Warp, margin: i32) {
        if warp.is_identity() && margin == 0 {
            self.draw(sprite, x, y);
            return;
        }
        let sw = sprite.width as f32;
        let sh = sprite.height as f32;
        let x0 = (x - margin).max(0);
        let y0 = (y - margin).max(0);
        let x1 = (x + sprite.width as i32 + margin).min(self.width as i32);
        let y1 = (y + sprite.height as i32 + margin).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let dst_stride = self.width as usize * 4;
        for row in y0..y1 {
            let v = (row - y) as f32 / sh;
            let dst_row = row as usize * dst_stride;
            for column in x0..x1 {
                let u = (column - x) as f32 / sw;
                let (du, dv) = warp.sample(u, v);
                let src_x = (u + du) * sw;
                let src_y = (v + dv) * sh;
                let Some(src) = sprite.sample(src_x, src_y) else {
                    continue;
                };
                if src[3] == 0 {
                    continue;
                }
                let dst_offset = dst_row + column as usize * 4;
                Self::over(&mut self.pixels[dst_offset..dst_offset + 4], src);
            }
        }
    }

    /// Наложение спрайта левым верхним углом в (`x`, `y`).
    pub fn draw(&mut self, sprite: &Sprite, x: i32, y: i32) {
        let sw = sprite.width as i32;
        let sh = sprite.height as i32;
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + sw).min(self.width as i32);
        let y1 = (y + sh).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let dst_stride = self.width as usize * 4;
        let src_stride = sprite.width as usize * 4;
        for row in y0..y1 {
            let src_row = (row - y) as usize * src_stride;
            let dst_row = row as usize * dst_stride;
            for column in x0..x1 {
                let src_offset = src_row + (column - x) as usize * 4;
                let alpha = sprite.pixels[src_offset + 3];
                if alpha == 0 {
                    continue;
                }
                let src = [
                    sprite.pixels[src_offset],
                    sprite.pixels[src_offset + 1],
                    sprite.pixels[src_offset + 2],
                    alpha,
                ];
                let dst_offset = dst_row + column as usize * 4;
                if alpha == 255 {
                    self.pixels[dst_offset..dst_offset + 4].copy_from_slice(&src);
                } else {
                    Self::over(&mut self.pixels[dst_offset..dst_offset + 4], src);
                }
            }
        }
    }
}

fn content_bounds_rgba(image: &image::RgbaImage, threshold: u8) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0) = (image.width(), image.height());
    let (mut x1, mut y1) = (0_u32, 0_u32);
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel.0[3] > threshold {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    (x0 < x1).then_some((x0, y0, x1, y1))
}

/// Сохраняет кадр поверх шахматки: так видно и прозрачность, и края.
///
/// Нужно для визуальной проверки — смотреть на кадр в окне поверх случайных
/// обоев бессмысленно, дефекты альфы там не читаются.
pub fn save_on_checkerboard(canvas: &Canvas, path: &Path) -> Result<(), image::ImageError> {
    const CELL: u32 = 12;
    let mut out = image::RgbImage::new(canvas.width(), canvas.height());
    for y in 0..canvas.height() {
        for x in 0..canvas.width() {
            let offset = (y as usize * canvas.width() as usize + x as usize) * 4;
            let pixel = &canvas.pixels()[offset..offset + 4];
            let checker = if ((x / CELL) + (y / CELL)).is_multiple_of(2) {
                235_u32
            } else {
                170
            };
            let inverse = 255 - u32::from(pixel[3]);
            let mix = |channel: u32| -> u8 {
                let under = checker * inverse;
                let under = (under + 1 + (under >> 8)) >> 8;
                (channel + under).min(255) as u8
            };
            out.put_pixel(
                x,
                y,
                image::Rgb([
                    mix(u32::from(pixel[2])),
                    mix(u32::from(pixel[1])),
                    mix(u32::from(pixel[0])),
                ]),
            );
        }
    }
    out.save(path)
}

/// Исходник спрайта в полном разрешении.
///
/// Хранится, чтобы менять размер персонажа в рантайме, каждый раз уменьшая
/// **от оригинала**. Пересчёт из уже уменьшенной копии накапливал бы потери,
/// и после пары изменений размера арт превратился бы в кашу.
pub struct SpriteSource {
    image: image::RgbaImage,
    /// Сколько пустых полей срезано слева и сверху. Нужно, чтобы координаты
    /// из манифеста (визор, точки привязки) остались правильными.
    offset: (u32, u32),
}

impl SpriteSource {
    /// Загружает, отсекает шум прозрачности, домножает на альфу и обрезает по
    /// содержимому. Дальше остаётся только уменьшать.
    pub fn load(path: &Path, alpha_floor: u8) -> Result<Self, image::ImageError> {
        let source = image::open(path)?.into_rgba8();
        let mut premultiplied = image::RgbaImage::new(source.width(), source.height());
        for (out, pixel) in premultiplied.pixels_mut().zip(source.pixels()) {
            let [r, g, b, a] = pixel.0;
            let a = if a <= alpha_floor { 0 } else { a };
            let apply = |channel: u8| -> u8 {
                let value = u32::from(channel) * u32::from(a);
                ((value + 1 + (value >> 8)) >> 8) as u8
            };
            *out = image::Rgba([apply(r), apply(g), apply(b), a]);
        }
        let (image, offset) = match content_bounds_rgba(&premultiplied, alpha_floor) {
            Some((x0, y0, x1, y1)) => (
                image::imageops::crop_imm(&premultiplied, x0, y0, x1 - x0, y1 - y0).to_image(),
                (x0, y0),
            ),
            None => (premultiplied, (0, 0)),
        };
        Ok(Self { image, offset })
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.image.height()
    }

    /// Смещение обрезки относительно исходного файла.
    #[must_use]
    pub const fn offset(&self) -> (u32, u32) {
        self.offset
    }

    /// Уменьшает фильтром Ланцоша по premultiplied-данным: это пространство
    /// линейно по альфе, поэтому по краю не появляется светлый ореол.
    #[must_use]
    pub fn scaled(&self, scale: f32) -> Sprite {
        let target_w = ((self.image.width() as f32 * scale).round() as u32).max(1);
        let target_h = ((self.image.height() as f32 * scale).round() as u32).max(1);
        let scaled = if target_w == self.image.width() && target_h == self.image.height() {
            self.image.clone()
        } else {
            image::imageops::resize(
                &self.image,
                target_w,
                target_h,
                image::imageops::FilterType::Lanczos3,
            )
        };
        let mut pixels = Vec::with_capacity((target_w as usize) * (target_h as usize) * 4);
        for pixel in scaled.pixels() {
            let [r, g, b, a] = pixel.0;
            // Ланцош может дать выброс выше альфы, а для premultiplied канал
            // не имеет права её превышать.
            pixels.extend_from_slice(&[b.min(a), g.min(a), r.min(a), a]);
        }
        Sprite {
            width: target_w,
            height: target_h,
            pixels,
        }
    }
}

/// Спрайт в premultiplied BGRA.
pub struct Sprite {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Sprite {
    /// Загружает PNG с прямой альфой и переводит в premultiplied BGRA.
    ///
    /// `alpha_floor` отсекает остаточный шум прозрачности: в исходных артах
    /// встречаются одиночные пиксели с альфой 1–4, из-за которых обрезка по
    /// `alpha > 0` захватывает весь холст.
    /// `scale` — во сколько раз уменьшить (0.2 = впятеро). Уменьшение идёт
    /// фильтром Ланцоша по **premultiplied**-данным: это пространство линейно
    /// по альфе, поэтому прозрачные пиксели не подмешивают свой цвет в
    /// непрозрачные и по краю не появляется ореол.
    pub fn load(path: &Path, alpha_floor: u8, scale: f32) -> Result<Self, image::ImageError> {
        let source = image::open(path)?.into_rgba8();
        let (width, height) = source.dimensions();

        let mut premultiplied = image::RgbaImage::new(width, height);
        for (out, pixel) in premultiplied.pixels_mut().zip(source.pixels()) {
            let [r, g, b, a] = pixel.0;
            let a = if a <= alpha_floor { 0 } else { a };
            let apply = |channel: u8| -> u8 {
                let value = u32::from(channel) * u32::from(a);
                ((value + 1 + (value >> 8)) >> 8) as u8
            };
            *out = image::Rgba([apply(r), apply(g), apply(b), a]);
        }

        // обрезка по содержимому до уменьшения — иначе пустые поля съедают
        // разрешение и портят фильтрацию края
        let bounds = content_bounds_rgba(&premultiplied, alpha_floor);
        let premultiplied = match bounds {
            Some((x0, y0, x1, y1)) => {
                image::imageops::crop_imm(&premultiplied, x0, y0, x1 - x0, y1 - y0).to_image()
            }
            None => premultiplied,
        };

        let scaled = if (scale - 1.0).abs() < f32::EPSILON {
            premultiplied
        } else {
            let target_w = ((premultiplied.width() as f32 * scale).round() as u32).max(1);
            let target_h = ((premultiplied.height() as f32 * scale).round() as u32).max(1);
            image::imageops::resize(
                &premultiplied,
                target_w,
                target_h,
                image::imageops::FilterType::Lanczos3,
            )
        };

        let (width, height) = scaled.dimensions();
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for pixel in scaled.pixels() {
            let [r, g, b, a] = pixel.0;
            // Ланцош может дать выброс выше альфы — для premultiplied это
            // недопустимо, канал не может превышать собственную альфу.
            pixels.extend_from_slice(&[b.min(a), g.min(a), r.min(a), a]);
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Билинейная выборка. `None` — точка вне спрайта.
    ///
    /// Интерполяция идёт по premultiplied-значениям: это пространство линейно
    /// по альфе, поэтому прозрачные соседи не подмешивают свой цвет и по краю
    /// деформации не возникает ореола.
    #[must_use]
    fn sample(&self, x: f32, y: f32) -> Option<[u8; 4]> {
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        if x0 >= self.width || y0 >= self.height {
            return None;
        }
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;

        let at = |cx: u32, cy: u32| -> &[u8] {
            let offset = (cy as usize * self.width as usize + cx as usize) * 4;
            &self.pixels[offset..offset + 4]
        };
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let mut out = [0_u8; 4];
        for channel in 0..4 {
            let top = f32::from(a[channel]) + (f32::from(b[channel]) - f32::from(a[channel])) * tx;
            let bottom =
                f32::from(c[channel]) + (f32::from(d[channel]) - f32::from(c[channel])) * tx;
            out[channel] = (top + (bottom - top) * ty).round().clamp(0.0, 255.0) as u8;
        }
        Some(out)
    }

    /// Границы непрозрачной области: (x0, y0, x1, y1), правый и нижний край
    /// исключаются. `None`, если спрайт полностью прозрачен.
    #[must_use]
    pub fn content_bounds(&self, threshold: u8) -> Option<(u32, u32, u32, u32)> {
        let (mut x0, mut y0) = (self.width, self.height);
        let (mut x1, mut y1) = (0_u32, 0_u32);
        for y in 0..self.height {
            let row = y as usize * self.width as usize * 4;
            for x in 0..self.width {
                if self.pixels[row + x as usize * 4 + 3] > threshold {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        (x0 < x1).then_some((x0, y0, x1, y1))
    }

    /// Вырезает прямоугольник в новый спрайт.
    #[must_use]
    pub fn crop(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Self {
        let width = x1 - x0;
        let height = y1 - y0;
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in y0..y1 {
            let row = (y as usize * self.width as usize + x0 as usize) * 4;
            pixels.extend_from_slice(&self.pixels[row..row + width as usize * 4]);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Уменьшение целым коэффициентом усреднением блока.
    ///
    /// Усреднение идёт по premultiplied-значениям — это корректно, потому что
    /// premultiplied-пространство линейно по альфе, и прозрачные пиксели не
    /// подмешивают свой цвет в непрозрачные.
    #[must_use]
    pub fn downscale(&self, factor: u32) -> Self {
        if factor <= 1 {
            return Self {
                width: self.width,
                height: self.height,
                pixels: self.pixels.clone(),
            };
        }
        let width = (self.width / factor).max(1);
        let height = (self.height / factor).max(1);
        let mut pixels = vec![0_u8; (width as usize) * (height as usize) * 4];
        let samples = factor * factor;
        for y in 0..height {
            for x in 0..width {
                let mut sum = [0_u32; 4];
                for dy in 0..factor {
                    let sy = y * factor + dy;
                    let row = sy as usize * self.width as usize * 4;
                    for dx in 0..factor {
                        let offset = row + (x * factor + dx) as usize * 4;
                        for (channel, total) in sum.iter_mut().enumerate() {
                            *total += u32::from(self.pixels[offset + channel]);
                        }
                    }
                }
                let offset = (y as usize * width as usize + x as usize) * 4;
                for (channel, total) in sum.iter().enumerate() {
                    pixels[offset + channel] = (total / samples) as u8;
                }
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Canvas;

    #[test]
    fn opaque_fill_replaces_and_transparent_fill_leaves_canvas_untouched() {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill_rect(0, 0, 4, 4, [10, 20, 30, 255]);
        assert_eq!(&canvas.pixels()[0..4], &[10, 20, 30, 255]);

        canvas.fill_rect(0, 0, 4, 4, [0, 0, 0, 0]);
        assert_eq!(&canvas.pixels()[0..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn half_alpha_over_opaque_keeps_result_in_range() {
        let mut canvas = Canvas::new(1, 1);
        canvas.fill_rect(0, 0, 1, 1, [200, 200, 200, 255]);
        // premultiplied: половина альфы, цвет уже домножен
        canvas.fill_rect(0, 0, 1, 1, [64, 64, 64, 128]);
        let pixel = &canvas.pixels()[0..4];
        assert_eq!(
            pixel[3], 255,
            "альфа под непрозрачным слоем не должна падать"
        );
        assert!(pixel[0] > 64 && pixel[0] < 200);
    }

    #[test]
    fn drawing_outside_the_canvas_is_clipped_without_panic() {
        let mut canvas = Canvas::new(8, 8);
        canvas.fill_rect(-100, -100, 4, 4, [1, 2, 3, 255]);
        canvas.fill_rect(100, 100, 4, 4, [1, 2, 3, 255]);
        canvas.fill_rect(-2, -2, 4, 4, [9, 9, 9, 255]);
        assert_eq!(&canvas.pixels()[0..4], &[9, 9, 9, 255]);
    }

    #[test]
    fn canvas_opacity_scales_premultiplied_colour_and_alpha() {
        let mut source = Canvas::new(1, 1);
        source.fill_rect(0, 0, 1, 1, [128, 64, 32, 128]);
        let mut target = Canvas::new(1, 1);
        target.draw_canvas_opacity(&source, 0.5);
        assert_eq!(&target.pixels()[0..4], &[64, 32, 16, 64]);

        let unchanged = target.pixels().to_vec();
        target.draw_canvas_opacity(&source, 0.0);
        assert_eq!(target.pixels(), unchanged);
    }
}
