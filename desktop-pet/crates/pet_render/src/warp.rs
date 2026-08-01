//! Деформация спрайта управляющей сеткой.
//!
//! Персонаж — одна плоская картинка, но живым он должен быть без разрезания на
//! слои: дышать, прижимать уши, вилять хвостом, сплющиваться при приземлении.
//! Всё это делается смещением узлов сетки, наложенной на спрайт.
//!
//! Сетка задаёт **обратное отображение**: для каждого пикселя результата
//! известно, откуда его брать в исходнике. Прямое отображение потребовало бы
//! обращения, а обратное считается сразу и не оставляет дыр.

/// Узлов по горизонтали и вертикали. Четыре на пять хватает, чтобы отдельно
/// двигать уши, корпус и хвост, и при этом сетка остаётся дешёвой.
pub const WARP_COLS: usize = 4;
pub const WARP_ROWS: usize = 5;

/// Смещения узлов в долях размера спрайта.
///
/// Значение сдвигает **точку выборки**, а не саму картинку: чтобы ухо
/// наклонилось вправо, брать надо левее, то есть смещение отрицательное.
#[derive(Clone, Copy, Debug)]
pub struct Warp {
    offsets: [[(f32, f32); WARP_COLS]; WARP_ROWS],
}

impl Default for Warp {
    fn default() -> Self {
        Self::identity()
    }
}

impl Warp {
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            offsets: [[(0.0, 0.0); WARP_COLS]; WARP_ROWS],
        }
    }

    /// Смещает один узел.
    pub fn set(&mut self, column: usize, row: usize, dx: f32, dy: f32) {
        if let Some(cell) = self
            .offsets
            .get_mut(row)
            .and_then(|line| line.get_mut(column))
        {
            *cell = (dx, dy);
        }
    }

    /// Добавляет смещение к узлу.
    pub fn add(&mut self, column: usize, row: usize, dx: f32, dy: f32) {
        if let Some(cell) = self
            .offsets
            .get_mut(row)
            .and_then(|line| line.get_mut(column))
        {
            cell.0 += dx;
            cell.1 += dy;
        }
    }

    /// Смещает целый ряд — удобно для дыхания и наклона корпуса.
    pub fn add_row(&mut self, row: usize, dx: f32, dy: f32) {
        for column in 0..WARP_COLS {
            self.add(column, row, dx, dy);
        }
    }

    /// Билинейная интерполяция смещения в точке (`u`, `v`), обе в 0…1.
    #[must_use]
    pub fn sample(&self, u: f32, v: f32) -> (f32, f32) {
        let fx = (u.clamp(0.0, 1.0)) * (WARP_COLS - 1) as f32;
        let fy = (v.clamp(0.0, 1.0)) * (WARP_ROWS - 1) as f32;
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let x1 = (x0 + 1).min(WARP_COLS - 1);
        let y1 = (y0 + 1).min(WARP_ROWS - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;

        let mix = |a: (f32, f32), b: (f32, f32), t: f32| -> (f32, f32) {
            (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
        };
        let top = mix(self.offsets[y0][x0], self.offsets[y0][x1], tx);
        let bottom = mix(self.offsets[y1][x0], self.offsets[y1][x1], tx);
        mix(top, bottom, ty)
    }

    /// Наибольшее смещение по модулю — по нему считается запас холста, чтобы
    /// деформированные части не срезались краем.
    #[must_use]
    pub fn extent(&self) -> (f32, f32) {
        let mut max_x = 0.0_f32;
        let mut max_y = 0.0_f32;
        for row in &self.offsets {
            for (dx, dy) in row {
                max_x = max_x.max(dx.abs());
                max_y = max_y.max(dy.abs());
            }
        }
        (max_x, max_y)
    }

    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.offsets
            .iter()
            .flatten()
            .all(|(dx, dy)| dx.abs() < 1e-4 && dy.abs() < 1e-4)
    }
}

#[cfg(test)]
mod tests {
    use super::{WARP_COLS, WARP_ROWS, Warp};

    #[test]
    fn identity_samples_to_zero_everywhere() {
        let warp = Warp::identity();
        assert!(warp.is_identity());
        for step in 0..=10 {
            let t = step as f32 / 10.0;
            assert_eq!(warp.sample(t, t), (0.0, 0.0));
        }
    }

    #[test]
    fn sampling_interpolates_between_nodes() {
        let mut warp = Warp::identity();
        warp.set(0, 0, -1.0, 0.0);
        warp.set(WARP_COLS - 1, 0, 1.0, 0.0);
        // ровно посередине верхнего ряда смещения гасят друг друга
        let (dx, _) = warp.sample(0.5, 0.0);
        assert!(dx.abs() < 1e-5, "ожидался ноль в середине, получено {dx}");
        // у краёв — исходные значения
        assert!((warp.sample(0.0, 0.0).0 + 1.0).abs() < 1e-5);
        assert!((warp.sample(1.0, 0.0).0 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn extent_reports_the_largest_offset() {
        let mut warp = Warp::identity();
        warp.set(1, 2, 0.05, -0.12);
        warp.set(2, 3, -0.03, 0.04);
        let (x, y) = warp.extent();
        assert!((x - 0.05).abs() < 1e-6);
        assert!((y - 0.12).abs() < 1e-6);
    }

    #[test]
    fn row_offsets_apply_to_every_column() {
        let mut warp = Warp::identity();
        warp.add_row(WARP_ROWS - 1, 0.0, 0.2);
        for column in 0..WARP_COLS {
            let u = column as f32 / (WARP_COLS - 1) as f32;
            assert!((warp.sample(u, 1.0).1 - 0.2).abs() < 1e-5);
        }
        assert!(!warp.is_identity());
    }
}
