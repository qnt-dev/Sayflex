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
    /// Штатный орбитальный квадрат, временно вплетённый в хвост переноса.
    DragTether,
    /// Плавно возвращается из хвоста на свою прежнюю орбиту.
    OrbitReturn,
    /// Отстаёт и гаснет — метеоритный хвост.
    Trail,
    /// Разлетается после взрыва.
    Burst,
    /// Летит к назначенной точке и там гасит скорость.
    Reassemble,
    /// Свободно движется внутри сжимающейся области и сталкивается с соседями.
    Tension,
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
    helix_phase: f32,
    drag_path: Vec<Vec2>,
    drag_catchup_debt: f32,
    drag_spacing: f32,
    drag_tangent: Vec2,
    tension_center: Vec2,
    tension_radius: f32,
    tension_pressure: f32,
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
            helix_phase: 0.0,
            drag_path: Vec::with_capacity(64),
            drag_catchup_debt: 0.0,
            drag_spacing: 6.5,
            drag_tangent: Vec2::new(1.0, 0.0),
            tension_center: Vec2::default(),
            tension_radius: 0.0,
            tension_pressure: 0.0,
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
            if !self.spawn_orbit(center, inner, outer) {
                return;
            }
        }
    }

    /// Adds one satellite to an existing orbit. Hosts use this to make the
    /// calm squares hatch around a newly rebuilt sphere one by one.
    pub fn spawn_orbit(&mut self, center: Vec2, inner: f32, outer: f32) -> bool {
        let Some(index) = self.allocate() else {
            return false;
        };
        let angle = self.rng.range(0.0, std::f32::consts::TAU);
        let radius = self.rng.range(inner, outer);
        self.position[index] = center;
        self.velocity[index] = Vec2::default();
        self.size[index] = self.rng.range(8.0, 24.0) * self.size_scale;
        self.life[index] = 0.0;
        self.life_span[index] = f32::INFINITY;
        self.phase[index] = angle;
        self.radius[index] = radius;
        let direction = if self.rng.unit() < 0.5 { -1.0 } else { 1.0 };
        self.spin[index] = direction * self.rng.range(0.18, 0.85);
        self.tilt[index] = self.rng.range(-1.35, 1.35);
        self.plane[index] = self.rng.range(0.0, std::f32::consts::TAU);
        self.depth[index] = 0.0;
        self.home[index] = center;
        self.color[index] = PALETTE[(self.rng.unit() * 4.0) as usize & 3];
        self.mode[index] = Mode::Orbit;
        true
    }

    /// Releases every slot without reallocating the particle pool.
    pub fn clear(&mut self) {
        for index in 0..self.capacity() {
            self.release(index);
        }
        self.tension_pressure = 0.0;
        self.tension_radius = 0.0;
        self.emit_debt = 0.0;
        self.last_origin = None;
        self.drag_path.clear();
        self.drag_catchup_debt = 0.0;
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
            self.phase[index] = 0.0;
            self.radius[index] = 0.0;
            self.spin[index] = 0.0;
            self.tilt[index] = 0.0;
            self.plane[index] = 0.0;
            self.depth[index] = 0.0;
            self.home[index] = origin;
            self.color[index] = PALETTE[(self.rng.unit() * 4.0) as usize & 3];
            self.mode[index] = Mode::Burst.min_placeholder(Mode::Trail);
        }
    }

    /// Переводит именно штатные орбитальные квадраты в режим хвоста.
    /// Новые частицы не создаются: пользователь видит, как знакомое облако
    /// отстаёт от сферы, а не как поверх него включается второй эффект.
    pub fn begin_drag_tail(&mut self, center: Vec2) {
        self.drag_path.clear();
        self.drag_path.push(center);
        self.emit_debt = 0.0;
        self.drag_catchup_debt = 0.0;
        for index in 0..self.capacity() {
            if matches!(self.mode[index], Mode::Orbit | Mode::OrbitReturn) {
                self.mode[index] = Mode::DragTether;
                self.target[index] = self.position[index];
                self.velocity[index] = Vec2::default();
                self.life[index] = 0.0;
                self.depth[index] = -0.45;
            }
        }
    }

    /// Продлевает две переплетённые нити вдоль реальной траектории окна.
    /// Точки пути добавляются по расстоянию, поэтому рисунок одинаков при
    /// любой частоте событий мыши и сохраняет окружности и резкие повороты.
    pub fn drag_tail(&mut self, origin: Vec2, movement: Vec2, spacing: f32) {
        let distance = movement.length();
        if distance <= f32::EPSILON || spacing <= f32::EPSILON {
            return;
        }
        let inverse = movement * -1.0;
        for point in &mut self.drag_path {
            *point = *point + inverse;
        }
        for index in 0..self.capacity() {
            if matches!(
                self.mode[index],
                Mode::DragTether | Mode::Trail | Mode::Burst
            ) {
                self.position[index] = self.position[index] + inverse;
                self.target[index] = self.target[index] + inverse;
            }
        }

        let tangent = movement * (1.0 / distance);
        self.drag_tangent = tangent;
        self.drag_spacing = spacing;
        let start = origin - movement;
        self.emit_debt += distance;
        while self.emit_debt >= spacing {
            self.emit_debt -= spacing;
            let travelled = distance - self.emit_debt;
            let point = start + tangent * travelled;
            if self.drag_path.len() == self.drag_path.capacity() {
                self.drag_path.remove(0);
            }
            self.drag_path.push(point);
            self.helix_phase =
                (self.helix_phase + spacing * 0.095).rem_euclid(std::f32::consts::TAU);
        }
        let catchup_fraction = self.drag_catchup_debt / self.drag_spacing;
        self.retarget_drag_tail(origin, tangent, catchup_fraction);
    }

    /// Advances the same pursuit while the pointer is stationary. The oldest
    /// part of the path is consumed continuously, so the tail begins shrinking
    /// on the first frame instead of waiting for an inactivity timeout.
    pub fn advance_drag_tail(&mut self, origin: Vec2, dt: f32, catchup_speed: f32) -> bool {
        if !self.mode.contains(&Mode::DragTether) {
            return false;
        }
        if self.drag_path.len() <= 1 {
            self.end_drag_tail(origin);
            return false;
        }

        self.drag_catchup_debt += dt.max(0.0) * catchup_speed.max(0.0);
        while self.drag_catchup_debt >= self.drag_spacing && self.drag_path.len() > 1 {
            self.drag_catchup_debt -= self.drag_spacing;
            self.drag_path.remove(0);
        }
        if self.drag_path.len() <= 1 {
            self.end_drag_tail(origin);
            return false;
        }
        let catchup_fraction = self.drag_catchup_debt / self.drag_spacing;
        self.retarget_drag_tail(origin, self.drag_tangent, catchup_fraction);
        true
    }

    fn retarget_drag_tail(&mut self, origin: Vec2, fallback_tangent: Vec2, catchup_fraction: f32) {
        let count = self
            .mode
            .iter()
            .filter(|mode_| **mode_ == Mode::DragTether)
            .count();
        if count == 0 {
            return;
        }
        let pairs = count.div_ceil(2).max(1);
        let mut ordinal = 0_usize;
        for index in 0..self.capacity() {
            if self.mode[index] != Mode::DragTether {
                continue;
            }
            let pair = ordinal / 2;
            let side = if ordinal.is_multiple_of(2) { -1.0 } else { 1.0 };
            let strand_position = (pair + 1) as f32 / (pairs + 1) as f32;
            let latest = self.drag_path.len().saturating_sub(1) as f32;
            let path_position = (latest * (1.0 - strand_position)
                + catchup_fraction.clamp(0.0, 1.0) * strand_position)
                .clamp(0.0, latest);
            let lower_index = path_position.floor() as usize;
            let upper_index = (lower_index + 1).min(self.drag_path.len() - 1);
            let interpolation = path_position - lower_index as f32;
            let lower = self.drag_path.get(lower_index).copied().unwrap_or(origin);
            let upper = self.drag_path.get(upper_index).copied().unwrap_or(lower);
            let point = lower + (upper - lower) * interpolation;
            let older = self
                .drag_path
                .get(lower_index.saturating_sub(1))
                .copied()
                .unwrap_or(point - fallback_tangent);
            let segment = point - older;
            let tangent = if segment.length() > 0.001 {
                segment * (1.0 / segment.length())
            } else {
                fallback_tangent
            };
            let normal = Vec2::new(-tangent.y, tangent.x);
            let phase = self.helix_phase - pair as f32 * 0.82;
            let amplitude = (6.0 + 3.5 * pair as f32 / pairs as f32) * self.size_scale.sqrt();
            self.target[index] = point + normal * (phase.sin() * amplitude * side);
            // Весь узор остаётся позади сферы, а лёгкая разница глубины
            // позволяет двум нитям визуально переплетаться между собой.
            self.depth[index] = -0.58 + phase.cos() * side * 0.16;
            ordinal += 1;
        }
    }

    /// После отпускания квадраты не телепортируются, а догоняют прежние
    /// орбиты из своих последних точек хвоста.
    pub fn end_drag_tail(&mut self, center: Vec2) {
        for index in 0..self.capacity() {
            if self.mode[index] == Mode::DragTether {
                self.mode[index] = Mode::OrbitReturn;
                self.home[index] = center;
                self.life[index] = 0.0;
            }
        }
        self.drag_path.clear();
        self.emit_debt = 0.0;
        self.drag_catchup_debt = 0.0;
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

    /// Adds small fragments cut from the sphere before applying the normal
    /// burst physics. The idle orbit only has twenty satellites, which is not
    /// dense enough to describe the sphere breaking apart and rebuilding.
    pub fn burst_with_fragments(
        &mut self,
        center: Vec2,
        source_radius: f32,
        speed: f32,
        fragment_count: usize,
    ) {
        let target_count = fragment_count.min(self.capacity());
        while self.alive() < target_count {
            let Some(index) = self.allocate() else { break };
            let angle = self.rng.range(0.0, std::f32::consts::TAU);
            let direction = Vec2::new(angle.cos(), angle.sin());
            let distance = source_radius * self.rng.unit().sqrt() * 0.92;
            self.position[index] = center + direction * distance;
            self.velocity[index] = Vec2::default();
            self.size[index] = self.rng.range(6.0, 19.0) * self.size_scale;
            self.life[index] = 0.0;
            self.life_span[index] = f32::INFINITY;
            self.phase[index] = angle;
            self.radius[index] = distance;
            self.spin[index] = 0.0;
            self.tilt[index] = 0.0;
            self.plane[index] = 0.0;
            self.depth[index] = self.rng.range(-0.45, 0.65);
            self.home[index] = center;
            self.color[index] = PALETTE[(self.rng.unit() * 4.0) as usize & 3];
            self.mode[index] = Mode::Burst;
        }
        self.burst(center, speed);
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

    /// Places all live fragments in one continuously shrinking chamber.
    ///
    /// No particle receives an individual slot or a second destination. The
    /// existing velocity is preserved; the wall and mutual collisions alone
    /// make impacts more frequent as the chamber becomes smaller.
    pub fn compress_in(&mut self, center: Vec2, radius: f32, pressure: f32) {
        self.tension_center = center;
        self.tension_radius = radius.max(0.0);
        self.tension_pressure = pressure.clamp(0.0, 1.0);
        for index in 0..self.capacity() {
            if self.mode[index] == Mode::Dead {
                continue;
            }
            self.mode[index] = Mode::Tension;
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
            if matches!(
                self.mode[index],
                Mode::Trail | Mode::Burst | Mode::DragTether
            ) {
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
            if matches!(self.mode[index], Mode::Orbit | Mode::OrbitReturn) {
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
                Mode::DragTether => {
                    // The path defines only the centre of each link. Every
                    // square keeps a small independent orbit around that
                    // centre, so holding the mouse still never freezes the
                    // companion into a dead line.
                    let direction = if self.spin[index] < 0.0 { -1.0 } else { 1.0 };
                    self.phase[index] += dt * direction * (1.75 + self.spin[index].abs() * 1.55);
                    let float_radius = 1.8 + self.radius[index].rem_euclid(7.0) * 0.19;
                    let animated_target = self.target[index]
                        + Vec2::new(self.phase[index].cos(), self.phase[index].sin())
                            * float_radius;
                    let response = 1.0 - (-dt * 17.0).exp();
                    self.position[index] =
                        self.position[index] + (animated_target - self.position[index]) * response;
                }
                Mode::OrbitReturn => {
                    self.phase[index] += dt * self.spin[index];
                    let target = orbit_position(
                        self.home[index] + self.anchor_offset,
                        self.phase[index],
                        self.radius[index],
                        self.tilt[index],
                        self.plane[index],
                    );
                    let response = 1.0 - (-dt * 10.5).exp();
                    self.position[index] =
                        self.position[index] + (target.0 - self.position[index]) * response;
                    self.depth[index] += (target.1 - self.depth[index]) * response;
                    self.life[index] += dt;
                    if self.life[index] >= 0.48 {
                        self.mode[index] = Mode::Orbit;
                    }
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
                Mode::Tension => {
                    let to_center = self.tension_center - self.position[index];
                    let distance = to_center.length();
                    let direction = if distance > 0.001 {
                        to_center * (1.0 / distance)
                    } else {
                        Vec2::default()
                    };
                    let pressure = self.tension_pressure;
                    let damping = (-dt * (0.7 + pressure * 0.9)).exp();
                    let pull = 180.0 + pressure * 520.0;
                    self.velocity[index] = self.velocity[index] * damping + direction * (pull * dt);
                    self.position[index] = self.position[index] + self.velocity[index] * dt;
                    self.depth[index] *= 1.0 - (dt * 4.0).min(1.0);
                }
            }
        }
        let collision_passes = 1 + (self.tension_pressure * 3.0).floor() as usize;
        for _ in 0..collision_passes {
            self.confine_tension_particles();
            self.resolve_tension_collisions(dt);
        }
    }

    fn confine_tension_particles(&mut self) {
        let pressure = self.tension_pressure;
        let restitution = 0.36 + pressure * 0.42;
        for index in 0..self.capacity() {
            if self.mode[index] != Mode::Tension {
                continue;
            }
            let offset = self.position[index] - self.tension_center;
            let distance = offset.length();
            let particle_radius = self.size[index] * (0.50 - pressure * 0.36);
            let limit = (self.tension_radius - particle_radius).max(0.35);
            if distance <= limit || distance <= 0.001 {
                continue;
            }
            let normal = offset * (1.0 / distance);
            self.position[index] = self.tension_center + normal * limit;
            let radial_speed =
                self.velocity[index].x * normal.x + self.velocity[index].y * normal.y;
            if radial_speed > 0.0 {
                self.velocity[index] =
                    self.velocity[index] - normal * (radial_speed * (1.0 + restitution));
            }
            // The inward-moving wall adds pressure without assigning a new
            // target. This keeps particles bouncing inside the same chamber.
            self.velocity[index] = self.velocity[index] - normal * (5.0 + pressure * 34.0);
        }
    }

    fn resolve_tension_collisions(&mut self, dt: f32) {
        let pressure = self.tension_pressure;
        let restitution = 0.42 + pressure * 0.44;
        let collision_scale = 1.0 - pressure * 0.76;
        for first in 0..self.capacity() {
            if self.mode[first] != Mode::Tension {
                continue;
            }
            for second in (first + 1)..self.capacity() {
                if self.mode[second] != Mode::Tension {
                    continue;
                }
                let delta = self.position[second] - self.position[first];
                let distance = delta.length();
                let minimum =
                    ((self.size[first] + self.size[second]) * 0.52 * collision_scale).max(0.45);
                if distance >= minimum {
                    continue;
                }
                let normal = if distance > 0.001 {
                    delta * (1.0 / distance)
                } else {
                    let angle = (first as f32 * 12.989_8 + second as f32 * 4.141_4).sin();
                    Vec2::new(angle.cos(), angle.sin())
                };
                let overlap = minimum - distance;
                let correction = normal * (overlap * 0.5);
                self.position[first] = self.position[first] - correction;
                self.position[second] = self.position[second] + correction;
                let relative = self.velocity[second] - self.velocity[first];
                let closing_speed = relative.x * normal.x + relative.y * normal.y;
                if closing_speed < 0.0 {
                    let impulse = normal * (-closing_speed * (1.0 + restitution) * 0.5);
                    self.velocity[first] = self.velocity[first] - impulse;
                    self.velocity[second] = self.velocity[second] + impulse;
                }
                let pressure_impulse = normal * (overlap * (24.0 + pressure * 120.0) * dt);
                self.velocity[first] = self.velocity[first] - pressure_impulse;
                self.velocity[second] = self.velocity[second] + pressure_impulse;
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
            let tension_scale = if self.mode[index] == Mode::Tension {
                1.0 - self.tension_pressure * 0.72
            } else {
                1.0
            };
            out.push(Quad {
                position: self.position[index],
                size: (self.size[index] * (1.0 + depth * 0.45) * tension_scale).max(0.7),
                alpha: (alpha * self.brightness).clamp(0.0, 1.0),
                color: self.color[index],
                depth,
            });
        }
    }
}

fn orbit_position(home: Vec2, phase: f32, radius: f32, tilt: f32, plane: f32) -> (Vec2, f32) {
    let breathe = 1.0 + (phase * 1.7).sin() * 0.05;
    let radius = radius * breathe;
    let (sin_theta, cos_theta) = phase.sin_cos();
    let local_x = radius * cos_theta;
    let local_y = radius * sin_theta;
    let (sin_tilt, cos_tilt) = tilt.sin_cos();
    let tilted_y = local_y * cos_tilt;
    let z = local_y * sin_tilt;
    let (sin_plane, cos_plane) = plane.sin_cos();
    let screen_x = local_x * cos_plane - tilted_y * sin_plane;
    let screen_y = local_x * sin_plane + tilted_y * cos_plane;
    (
        home + Vec2::new(screen_x, screen_y),
        (z / radius.max(1.0)).clamp(-1.0, 1.0),
    )
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
    fn drag_tail_reuses_orbit_squares_and_returns_them_without_a_jump() {
        let mut particles = Particles::with_capacity(64, 19, 1.0);
        let center = Vec2::new(100.0, 50.0);
        particles.seed_orbit(center, 20.0, 34.0, 20);
        particles.update(1.0 / 60.0);
        particles.begin_drag_tail(center);
        particles.drag_tail(center, Vec2::new(120.0, 0.0), 6.5);
        for _ in 0..30 {
            particles.update(1.0 / 60.0);
        }

        assert_eq!(particles.alive(), 20, "хвост не создаёт новые квадраты");
        assert_eq!(
            particles
                .mode
                .iter()
                .filter(|mode_| **mode_ == Mode::DragTether)
                .count(),
            20
        );
        let mut quads = Vec::new();
        particles.collect(&mut quads);
        assert!(quads.iter().all(|quad| quad.depth < 0.0));
        assert!(quads.iter().all(|quad| quad.position.x < center.x - 2.0));
        let path_before_catchup = particles.drag_path.len();
        assert!(particles.advance_drag_tail(center, 1.0 / 60.0, 240.0));
        assert!(
            particles.drag_catchup_debt > 0.0,
            "погоня должна начинаться на первом неподвижном кадре"
        );
        assert!(particles.drag_path.len() <= path_before_catchup);
        let held_positions = quads.iter().map(|quad| quad.position).collect::<Vec<_>>();
        for _ in 0..18 {
            particles.update(1.0 / 60.0);
        }
        particles.collect(&mut quads);
        assert!(
            quads
                .iter()
                .zip(&held_positions)
                .any(|(quad, before)| (quad.position - *before).length() > 0.7),
            "удерживаемый хвост обязан продолжать жить без движения мыши"
        );

        particles.end_drag_tail(center);
        for _ in 0..40 {
            particles.update(1.0 / 60.0);
        }
        assert_eq!(
            particles
                .mode
                .iter()
                .filter(|mode_| **mode_ == Mode::Orbit)
                .count(),
            20
        );
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
