//! Система квадратных частиц.
//!
//! Один предвыделенный пул, структура «массив полей» (SoA), нулевые выделения
//! памяти в кадре. Индекс в пуле служит устойчивым идентификатором частицы —
//! это то, что позволяет шарику взорваться и собраться обратно теми же самыми
//! частицами, а не случайными.
//!
//! Эмиссия хвоста считается **по пройденному расстоянию**, а не по числу
//! кадров: иначе при 30 fps хвост будет вдвое реже, чем при 60.

/// Двумерный вектор. Своя реализация, чтобы не тянуть зависимость ради двух
/// полей.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, scale: f32) -> Self {
        Self::new(self.x * scale, self.y * scale)
    }
}

/// Поведение частицы.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Свободный слот.
    Dead,
    /// Кружит вокруг владельца.
    Orbit,
    /// Отстаёт и гаснет — метеоритный хвост.
    Trail,
    /// Разлетается после взрыва.
    Burst,
    /// Летит к назначенной точке и там гасит скорость.
    Reassemble,
}

/// Палитра. Цвет отделён от поведения, чтобы состояние персонажа могло
/// перекрашивать частицы, не трогая физику.
#[derive(Clone, Copy, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const VIOLET: Rgb = Rgb(0xA8, 0x55, 0xF7);
pub const MAGENTA: Rgb = Rgb(0xE8, 0x4B, 0xD0);
pub const CYAN: Rgb = Rgb(0x4D, 0xE8, 0xE0);
pub const BLUE: Rgb = Rgb(0x5B, 0x8C, 0xF5);

pub const PALETTE: [Rgb; 4] = [VIOLET, MAGENTA, CYAN, BLUE];

/// Отрисовываемая частица.
#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub position: Vec2,
    pub size: f32,
    /// 0…1, с учётом угасания по времени жизни и удалённости.
    pub alpha: f32,
    pub color: Rgb,
    /// −1 — частица дальше всего за спиной, +1 — ближе всего к зрителю.
    /// По знаку решается, рисовать её до спрайта или после.
    pub depth: f32,
}

/// Детерминированный генератор. Свой, чтобы прогон в тестовой лаборатории
/// повторялся кадр в кадр при одном и том же зерне.
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Равномерно в [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1 << 24) as f32
    }

    /// Равномерно в [low, high).
    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + self.unit() * (high - low)
    }
}

pub struct Particles {
    position: Vec<Vec2>,
    velocity: Vec<Vec2>,
    target: Vec<Vec2>,
    size: Vec<f32>,
    life: Vec<f32>,
    life_span: Vec<f32>,
    phase: Vec<f32>,
    radius: Vec<f32>,
    /// Угловая скорость со знаком: знак задаёт направление вращения.
    spin: Vec<f32>,
    /// Наклон плоскости орбиты относительно плоскости экрана — он и даёт
    /// уход «вглубь».
    tilt: Vec<f32>,
    /// Разворот плоскости орбиты вокруг оси взгляда.
    plane: Vec<f32>,
    /// Текущая нормированная глубина, пересчитывается в `update`.
    depth: Vec<f32>,
    /// Центр орбиты каждой частицы. Раньше центр был один на всё облако, и
    /// частицы, посеянные у ушей и хвоста, всё равно кружили вокруг середины
    /// тела. Теперь якорь хранится вместе с частицей.
    home: Vec<Vec2>,
    color: Vec<Rgb>,
    mode: Vec<Mode>,
    free: Vec<u32>,
    rng: Rng,
    /// Множитель размера. Частицы задаются в пикселях исходного арта, а
    /// персонаж на экране уменьшен — без этого квадратики оказываются
    /// несоразмерно крупными.
    size_scale: f32,
    /// Общий множитель яркости облака: реакция на печать и голос.
    brightness: f32,
    /// Смещение якорей вслед за анимацией тела.
    ///
    /// Якорь хранит место покоя, а персонаж при прыжке поднимается — без
    /// этого смещения облако осталось бы висеть на земле, пока тело в
    /// воздухе.
    anchor_offset: Vec2,
    /// Остаток пути, не покрытый эмиссией на прошлом кадре.
    emit_debt: f32,
    last_origin: Option<Vec2>,
}

impl Particles {
    /// `size_scale` — во сколько раз уменьшен арт на экране.
    #[must_use]
    pub fn with_capacity(capacity: usize, seed: u64, size_scale: f32) -> Self {
        Self {
            position: vec![Vec2::default(); capacity],
            velocity: vec![Vec2::default(); capacity],
            target: vec![Vec2::default(); capacity],
            size: vec![0.0; capacity],
            life: vec![0.0; capacity],
            life_span: vec![1.0; capacity],
            phase: vec![0.0; capacity],
            radius: vec![0.0; capacity],
            spin: vec![0.0; capacity],
            tilt: vec![0.0; capacity],
            plane: vec![0.0; capacity],
            depth: vec![0.0; capacity],
            home: vec![Vec2::default(); capacity],
            color: vec![VIOLET; capacity],
            mode: vec![Mode::Dead; capacity],
            free: (0..capacity as u32).rev().collect(),
            rng: Rng::new(seed),
            size_scale,
            brightness: 1.0,
            anchor_offset: Vec2::default(),
            emit_debt: 0.0,
            last_origin: None,
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.mode.len()
    }

    #[must_use]
    pub fn alive(&self) -> usize {
        self.capacity() - self.free.len()
    }

    fn allocate(&mut self) -> Option<usize> {
        self.free.pop().map(|index| index as usize)
    }

    fn release(&mut self, index: usize) {
        if self.mode[index] != Mode::Dead {
            self.mode[index] = Mode::Dead;
            self.free.push(index as u32);
        }
    }

    /// Раскладывает `count` частиц по кольцу вокруг центра. Используется для
    /// спокойного состояния и как исходное состояние шарика.
    pub fn seed_orbit(&mut self, center: Vec2, inner: f32, outer: f32, count: usize) {
        for _ in 0..count {
            let Some(index) = self.allocate() else { return };
            let angle = self.rng.range(0.0, std::f32::consts::TAU);
            let radius = self.rng.range(inner, outer);
            self.position[index] = center;
            self.velocity[index] = Vec2::default();
            self.size[index] = self.rng.range(8.0, 24.0) * self.size_scale;
            self.life[index] = 0.0;
            self.life_span[index] = f32::INFINITY;
            self.phase[index] = angle;
            self.radius[index] = radius;
            // Каждая частица получает собственную плоскость орбиты, своё
            // направление и свою скорость. Без этого облако читается как одно
            // жёсткое кольцо, вращающееся целиком.
            let direction = if self.rng.unit() < 0.5 { -1.0 } else { 1.0 };
            self.spin[index] = direction * self.rng.range(0.18, 0.85);
            self.tilt[index] = self.rng.range(-1.35, 1.35);
            self.plane[index] = self.rng.range(0.0, std::f32::consts::TAU);
            self.depth[index] = 0.0;
            self.home[index] = center;
            self.color[index] = PALETTE[(self.rng.unit() * 4.0) as usize & 3];
            self.mode[index] = Mode::Orbit;
        }
    }

    /// Куда сместилось тело относительно позы покоя. Применяется только к
    /// орбитальным частицам: хвост и разлетевшиеся живут в мировых
    /// координатах и телу не подчиняются.
    pub fn set_anchor_offset(&mut self, offset: Vec2) {
        self.anchor_offset = offset;
    }

    /// Общая яркость облака. Значение выше единицы даёт вспышку.
    pub fn set_brightness(&mut self, value: f32) {
        self.brightness = value.clamp(0.0, 2.0);
    }

    /// Гасит всё облако и раскладывает его заново по кольцу. Нужно после
    /// сборки шарика: частицы возвращаются к спокойному вращению.
    pub fn reseed_orbit(&mut self, center: Vec2, inner: f32, outer: f32, count: usize) {
        for index in 0..self.capacity() {
            self.release(index);
        }
        self.seed_orbit(center, inner, outer, count);
    }

    /// Эмиссия хвоста по пройденному пути. `spacing` — расстояние между
    /// соседними частицами в пикселях.
    pub fn emit_trail(&mut self, origin: Vec2, spacing: f32, drift: Vec2) {
        let Some(previous) = self.last_origin.replace(origin) else {
            return;
        };
        let step = origin - previous;
        let distance = step.length();
        if distance <= f32::EPSILON {
            return;
        }
        self.emit_debt += distance;
        while self.emit_debt >= spacing {
            self.emit_debt -= spacing;
            let Some(index) = self.allocate() else {
                self.emit_debt = 0.0;
                return;
            };
            let t = self.emit_debt / distance;
            self.position[index] = origin - step * t;
            self.velocity[index] =
                drift + Vec2::new(self.rng.range(-14.0, 14.0), self.rng.range(-14.0, 14.0));
            self.size[index] = self.rng.range(7.0, 18.0) * self.size_scale;
            self.life[index] = 0.0;
            self.life_span[index] = self.rng.range(0.5, 1.1);
            self.color[index] = PALETTE[(self.rng.unit() * 4.0) as usize & 3];
            self.mode[index] = Mode::Burst.min_placeholder(Mode::Trail);
        }
    }

    /// Переводит живые частицы в разлёт от `center`.
    pub fn burst(&mut self, center: Vec2, speed: f32) {
        for index in 0..self.capacity() {
            if self.mode[index] == Mode::Dead {
                continue;
            }
            let offset = self.position[index] - center;
            let length = offset.length().max(0.001);
            let direction = offset * (1.0 / length);
            let jitter = self.rng.range(0.6, 1.4);
            self.velocity[index] = direction * (speed * jitter);
            self.life[index] = 0.0;
            self.life_span[index] = f32::INFINITY;
            self.mode[index] = Mode::Burst;
        }
    }

    /// Назначает каждой живой частице постоянную точку на окружности и
    /// включает притяжение. Точки распределяются золотым углом — это даёт
    /// равномерность без случайных сгущений.
    pub fn reassemble(&mut self, center: Vec2, radius: f32) {
        const GOLDEN_ANGLE: f32 = 2.399_963_2;
        let mut ordinal = 0_u32;
        for index in 0..self.capacity() {
            if self.mode[index] == Mode::Dead {
                continue;
            }
            let angle = GOLDEN_ANGLE * ordinal as f32;
            // корень выравнивает плотность от центра к краю
            let spread = ((ordinal as f32 + 0.5) / self.alive().max(1) as f32).sqrt();
            self.target[index] = center + Vec2::new(angle.cos(), angle.sin()) * (radius * spread);
            self.mode[index] = Mode::Reassemble;
            ordinal += 1;
        }
    }

    /// Насколько плотно собрались частицы: 0 — разлетелись, 1 — на своих
    /// местах. По этому числу решается, когда подменить облако гладкой сферой.
    #[must_use]
    pub fn convergence(&self, tolerance: f32) -> f32 {
        let mut total = 0_u32;
        let mut settled = 0_u32;
        for index in 0..self.capacity() {
            if self.mode[index] != Mode::Reassemble {
                continue;
            }
            total += 1;
            if (self.target[index] - self.position[index]).length() <= tolerance {
                settled += 1;
            }
        }
        if total == 0 {
            return 1.0;
        }
        f32::from(settled as u16) / f32::from(total as u16)
    }

    /// Сдвигает частицы, живущие в мировых координатах.
    ///
    /// Нужно, когда окно переехало: хвост и разлетевшиеся частицы должны
    /// остаться на месте относительно экрана, а холст под ними уехал.
    ///
    /// Орбитальные частицы сюда **не входят намеренно**: они привязаны к телу
    /// персонажа, а тело внутри холста стоит на месте — двигается само окно.
    /// Если сдвигать и их, облако начнёт отставать при каждом прыжке.
    pub fn shift(&mut self, delta: Vec2) {
        if delta.x == 0.0 && delta.y == 0.0 {
            return;
        }
        for index in 0..self.capacity() {
            if matches!(self.mode[index], Mode::Trail | Mode::Burst) {
                self.position[index] = self.position[index] + delta;
            }
        }
    }

    /// Переносит якоря орбит в новое место — так облако следует за шариком,
    /// который свободно летает вокруг персонажа.
    pub fn move_home(&mut self, delta: Vec2) {
        if delta.x == 0.0 && delta.y == 0.0 {
            return;
        }
        for index in 0..self.capacity() {
            if self.mode[index] == Mode::Orbit {
                self.home[index] = self.home[index] + delta;
            }
        }
    }

    pub fn update(&mut self, dt: f32) {
        for index in 0..self.capacity() {
            match self.mode[index] {
                Mode::Dead => continue,
                Mode::Orbit => {
                    self.phase[index] += dt * self.spin[index];
                    let breathe = 1.0 + (self.phase[index] * 1.7).sin() * 0.05;
                    let radius = self.radius[index] * breathe;

                    // Точка на окружности в собственной плоскости орбиты,
                    // затем наклон плоскости (появляется координата z) и её
                    // разворот вокруг оси взгляда.
                    let (sin_theta, cos_theta) = self.phase[index].sin_cos();
                    let local_x = radius * cos_theta;
                    let local_y = radius * sin_theta;

                    let (sin_tilt, cos_tilt) = self.tilt[index].sin_cos();
                    let tilted_y = local_y * cos_tilt;
                    let z = local_y * sin_tilt;

                    let (sin_plane, cos_plane) = self.plane[index].sin_cos();
                    let screen_x = local_x * cos_plane - tilted_y * sin_plane;
                    let screen_y = local_x * sin_plane + tilted_y * cos_plane;

                    self.position[index] =
                        self.home[index] + self.anchor_offset + Vec2::new(screen_x, screen_y);
                    self.depth[index] = (z / radius.max(1.0)).clamp(-1.0, 1.0);
                }
                Mode::Trail => {
                    self.life[index] += dt;
                    if self.life[index] >= self.life_span[index] {
                        self.release(index);
                        continue;
                    }
                    self.velocity[index] = self.velocity[index] * 0.90;
                    self.position[index] = self.position[index] + self.velocity[index] * dt;
                }
                Mode::Burst => {
                    self.velocity[index] = self.velocity[index] * 0.965;
                    self.position[index] = self.position[index] + self.velocity[index] * dt;
                }
                Mode::Reassemble => {
                    let to_target = self.target[index] - self.position[index];
                    let distance = to_target.length();
                    // притяжение с ограничением ускорения и сильным гашением
                    // у цели — иначе частицы проскакивают и дрожат
                    let pull = (distance * 26.0).min(900.0);
                    let direction = if distance > 0.001 {
                        to_target * (1.0 / distance)
                    } else {
                        Vec2::default()
                    };
                    self.velocity[index] = self.velocity[index] * 0.86 + direction * (pull * dt);
                    self.position[index] = self.position[index] + self.velocity[index] * dt;
                }
            }
        }
    }

    /// Заполняет `out` видимыми частицами. Буфер переиспользуется вызывающим,
    /// поэтому в кадре ничего не выделяется.
    pub fn collect(&self, out: &mut Vec<Quad>) {
        out.clear();
        for index in 0..self.capacity() {
            if self.mode[index] == Mode::Dead {
                continue;
            }
            let mut alpha = if self.life_span[index].is_finite() {
                (1.0 - self.life[index] / self.life_span[index]).clamp(0.0, 1.0)
            } else {
                1.0
            };
            // Ближняя частица крупнее и ярче, дальняя — мельче и тусклее.
            // Это и создаёт объём, никакой настоящей 3D-геометрии не нужно.
            let depth = self.depth[index];
            alpha *= 0.45 + (depth + 1.0) * 0.275;
            out.push(Quad {
                position: self.position[index],
                size: (self.size[index] * (1.0 + depth * 0.45)).max(1.0),
                alpha: (alpha * self.brightness).clamp(0.0, 1.0),
                color: self.color[index],
                depth,
            });
        }
    }
}

impl Mode {
    /// Заглушка-хелпер: делает намерение явным в месте эмиссии хвоста.
    const fn min_placeholder(self, actual: Self) -> Self {
        let _ = self;
        actual
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, Particles, Vec2};

    #[test]
    fn trail_density_does_not_depend_on_frame_rate() {
        // один и тот же путь, пройденный за 1 и за 4 кадра, должен дать
        // одинаковое число частиц
        let count_for = |steps: u32| {
            let mut particles = Particles::with_capacity(512, 7, 1.0);
            particles.emit_trail(Vec2::new(0.0, 0.0), 10.0, Vec2::default());
            for step in 1..=steps {
                let x = 100.0 * step as f32 / steps as f32;
                particles.emit_trail(Vec2::new(x, 0.0), 10.0, Vec2::default());
            }
            particles.alive()
        };
        assert_eq!(count_for(1), count_for(4));
        assert_eq!(count_for(1), 10);
    }

    #[test]
    fn pool_never_exceeds_capacity_and_reuses_slots() {
        let mut particles = Particles::with_capacity(16, 3, 1.0);
        for step in 0..500 {
            particles.emit_trail(Vec2::new(step as f32 * 50.0, 0.0), 1.0, Vec2::default());
        }
        assert!(particles.alive() <= particles.capacity());

        for _ in 0..400 {
            particles.update(0.1);
        }
        assert_eq!(particles.alive(), 0, "хвост обязан полностью истечь");
    }

    #[test]
    fn reassembly_converges_and_stays_finite() {
        let mut particles = Particles::with_capacity(200, 11, 1.0);
        let centre = Vec2::new(0.0, 0.0);
        particles.seed_orbit(centre, 10.0, 30.0, 200);
        particles.burst(centre, 600.0);
        for _ in 0..30 {
            particles.update(1.0 / 60.0);
        }
        particles.reassemble(centre, 40.0);
        for _ in 0..600 {
            particles.update(1.0 / 60.0);
        }
        assert!(
            particles.convergence(3.0) > 0.95,
            "частицы должны сойтись к назначенным точкам"
        );
        let mut quads = Vec::new();
        particles.collect(&mut quads);
        assert!(
            quads
                .iter()
                .all(|q| q.position.x.is_finite() && q.position.y.is_finite()),
            "ни одной NaN-координаты"
        );
    }

    #[test]
    fn orbit_particles_stay_bounded_and_never_expire() {
        let mut particles = Particles::with_capacity(64, 5, 1.0);
        let centre = Vec2::new(50.0, 50.0);
        particles.seed_orbit(centre, 20.0, 40.0, 64);
        for _ in 0..240 {
            particles.update(1.0 / 60.0);
        }
        let mut quads = Vec::new();
        particles.collect(&mut quads);
        for quad in &quads {
            let distance = (quad.position - centre).length();
            assert!(
                distance <= 44.0,
                "частица вышла за внешний радиус: {distance}"
            );
            assert!(quad.position.x.is_finite() && quad.position.y.is_finite());
        }
        assert_eq!(particles.alive(), 64, "орбитальные частицы не истекают");
        assert!(matches!(Mode::Orbit, Mode::Orbit));
    }

    #[test]
    fn burst_and_reassembly_survive_many_cycles() {
        // 200 циклов «взрыв → сборка» не должны копить NaN или терять частицы
        let mut particles = Particles::with_capacity(120, 77, 1.0);
        let centre = Vec2::new(0.0, 0.0);
        particles.seed_orbit(centre, 20.0, 40.0, 60);
        for _ in 0..200 {
            particles.burst(centre, 500.0);
            for _ in 0..20 {
                particles.update(1.0 / 60.0);
            }
            particles.reassemble(centre, 30.0);
            for _ in 0..240 {
                particles.update(1.0 / 60.0);
            }
            assert!(particles.convergence(3.0) > 0.9, "облако не собралось");
            particles.reseed_orbit(centre, 20.0, 40.0, 60);
            assert_eq!(particles.alive(), 60, "число частиц должно сохраняться");
        }
        let mut quads = Vec::new();
        particles.collect(&mut quads);
        assert!(
            quads
                .iter()
                .all(|q| q.position.x.is_finite() && q.position.y.is_finite()),
            "после долгой работы появились нечисловые координаты"
        );
    }

    #[test]
    fn cloud_follows_the_body_when_it_leaves_the_ground() {
        let mut particles = Particles::with_capacity(32, 31, 1.0);
        let anchor = Vec2::new(100.0, 100.0);
        particles.seed_orbit(anchor, 10.0, 18.0, 16);
        particles.update(1.0 / 60.0);
        let mut before = Vec::new();
        particles.collect(&mut before);
        let low = before.iter().map(|q| q.position.y).sum::<f32>() / before.len() as f32;

        // тело подпрыгнуло на 40 пикселей
        particles.set_anchor_offset(Vec2::new(0.0, -40.0));
        particles.update(1.0 / 60.0);
        let mut after = Vec::new();
        particles.collect(&mut after);
        let high = after.iter().map(|q| q.position.y).sum::<f32>() / after.len() as f32;

        assert!(
            (low - high - 40.0).abs() < 2.0,
            "облако не поднялось вместе с телом: было {low}, стало {high}"
        );
    }

    #[test]
    fn window_shift_never_detaches_body_anchored_clouds() {
        let mut particles = Particles::with_capacity(32, 13, 1.0);
        let anchor = Vec2::new(100.0, 100.0);
        particles.seed_orbit(anchor, 10.0, 20.0, 16);
        for _ in 0..60 {
            particles.update(1.0 / 60.0);
            // окно уезжает каждый кадр, как при прыжке
            particles.shift(Vec2::new(-4.0, 0.0));
        }
        let mut quads = Vec::new();
        particles.collect(&mut quads);
        for quad in &quads {
            let distance = (quad.position - anchor).length();
            assert!(
                distance <= 22.0,
                "облако отстало от тела на {distance} пикселей"
            );
        }
    }

    #[test]
    fn separate_anchors_keep_their_own_clouds() {
        let mut particles = Particles::with_capacity(64, 9, 1.0);
        let left = Vec2::new(0.0, 0.0);
        let right = Vec2::new(400.0, 0.0);
        particles.seed_orbit(left, 10.0, 20.0, 12);
        particles.seed_orbit(right, 10.0, 20.0, 12);
        for _ in 0..120 {
            particles.update(1.0 / 60.0);
        }
        let mut quads = Vec::new();
        particles.collect(&mut quads);
        let near_left = quads.iter().filter(|q| q.position.x < 100.0).count();
        let near_right = quads.iter().filter(|q| q.position.x > 300.0).count();
        assert_eq!(near_left, 12, "левое облако должно остаться у своего якоря");
        assert_eq!(
            near_right, 12,
            "правое облако должно остаться у своего якоря"
        );
    }

    #[test]
    fn orbits_spin_both_ways_and_pass_in_front_and_behind() {
        let mut particles = Particles::with_capacity(240, 21, 1.0);
        let centre = Vec2::new(0.0, 0.0);
        particles.seed_orbit(centre, 30.0, 60.0, 240);
        particles.update(1.0 / 60.0);

        let mut quads = Vec::new();
        particles.collect(&mut quads);
        let behind = quads.iter().filter(|quad| quad.depth < -0.15).count();
        let front = quads.iter().filter(|quad| quad.depth > 0.15).count();
        assert!(behind > 15, "нет частиц позади персонажа: {behind}");
        assert!(front > 15, "нет частиц перед персонажем: {front}");

        // ближние обязаны быть крупнее дальних, иначе объём не читается
        let near = quads
            .iter()
            .filter(|quad| quad.depth > 0.5)
            .map(|quad| quad.size)
            .fold(0.0_f32, f32::max);
        let far = quads
            .iter()
            .filter(|quad| quad.depth < -0.5)
            .map(|quad| quad.size)
            .fold(f32::MAX, f32::min);
        assert!(
            near > far,
            "глубина не влияет на размер: {near} против {far}"
        );
    }
}
