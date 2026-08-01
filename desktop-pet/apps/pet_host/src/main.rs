#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Самостоятельное приложение настольного питомца.
//!
//! Своё окно через `CreateWindowExW` и `UpdateLayeredWindow`: показ и скрытие
//! не выполняются никогда, поэтому потеря extended-стилей при показе, из-за
//! которой в основном приложении мелькал белый кадр, здесь невозможна.
//! Композит на CPU, лицо рисуется процедурно, частицы живут в предвыделенном
//! пуле.
//!
//! Запуск:
//!   `cargo run -p pet_host --release`
//!   `cargo run -p pet_host --release -- --scale 0.28`
//!   `cargo run -p pet_host --release -- --stdin-control`
//!   `cargo run -p pet_host --release -- --dump sheet.png`

mod manifest;

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use pet_core::{Activity, FrameStats};
use pet_face::{EyeGlyph, FaceState};
use pet_hands::{Gesture, HandPose};
use pet_ipc::{Event, Events};
use pet_particles::{Particles, Quad, Rng, Vec2};
use pet_platform::Overlay;
use pet_render::{Canvas, Sprite, SpriteSource, WARP_ROWS, Warp};

use manifest::FaceManifest;

/// Пределы масштаба персонажа относительно исходного арта.
const MIN_SCALE: f32 = 0.10;
const MAX_SCALE: f32 = 0.45;
const DEFAULT_SCALE: f32 = 0.16;
/// Какую позу грузить. Стоящая умеет ходить, сидячая — только прыгать.
const MANIFEST: &str = "stand.json";

/// Запас вокруг сцены под облако частиц, в долях роста персонажа.
const MARGIN_FRACTION: f32 = 0.42;
/// Базовый радиус орбиты шарика, в долях ширины персонажа.
const ORB_ORBIT_BASE: f32 = 0.95;
/// Во сколько раз радиус может вырасти при смене цели.
const ORB_ORBIT_MAX: f32 = 1.35;
/// Сплюснутость орбиты по вертикали: шарик обходит персонажа, а не летает
/// по кругу.
const ORB_ORBIT_FLATTEN: f32 = 0.42;
/// Высота прыжка в долях роста персонажа.
const HOP_HEIGHT: f32 = 0.55;

/// Ближе этого расстояния до курсора персонаж считает, что дошёл.
const STOP_DISTANCE: f32 = 8.0;
/// За сколько ширин персонажа до цели он переходит с шага на бег.
const RUN_DISTANCE: f32 = 3.0;
/// Скорость шага и бега в ширинах персонажа за секунду.
const WALK_SPEED: f32 = 0.9;
const RUN_SPEED: f32 = 3.2;

/// Ширина кисти в долях ширины персонажа.
const HAND_WIDTH: f32 = 0.34;

const PET_PARTICLES: usize = 700;
const ORB_PARTICLES: usize = 320;
/// Частиц на каждый якорь — уши и хвост. Ровное кольцо вокруг тела читалось
/// как рамка, поэтому облако собрано в три небольшие группы.
const PARTICLES_PER_ANCHOR: usize = 9;
const ORB_ORBIT_COUNT: usize = 20;

const MOVING_THRESHOLD: f32 = 12.0;
const SLEEP_AFTER: Duration = Duration::from_secs(20);
const WORK_AREA_BOTTOM_MARGIN: i32 = 24;

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let scale = argument_value(&arguments, "--scale")
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(DEFAULT_SCALE)
        .clamp(MIN_SCALE, MAX_SCALE);
    let stdin_control = arguments
        .iter()
        .any(|argument| argument == "--stdin-control");

    let result = match (
        argument_value(&arguments, "--dump"),
        argument_value(&arguments, "--walk"),
        argument_value(&arguments, "--hands"),
    ) {
        (_, _, Some(path)) => dump_hands(Path::new(&path), scale),
        (_, Some(path), None) => dump_walk(Path::new(&path), scale),
        (Some(path), None, None) => dump_sheet(Path::new(&path), scale),
        (None, None, None) => run(scale, stdin_control),
    };
    if let Err(error) = result {
        eprintln!("pet_host: {error}");
        std::process::exit(1);
    }
}

fn argument_value(arguments: &[String], name: &str) -> Option<String> {
    let index = arguments.iter().position(|argument| argument == name)?;
    arguments.get(index + 1).cloned()
}

/// Всё, что зависит от текущего масштаба. Пересобирается при его изменении.
struct Rig {
    scale: f32,
    creature: Sprite,
    orb: Sprite,
    canvas_size: (u32, u32),
    creature_origin: Vec2,
    creature_centre: Vec2,
    orb_rest: Vec2,
    /// Левый верхний угол визора в координатах холста.
    visor_origin: Vec2,
    visor: pet_face::Visor,
    /// Уши и хвост в координатах холста: (центр, радиус).
    anchors: Vec<(Vec2, f32)>,
    /// Ноги: спрайт, пивот и левый верхний угол в координатах холста.
    legs: Vec<(Sprite, (f32, f32), Vec2)>,
    /// Манжеты карманов в координатах холста: левая, правая.
    pockets: Vec<Vec2>,
    /// Линия земли — по ней ложится тень.
    ground: f32,
    /// Профильные позы с отдельными конечностями. Правая и левая версии
    /// раздельны, чтобы в кадре не зеркалить сотни тысяч пикселей.
    profile_right: ProfileRig,
    profile_left: ProfileRig,
}

struct ProfileRig {
    creature: Sprite,
    creature_origin: Vec2,
    visor_origin: Vec2,
    visor: pet_face::Visor,
    legs: Vec<(Sprite, (f32, f32), Vec2)>,
    /// Дальняя рука рисуется за корпусом, ближняя — перед ним.
    arms: Vec<(Sprite, (f32, f32), Vec2)>,
}

/// Нога в полном разрешении со своей геометрией.
struct LegSource {
    sprite: SpriteSource,
    /// Бедро в координатах спрайта ноги.
    pivot: (f32, f32),
    /// Где нога стоит относительно корпуса, в координатах исходного арта.
    origin: (f32, f32),
}

struct Assets {
    creature: SpriteSource,
    orb: SpriteSource,
    legs: Vec<LegSource>,
    manifest: FaceManifest,
    profile_right: PoseAssets,
    profile_left: PoseAssets,
}

struct PoseAssets {
    creature: SpriteSource,
    legs: Vec<LegSource>,
    arms: Vec<LegSource>,
    manifest: FaceManifest,
}

impl PoseAssets {
    fn load(manifest_name: &str) -> Result<Self, String> {
        let manifest = FaceManifest::load(&asset_path(manifest_name))?;
        let creature = SpriteSource::load(&asset_path(&manifest.sprite), 10)
            .map_err(|error| format!("{}: {error}", manifest.sprite))?;
        let mut legs = Vec::new();
        for leg in &manifest.legs {
            let sprite = SpriteSource::load(&asset_path(&leg.sprite), 10)
                .map_err(|error| format!("{}: {error}", leg.sprite))?;
            let (crop_x, crop_y) = sprite.offset();
            legs.push(LegSource {
                pivot: (
                    leg.pivot.0 - leg.origin.0 - crop_x as f32,
                    leg.pivot.1 - leg.origin.1 - crop_y as f32,
                ),
                origin: leg.origin,
                sprite,
            });
        }
        let mut arms = Vec::new();
        for arm in &manifest.arms {
            let sprite = SpriteSource::load(&asset_path(&arm.sprite), 10)
                .map_err(|error| format!("{}: {error}", arm.sprite))?;
            let (crop_x, crop_y) = sprite.offset();
            arms.push(LegSource {
                pivot: (
                    arm.pivot.0 - arm.origin.0 - crop_x as f32,
                    arm.pivot.1 - arm.origin.1 - crop_y as f32,
                ),
                origin: arm.origin,
                sprite,
            });
        }
        Ok(Self {
            creature,
            legs,
            arms,
            manifest,
        })
    }

    fn build(&self, scale: f32, canvas_size: (u32, u32), ground: f32) -> ProfileRig {
        let creature = self.creature.scaled(scale);
        let (offset_x, offset_y) = self.creature.offset();
        let creature_origin = Vec2::new(
            (canvas_size.0 as f32 - creature.width() as f32) / 2.0,
            ground - (self.manifest.ground.unwrap_or(1090.0) - offset_y as f32) * scale,
        );
        let visor_origin = Vec2::new(
            creature_origin.x + (self.manifest.visor.x.saturating_sub(offset_x) as f32) * scale,
            creature_origin.y + (self.manifest.visor.y.saturating_sub(offset_y) as f32) * scale,
        );
        let legs = self
            .legs
            .iter()
            .map(|leg| {
                let sprite = leg.sprite.scaled(scale);
                let (crop_x, crop_y) = leg.sprite.offset();
                let position = Vec2::new(
                    creature_origin.x + (leg.origin.0 + crop_x as f32 - offset_x as f32) * scale,
                    creature_origin.y + (leg.origin.1 + crop_y as f32 - offset_y as f32) * scale,
                );
                (sprite, (leg.pivot.0 * scale, leg.pivot.1 * scale), position)
            })
            .collect();
        let arms = self
            .arms
            .iter()
            .map(|arm| {
                let sprite = arm.sprite.scaled(scale);
                let (crop_x, crop_y) = arm.sprite.offset();
                let position = Vec2::new(
                    creature_origin.x + (arm.origin.0 + crop_x as f32 - offset_x as f32) * scale,
                    creature_origin.y + (arm.origin.1 + crop_y as f32 - offset_y as f32) * scale,
                );
                (sprite, (arm.pivot.0 * scale, arm.pivot.1 * scale), position)
            })
            .collect();
        ProfileRig {
            creature,
            creature_origin,
            visor_origin,
            visor: self.manifest.visor_geometry(),
            legs,
            arms,
        }
    }
}

impl Assets {
    fn load() -> Result<Self, String> {
        let manifest = FaceManifest::load(&asset_path(MANIFEST))?;
        let creature = SpriteSource::load(&asset_path(&manifest.sprite), 10)
            .map_err(|error| format!("{}: {error}", manifest.sprite))?;
        let orb = SpriteSource::load(&asset_path("orb.png"), 10)
            .map_err(|error| format!("orb.png: {error}"))?;
        let mut legs = Vec::new();
        for leg in &manifest.legs {
            let sprite = SpriteSource::load(&asset_path(&leg.sprite), 10)
                .map_err(|error| format!("{}: {error}", leg.sprite))?;
            // Пивот задан в координатах целого арта, а спрайт ноги обрезан —
            // переводим в его собственные координаты, с поправкой на обрезку.
            let (crop_x, crop_y) = sprite.offset();
            legs.push(LegSource {
                pivot: (
                    leg.pivot.0 - leg.origin.0 - crop_x as f32,
                    leg.pivot.1 - leg.origin.1 - crop_y as f32,
                ),
                origin: leg.origin,
                sprite,
            });
        }
        Ok(Self {
            creature,
            orb,
            legs,
            manifest,
            profile_right: PoseAssets::load("profile_motion_right.json")?,
            profile_left: PoseAssets::load("profile_motion_left.json")?,
        })
    }

    /// Строит сцену под заданный масштаб. Спрайты каждый раз уменьшаются от
    /// оригинала, поэтому многократная смена размера не копит потери.
    fn build(&self, scale: f32) -> Rig {
        let creature = self.creature.scaled(scale);
        let orb = self.orb.scaled(scale);

        let margin = (creature.height() as f32 * MARGIN_FRACTION).round() as u32;
        // Шарик кружит вокруг персонажа, поэтому холст должен вмещать его
        // орбиту целиком — иначе он срезается краем окна.
        let orbit_max = creature.width() as f32 * ORB_ORBIT_BASE * ORB_ORBIT_MAX;
        let half_width =
            creature.width() as f32 / 2.0 + orbit_max + orb.width() as f32 / 2.0 + margin as f32;
        let half_height = creature.height() as f32 / 2.0
            + (orbit_max * ORB_ORBIT_FLATTEN + orb.height() as f32 / 2.0)
                .max(creature.height() as f32 * HOP_HEIGHT + margin as f32);
        let canvas_size = (
            (half_width * 2.0).round() as u32,
            (half_height * 2.0).round() as u32,
        );

        let creature_origin = Vec2::new(
            (canvas_size.0 as f32 - creature.width() as f32) / 2.0,
            (canvas_size.1 as f32 - creature.height() as f32) / 2.0,
        );
        let creature_centre = Vec2::new(
            creature_origin.x + creature.width() as f32 / 2.0,
            creature_origin.y + creature.height() as f32 / 2.0,
        );
        let orb_rest = creature_centre + Vec2::new(creature.width() as f32 * ORB_ORBIT_BASE, 0.0);

        // Координаты визора в манифесте даны в пикселях исходного файла, а
        // источник мог срезать пустые поля — вычитаем смещение обрезки.
        let (offset_x, offset_y) = self.creature.offset();
        let visor_origin = Vec2::new(
            creature_origin.x + (self.manifest.visor.x.saturating_sub(offset_x) as f32) * scale,
            creature_origin.y + (self.manifest.visor.y.saturating_sub(offset_y) as f32) * scale,
        );

        let anchors = self
            .manifest
            .particle_anchors
            .iter()
            .map(|anchor| {
                (
                    Vec2::new(
                        creature_origin.x + (anchor.x - offset_x as f32) * scale,
                        creature_origin.y + (anchor.y - offset_y as f32) * scale,
                    ),
                    anchor.radius * scale,
                )
            })
            .collect();
        let legs = self
            .legs
            .iter()
            .map(|leg| {
                let sprite = leg.sprite.scaled(scale);
                let (crop_x, crop_y) = leg.sprite.offset();
                let position = Vec2::new(
                    creature_origin.x + (leg.origin.0 + crop_x as f32 - offset_x as f32) * scale,
                    creature_origin.y + (leg.origin.1 + crop_y as f32 - offset_y as f32) * scale,
                );
                (sprite, (leg.pivot.0 * scale, leg.pivot.1 * scale), position)
            })
            .collect();
        let pockets = self
            .manifest
            .pockets
            .iter()
            .map(|(x, y)| {
                Vec2::new(
                    creature_origin.x + (x - offset_x as f32) * scale,
                    creature_origin.y + (y - offset_y as f32) * scale,
                )
            })
            .collect();
        let ground = self.manifest.ground.map_or_else(
            || creature_origin.y + creature.height() as f32 * 0.97,
            |line| creature_origin.y + (line - offset_y as f32) * scale,
        );

        let profile_right = self.profile_right.build(scale, canvas_size, ground);
        let profile_left = self.profile_left.build(scale, canvas_size, ground);

        Rig {
            scale,
            creature,
            orb,
            canvas_size,
            creature_origin,
            creature_centre,
            orb_rest,
            visor_origin,
            visor: self.manifest.visor_geometry(),
            anchors,
            legs,
            pockets,
            ground,
            profile_right,
            profile_left,
        }
    }
}

/// Во что персонаж вовлечён прямо сейчас. Обычный самостоятельный запуск
/// оставляет значения в покое; явная интеграция может прислать нейтральные
/// события через `--stdin-control`.
#[derive(Clone, Copy, Default)]
struct Engagement {
    /// Насколько активно пользователь печатает, 0…1 с затуханием.
    typing: f32,
    /// Уровень микрофона во время диктовки.
    voice: f32,
    listening: bool,
    /// Короткая вспышка радости или досады после события вставки.
    cheer: f32,
    upset: f32,
}

impl Engagement {
    /// Затухание: без новых событий персонаж возвращается к покою сам.
    fn decay(&mut self, dt: f32) {
        self.typing = (self.typing - dt * 0.85).max(0.0);
        self.voice = (self.voice - dt * 2.5).max(0.0);
        self.cheer = (self.cheer - dt * 0.7).max(0.0);
        self.upset = (self.upset - dt * 0.7).max(0.0);
    }

    fn apply(&mut self, event: Event) {
        match event {
            // Каждое нажатие подбрасывает уровень, а не задаёт его: так
            // получается ощущение ритма печати, а не ступенька.
            Event::Activity(intensity) => {
                self.typing = (self.typing + 0.22 + intensity * 0.25).min(1.0);
            }
            Event::Sound(level) => self.voice = self.voice.max(level),
            Event::FocusStarted => self.listening = true,
            Event::FocusFinished => self.listening = false,
            Event::Celebrate => self.cheer = 1.0,
            Event::Upset => self.upset = 1.0,
            Event::Shutdown => {}
        }
    }
}

/// Мимика и её расписание.
struct Mood {
    face: FaceState,
    /// Секунды до следующего моргания.
    blink_in: f32,
    /// Сколько ещё держать глаза закрытыми.
    blink_hold: f32,
    /// Секунды до следующего перескока крестика.
    swap_in: f32,
    cross_on_left: bool,
    /// Чтобы досада перекинула крестик один раз, а не каждый кадр.
    upset_shown: bool,
    rng: Rng,
}

impl Mood {
    fn new(seed: u64) -> Self {
        Self {
            face: FaceState::default(),
            blink_in: 2.4,
            blink_hold: 0.0,
            swap_in: 7.0,
            cross_on_left: false,
            upset_shown: false,
            rng: Rng::new(seed),
        }
    }

    /// `gaze` — направление на курсор, компоненты от −1 до 1.
    fn update(&mut self, dt: f32, gaze: (f32, f32), activity: Activity, mind: &Engagement) {
        self.face.left.gaze = gaze;
        self.face.right.gaze = gaze;

        // Во сне лицо гаснет и глаза закрываются. Отдельного состояния для
        // этого не нужно — хватает тех же параметров.
        if activity == Activity::Asleep {
            self.face.glow = (self.face.glow - dt * 1.2).max(0.25);
            self.face.left.morph_to(EyeGlyph::Closed);
            self.face.right.morph_to(EyeGlyph::Closed);
            self.face.mouth_curve = 0.15;
            self.face.advance(dt);
            return;
        }
        // Печать и речь подсвечивают лицо: чем активнее пользователь, тем
        // ярче светится визор.
        let excitement = mind.typing.max(mind.voice).max(mind.cheer);
        let target_glow = (0.9 + excitement * 0.45).min(1.35);
        self.face.glow += (target_glow - self.face.glow) * (dt * 3.0).min(1.0);

        if self.blink_hold > 0.0 {
            self.blink_hold -= dt;
            if self.blink_hold <= 0.0 {
                self.restore_eyes();
            }
        } else {
            self.blink_in -= dt;
            if self.blink_in <= 0.0 {
                self.blink_in = self.rng.range(2.6, 6.5);
                self.blink_hold = 0.11;
                self.face.left.morph_to(EyeGlyph::Closed);
                self.face.right.morph_to(EyeGlyph::Closed);
            }
        }

        self.swap_in -= dt;
        if self.swap_in <= 0.0 && self.blink_hold <= 0.0 {
            self.swap_in = self.rng.range(6.0, 13.0);
            self.cross_on_left = !self.cross_on_left;
            self.face.set_cross_on_left(self.cross_on_left);
        }

        // Досада перекидывает крестик и хмурит глаза, радость — раскрывает.
        if mind.upset > 0.75 && !self.upset_shown {
            self.upset_shown = true;
            self.cross_on_left = !self.cross_on_left;
            self.face.set_cross_on_left(self.cross_on_left);
        } else if mind.upset < 0.2 {
            self.upset_shown = false;
        }

        // Быстрое движение и вовлечённость раскрывают глаза шире.
        let target_open = if activity == Activity::Active {
            1.3
        } else {
            1.0 + mind.typing * 0.18 + mind.cheer * 0.3
        };
        let open = self.face.left.openness + (target_open - self.face.left.openness) * dt * 4.0;
        self.face.left.openness = open;
        self.face.right.openness = open;
        self.face.mouth_curve = if mind.upset > 0.3 {
            -0.7 * mind.upset
        } else if activity == Activity::Active {
            0.55
        } else {
            1.0
        };
        // Во время диктовки рот приоткрывается в такт голосу.
        self.face.mouth_open = if mind.listening {
            (mind.voice * 0.9).min(0.9)
        } else {
            0.0
        };

        self.face.advance(dt);
    }

    fn restore_eyes(&mut self) {
        if self.cross_on_left {
            self.face.left.morph_to(EyeGlyph::Cross);
            self.face.right.morph_to(EyeGlyph::Arc);
        } else {
            self.face.right.morph_to(EyeGlyph::Cross);
            self.face.left.morph_to(EyeGlyph::Arc);
        }
    }
}

/// Прыжок.
///
/// Персонаж на арте **сидит** — ходить он не может, и попытка навесить на него
/// шаги давала «полёт сидя». Сидящее существо перемещается прыжками, поэтому
/// здесь фаза приседания, баллистическая дуга и приземление со сплющиванием.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HopPhase {
    /// Сидит на месте, дышит.
    Resting,
    /// Приседает перед толчком.
    Crouch,
    /// В воздухе.
    Air,
    /// Гасит удар после касания.
    Landing,
}

struct Hopper {
    phase: HopPhase,
    timer: f32,
    /// Подъём над землёй в пикселях холста.
    height: f32,
    /// Сплющивание: 1 — обычная форма, меньше — присел.
    squash: f32,
    /// Длительность текущей фазы полёта.
    air_time: f32,
    /// Дыхание в покое.
    breath: f32,
    peak: f32,
    rng: Rng,
}

impl Hopper {
    const CROUCH_TIME: f32 = 0.13;
    const LAND_TIME: f32 = 0.17;

    fn new(seed: u64) -> Self {
        Self {
            phase: HopPhase::Resting,
            timer: 0.0,
            height: 0.0,
            squash: 1.0,
            air_time: 0.0,
            breath: 0.0,
            peak: 0.0,
            rng: Rng::new(seed),
        }
    }

    #[must_use]
    const fn airborne(&self) -> bool {
        matches!(self.phase, HopPhase::Air)
    }

    /// `excited` — есть ли повод подпрыгнуть прямо сейчас.
    ///
    /// Прыжок вертикальный, на месте: догонять курсор персонаж ходит и бегает,
    /// а подпрыгивает от радости. Именно поэтому здесь нет горизонтальной
    /// скорости — раньше она и превращала прыжок в способ передвижения.
    fn update(&mut self, dt: f32, excited: bool, peak: f32) {
        self.timer -= dt;
        self.breath += dt * 1.6;

        match self.phase {
            HopPhase::Resting => {
                self.height = self.breath.sin() * peak * 0.035;
                self.squash += (1.0 - self.squash) * (dt * 8.0).min(1.0);
                if self.timer <= 0.0 && excited {
                    self.phase = HopPhase::Crouch;
                    self.timer = Self::CROUCH_TIME;
                    self.air_time = self.rng.range(0.30, 0.42);
                    self.peak = peak * self.rng.range(0.82, 1.15);
                }
            }
            HopPhase::Crouch => {
                let progress = 1.0 - (self.timer / Self::CROUCH_TIME).clamp(0.0, 1.0);
                self.squash = 1.0 - progress * 0.22;
                self.height = 0.0;
                if self.timer <= 0.0 {
                    self.phase = HopPhase::Air;
                    self.timer = self.air_time;
                }
            }
            HopPhase::Air => {
                let progress = 1.0 - (self.timer / self.air_time).clamp(0.0, 1.0);
                // парабола: 0 на взлёте и приземлении, максимум посередине
                self.height = 4.0 * self.peak * progress * (1.0 - progress);
                // вытягивается в полёте, сильнее у вершины
                self.squash = 1.0 + (1.0 - (progress - 0.5).abs() * 2.0) * 0.10;
                if self.timer <= 0.0 {
                    self.phase = HopPhase::Landing;
                    self.timer = Self::LAND_TIME;
                    self.height = 0.0;
                }
            }
            HopPhase::Landing => {
                let progress = 1.0 - (self.timer / Self::LAND_TIME).clamp(0.0, 1.0);
                // резко сплющился и упруго вернулся
                self.squash = 1.0 - (1.0 - progress) * 0.26;
                self.height = 0.0;
                if self.timer <= 0.0 {
                    self.phase = HopPhase::Resting;
                    self.timer = self.rng.range(0.25, 0.6);
                }
            }
        }
    }
}

/// Свободное движение шарика вокруг персонажа.
///
/// Шарик — самостоятельный объект: он кружит по меняющейся орбите, иногда
/// ускоряется и обгоняет персонажа. Постоянное смещение вправо выглядело как
/// жёсткая сцепка.
struct OrbMotion {
    angle: f32,
    speed: f32,
    target_speed: f32,
    radius: f32,
    target_radius: f32,
    height: f32,
    target_height: f32,
    retarget_in: f32,
    rng: Rng,
}

impl OrbMotion {
    fn new(seed: u64, radius: f32) -> Self {
        Self {
            angle: -0.6,
            speed: 0.5,
            target_speed: 0.5,
            radius,
            target_radius: radius,
            height: 0.0,
            target_height: 0.0,
            retarget_in: 2.0,
            rng: Rng::new(seed),
        }
    }

    fn update(&mut self, dt: f32, base_radius: f32) -> Vec2 {
        self.retarget_in -= dt;
        if self.retarget_in <= 0.0 {
            self.retarget_in = self.rng.range(1.6, 4.5);
            // изредка — рывок с обгоном, иначе спокойный дрейф
            self.target_speed = if self.rng.unit() < 0.28 {
                self.rng.range(1.9, 3.4) * if self.rng.unit() < 0.5 { -1.0 } else { 1.0 }
            } else {
                self.rng.range(-0.7, 0.7)
            };
            self.target_radius = base_radius * self.rng.range(0.72, 1.35);
            self.target_height = self.rng.range(-0.42, 0.12);
        }
        self.speed += (self.target_speed - self.speed) * (dt * 2.2).min(1.0);
        self.radius += (self.target_radius - self.radius) * (dt * 1.6).min(1.0);
        self.height += (self.target_height - self.height) * (dt * 1.6).min(1.0);
        self.angle += self.speed * dt;

        // орбита сплюснута по вертикали: шарик ходит вокруг, а не по кругу
        Vec2::new(
            self.angle.cos() * self.radius,
            self.angle.sin() * self.radius * 0.42 + self.height * self.radius,
        )
    }
}

/// Аллюр. Чем дальше цель, тем размашистее движение.
///
/// Прыжка в этом перечислении нет намеренно: прыжок — эмоция, а не способ
/// передвижения. Раньше дальние дистанции персонаж покрывал скачками, и это
/// читалось как заяц, а не как живой зверёк.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Gait {
    Stand,
    Walk,
    Run,
}

impl Gait {
    /// Выбор аллюра по оставшемуся расстоянию, в ширинах персонажа.
    fn choose(distance: f32, width: f32) -> Self {
        if distance < STOP_DISTANCE {
            Self::Stand
        } else if distance < width * RUN_DISTANCE {
            Self::Walk
        } else {
            Self::Run
        }
    }

    /// Скорость в пикселях за секунду.
    fn speed(self, width: f32) -> f32 {
        match self {
            Self::Stand => 0.0,
            Self::Walk => width * WALK_SPEED,
            Self::Run => width * RUN_SPEED,
        }
    }
}

/// Цикл ходьбы и бега.
///
/// Фаза считается **по пройденному пути**, а не по времени: шаги совпадают со
/// скоростью, и при любой частоте кадров нога опускается там же, где и должна.
/// Ноги вращаются вокруг бедра в противофазе, корпус слегка приподнимается на
/// каждом шаге и покачивается на опорную ногу.
///
/// Бег — не отдельный цикл, а тот же с растянутым шагом, большей амплитудой ног
/// и наклоном корпуса вперёд. Отдельный цикл потребовал бы второго набора
/// спрайтов, а разница читается и так.
#[derive(Default)]
struct Walk {
    phase: f32,
    /// Насколько цикл активен: 0 — стоит, 1 — идёт.
    blend: f32,
    /// Насколько это бег, а не шаг: 0…1. Плавный, чтобы разгон был виден.
    run: f32,
    /// Наклон корпуса по ходу движения, -1…1.
    lean: f32,
}

impl Walk {
    /// `travelled` — путь за кадр, `stride` — длина шага в пикселях,
    /// `direction` — знак движения по горизонтали.
    fn update(&mut self, dt: f32, travelled: f32, stride: f32, gait: Gait, direction: f32) {
        // На бегу шаг длиннее: иначе ноги мельтешат, как при перемотке.
        let stride = stride.max(1.0) * (1.0 + self.run * 0.6);
        if travelled > 0.01 {
            self.phase += travelled / stride * std::f32::consts::PI;
        }
        let moving = gait != Gait::Stand;
        let target = if moving { 1.0 } else { 0.0 };
        self.blend += (target - self.blend) * (dt * 7.0).min(1.0);

        let run_target = if gait == Gait::Run { 1.0 } else { 0.0 };
        self.run += (run_target - self.run) * (dt * 4.0).min(1.0);

        // Наклон вперёд — то, что отличает бег от быстрой ходьбы сильнее всего.
        let lean_target = if moving {
            direction * (0.30 + self.run * 0.70)
        } else {
            0.0
        };
        self.lean += (lean_target - self.lean) * (dt * 5.0).min(1.0);

        if self.blend < 0.01 && !moving {
            // остановились — возвращаем ноги в исходное положение
            self.phase = 0.0;
        }
    }

    /// Углы поворота ног: первая — левая, вторая — правая.
    fn leg_angles(&self, amplitude: f32) -> (f32, f32) {
        let swing = self.phase.sin() * amplitude * self.blend * (1.0 + self.run * 0.85);
        (swing, -swing)
    }

    /// Подъём корпуса: на каждом шаге тело слегка выталкивается вверх.
    fn body_lift(&self, amount: f32) -> f32 {
        (self.phase * 2.0).sin().abs() * amount * self.blend * (1.0 + self.run * 1.3)
    }

    /// Покачивание в сторону опорной ноги.
    fn body_sway(&self, amount: f32) -> f32 {
        self.phase.sin() * amount * self.blend
    }

    /// Наклон корпуса по ходу движения, в долях размера спрайта.
    fn body_lean(&self) -> f32 {
        self.lean
    }
}

/// Разворот между фронтальной позой и профилем.
///
/// При смене направления персонаж сначала возвращается к фасу, затем выходит
/// в противоположный профиль. Поэтому спрайт никогда не зеркалится резким
/// скачком посреди шага.
struct Orientation {
    profile: f32,
    facing: f32,
}

impl Default for Orientation {
    fn default() -> Self {
        Self {
            profile: 0.0,
            facing: 1.0,
        }
    }
}

impl Orientation {
    fn update(&mut self, dt: f32, gait: Gait, direction: f32) {
        let moving = gait != Gait::Stand && direction.abs() > 0.0;
        let requested = if direction < 0.0 { -1.0 } else { 1.0 };
        let changing_side = moving && requested != self.facing && self.profile > 0.04;
        let target = if moving && !changing_side { 1.0 } else { 0.0 };
        let speed = if target > self.profile { 5.5 } else { 7.5 };
        self.profile += (target - self.profile) * (dt * speed).min(1.0);
        if moving && self.profile <= 0.04 {
            self.facing = requested;
        }
        if self.profile < 0.001 {
            self.profile = 0.0;
        } else if self.profile > 0.999 {
            self.profile = 1.0;
        }
    }

    fn profile_rig<'a>(&self, rig: &'a Rig) -> &'a ProfileRig {
        if self.facing < 0.0 {
            &rig.profile_left
        } else {
            &rig.profile_right
        }
    }
}

/// Живая пластика тела.
///
/// Персонаж — одна плоская картинка, поэтому уши, хвост и корпус двигаются
/// деформацией сетки, а не отдельными слоями. Так удаётся оживить его, не
/// разрезая арт и не дорисовывая скрытое.
#[derive(Default)]
struct Body {
    /// Фаза дыхания.
    breath: f32,
    /// Фаза виляния хвостом.
    tail: f32,
    /// Прижатие ушей: 0 — торчком, 1 — прижаты.
    ear_droop: f32,
    /// Амплитуда виляния: растёт от радости и печати.
    wag: f32,
}

impl Body {
    /// `squash` приходит от прыжка: при приземлении тело сплющивается.
    fn update(&mut self, dt: f32, mind: &Engagement, activity: Activity, squash: f32) {
        self.breath += dt * (1.1 + mind.typing * 0.9);
        self.tail += dt * (2.0 + self.wag * 6.0);

        // Уши прижимаются во сне и от досады, торчат при внимании.
        let target_droop = if activity == Activity::Asleep {
            1.0
        } else {
            mind.upset * 0.75
        };
        self.ear_droop += (target_droop - self.ear_droop) * (dt * 3.5).min(1.0);

        let target_wag = (mind.cheer * 0.9 + mind.typing * 0.5).min(1.0);
        self.wag += (target_wag - self.wag) * (dt * 2.5).min(1.0);

        let _ = squash;
    }

    /// Собирает сетку деформации на текущий кадр.
    fn warp(&self, squash: f32) -> Warp {
        let mut warp = Warp::identity();

        // Дыхание: корпус слегка сжимается по вертикали, макушка ходит вверх
        // и вниз. Выборка берётся ниже, поэтому знак положительный.
        let breathe = self.breath.sin() * 0.008;
        warp.add_row(0, 0.0, breathe);
        warp.add_row(1, 0.0, breathe * 0.6);

        // Приземление и присед: тело шире и ниже. squash < 1 — сплющен.
        let flatten = 1.0 - squash;
        warp.add_row(WARP_ROWS - 1, 0.0, -flatten * 0.05);
        warp.add(0, WARP_ROWS - 1, flatten * 0.05, 0.0);
        warp.add(3, WARP_ROWS - 1, -flatten * 0.05, 0.0);

        // Уши: верхний ряд наклоняется наружу и вниз. Внешние узлы тянутся
        // сильнее внутренних, поэтому уши расходятся, а не съезжают целиком.
        let droop = self.ear_droop;
        warp.add(0, 0, -droop * 0.09, -droop * 0.05);
        warp.add(3, 0, droop * 0.09, -droop * 0.05);
        warp.add(1, 0, -droop * 0.03, -droop * 0.02);
        warp.add(2, 0, droop * 0.03, -droop * 0.02);

        // Хвост живёт в правой нижней четверти: качается по горизонтали.
        let swing = self.tail.sin() * self.wag * 0.05;
        warp.add(3, 3, swing, 0.0);
        warp.add(3, WARP_ROWS - 1, swing * 0.7, 0.0);

        warp
    }
}

/// Кисти и их расписание.
///
/// Жест — короткое событие: кисть выходит из кармана, показывает и убирается.
/// Держать её снаружи постоянно нельзя — пропадёт ощущение, что она
/// материализовалась. При ходьбе и беге руки остаются в карманах: размахивать
/// голограммами на бегу выглядело бы неряшливо.
struct Hands {
    left: HandPose,
    right: HandPose,
    /// Сколько ещё держать текущий жест.
    hold: f32,
    /// Фаза хлопка: ладони сходятся и расходятся.
    clap_phase: f32,
    clapping: bool,
    /// Чтобы одно событие вызвало жест один раз, а не каждый кадр.
    cheer_shown: bool,
    upset_shown: bool,
    /// Секунды до следующего жеста от скуки.
    idle_in: f32,
    rng: Rng,
}

impl Hands {
    fn new(seed: u64) -> Self {
        Self {
            left: HandPose::new(),
            right: HandPose::new(),
            hold: 0.0,
            clap_phase: 0.0,
            clapping: false,
            cheer_shown: false,
            upset_shown: false,
            idle_in: 14.0,
            rng: Rng::new(seed),
        }
    }

    fn show(&mut self, left: Gesture, right: Gesture, seconds: f32) {
        self.left.blend_to(left);
        self.right.blend_to(right);
        self.hold = seconds;
    }

    fn update(&mut self, dt: f32, mind: &Engagement, gait: Gait) {
        self.hold -= dt;
        self.idle_in -= dt;
        if self.clapping {
            self.clap_phase += dt * 9.0;
        }

        // Флаги сбрасываются на спаде, иначе одна удачная вставка показала бы
        // жест только один раз за всю сессию.
        if mind.cheer < 0.3 {
            self.cheer_shown = false;
        }
        if mind.upset < 0.3 {
            self.upset_shown = false;
        }

        let busy = self.hold > 0.0;
        let moving = gait != Gait::Stand;

        if !busy && !moving {
            if mind.cheer > 0.8 && !self.cheer_shown {
                // Получилось — хлопает в ладоши.
                self.cheer_shown = true;
                self.clapping = true;
                self.clap_phase = 0.0;
                self.show(Gesture::Open, Gesture::Open, 1.7);
            } else if mind.upset > 0.8 && !self.upset_shown {
                // Не получилось вставить — показывает, что думает об этом.
                self.upset_shown = true;
                self.clapping = false;
                self.show(Gesture::Hidden, Gesture::MiddleFinger, 1.5);
            } else if self.idle_in <= 0.0 {
                self.idle_in = self.rng.range(18.0, 40.0);
                self.clapping = false;
                let gesture = match self.rng.unit() {
                    value if value < 0.45 => Gesture::ThumbsUp,
                    value if value < 0.80 => Gesture::Peace,
                    _ => Gesture::Point,
                };
                self.show(Gesture::Hidden, gesture, 1.6);
            }
        }

        if self.hold <= 0.0 {
            self.clapping = false;
            self.left.blend_to(Gesture::Hidden);
            self.right.blend_to(Gesture::Hidden);
        }
        // Ходьба прячет руки: жест обрывается, а не доигрывает на бегу.
        if moving {
            self.hold = self.hold.min(0.0);
        }

        self.left.update(dt);
        self.right.update(dt);
    }

    /// Насколько ладони сведены к центру при хлопке, 0…1.
    fn clap_reach(&self) -> f32 {
        if !self.clapping {
            return 0.0;
        }
        // Резкое схождение и плавный отход: удар должен быть слышен глазом.
        let swing = self.clap_phase.sin();
        swing.max(0.0).powf(0.6)
    }

    fn idle(&self) -> bool {
        self.left.is_hidden() && self.right.is_hidden()
    }
}

/// Жизненный цикл шарика: он периодически рассыпается на квадратики и
/// собирается обратно теми же самыми частицами.
///
/// Сборка работает только потому, что индекс в пуле служит устойчивым
/// идентификатором: каждой частице назначается своя точка на окружности, и она
/// летит именно туда. Без этого облако скучилось бы в центре.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OrbPhase {
    Whole,
    Scattered,
    Gathering,
}

struct OrbLife {
    phase: OrbPhase,
    timer: f32,
    /// Насколько сфера проявлена: 0 — только частицы, 1 — цельный шарик.
    solidity: f32,
    rng: Rng,
}

impl OrbLife {
    fn new(seed: u64) -> Self {
        Self {
            phase: OrbPhase::Whole,
            timer: 9.0,
            solidity: 1.0,
            rng: Rng::new(seed),
        }
    }

    /// Возвращает `true`, если сферу надо рисовать.
    fn update(&mut self, dt: f32, cloud: &mut Particles, centre: Vec2, radius: f32) -> bool {
        self.timer -= dt;
        match self.phase {
            OrbPhase::Whole => {
                self.solidity = (self.solidity + dt * 3.0).min(1.0);
                if self.timer <= 0.0 {
                    self.phase = OrbPhase::Scattered;
                    self.timer = self.rng.range(0.7, 1.2);
                    self.solidity = 0.0;
                    cloud.burst(centre, radius * self.rng.range(7.0, 11.0));
                }
            }
            OrbPhase::Scattered => {
                if self.timer <= 0.0 {
                    self.phase = OrbPhase::Gathering;
                    self.timer = 3.5;
                    cloud.reassemble(centre, radius * 0.9);
                }
            }
            OrbPhase::Gathering => {
                // сфера проявляется по мере схождения облака
                let converged = cloud.convergence(radius * 0.28);
                self.solidity = converged;
                if converged > 0.9 || self.timer <= 0.0 {
                    self.phase = OrbPhase::Whole;
                    self.timer = self.rng.range(11.0, 22.0);
                    cloud.reseed_orbit(centre, radius * 1.05, radius * 1.8, ORB_ORBIT_COUNT);
                }
            }
        }
        self.solidity > 0.02
    }
}

struct Clouds {
    pet: Particles,
    orb: Particles,
}

impl Clouds {
    fn new(rig: &Rig) -> Self {
        let mut pet = Particles::with_capacity(PET_PARTICLES, 0x5F1E_C7ED, rig.scale);
        // Облако собрано у ушей и хвоста, а не размазано вокруг силуэта.
        for (centre, radius) in &rig.anchors {
            pet.seed_orbit(*centre, radius * 0.35, *radius, PARTICLES_PER_ANCHOR);
        }
        let mut orb = Particles::with_capacity(ORB_PARTICLES, 0x0B1D_2E3F, rig.scale);
        let orb_half = rig.orb.width().max(rig.orb.height()) as f32 / 2.0;
        orb.seed_orbit(
            rig.orb_rest,
            orb_half * 1.05,
            orb_half * 1.8,
            ORB_ORBIT_COUNT,
        );
        Self { pet, orb }
    }

    fn alive(&self) -> usize {
        self.pet.alive() + self.orb.alive()
    }
}

fn run(initial_scale: f32, stdin_control: bool) -> Result<(), String> {
    let assets = Assets::load()?;
    let rig = assets.build(initial_scale);

    let (initial_cursor_x, initial_cursor_y) = pet_platform::cursor_position();
    let mut monitor = pet_platform::monitor_area_at(initial_cursor_x, initial_cursor_y);
    let screen = monitor.work;
    let mut anchor = Vec2::new(
        (screen.left + (screen.right - screen.left) / 2 - rig.canvas_size.0 as i32 / 2) as f32,
        (screen.bottom - rig.canvas_size.1 as i32 - WORK_AREA_BOTTOM_MARGIN) as f32,
    );

    let mut overlay = Overlay::new(
        rig.canvas_size.0,
        rig.canvas_size.1,
        anchor.x as i32,
        anchor.y as i32,
    )?;
    let mut canvas = Canvas::new(rig.canvas_size.0, rig.canvas_size.1);
    let mut character_layers = CharacterLayers::new(rig.canvas_size);
    let mut clouds = Clouds::new(&rig);
    let mut mood = Mood::new(0x1234_5678);
    let mut quads: Vec<Quad> = Vec::with_capacity(PET_PARTICLES.max(ORB_PARTICLES));
    // По умолчанию приложение полностью автономно. stdin активируется только
    // явным флагом и не связывает жизненный цикл с каким-либо другим продуктом.
    let events = stdin_control.then(Events::from_stdin);
    let mut inbox: Vec<Event> = Vec::new();
    let mut mind = Engagement::default();
    let mut typing_pulse = 0.0_f32;
    let mut body = Body::default();
    let mut walk = Walk::default();
    let mut orientation = Orientation::default();
    let mut hands = Hands::new(0x00BA_DA55);

    eprintln!(
        "pet_host: масштаб {:.2}, холст {}x{}, зверёк {}x{}, шарик {}x{}",
        rig.scale,
        rig.canvas_size.0,
        rig.canvas_size.1,
        rig.creature.width(),
        rig.creature.height(),
        rig.orb.width(),
        rig.orb.height(),
    );

    let mut stats = FrameStats::default();
    let mut last_frame = Instant::now();
    let mut last_report = Instant::now();
    let mut last_motion = Instant::now();
    let mut hopper = Hopper::new(0x00A1_B2C3);
    let orbit_radius = rig.creature.width() as f32 * 0.95;
    let mut orb_motion = OrbMotion::new(0x00C0_FFEE, orbit_radius);
    let mut orb_life = OrbLife::new(0x0DEF_ACED);
    let orb_body_radius = rig.orb.width() as f32 / 2.0;
    let mut orb_position = rig.orb_rest;

    loop {
        if !overlay.pump() {
            break;
        }

        let (cursor_x, cursor_y) = pet_platform::cursor_position();
        if cursor_x < monitor.bounds.left
            || cursor_x >= monitor.bounds.right
            || cursor_y < monitor.bounds.top
            || cursor_y >= monitor.bounds.bottom
        {
            monitor = pet_platform::monitor_area_at(cursor_x, cursor_y);
            anchor.y =
                (monitor.work.bottom - rig.canvas_size.1 as i32 - WORK_AREA_BOTTOM_MARGIN) as f32;
        }
        let desired = Vec2::new(
            cursor_x as f32 - rig.creature_centre.x,
            cursor_y as f32 - rig.creature_centre.y,
        );

        let had_events = if let Some(events) = &events {
            let ipc_connected = events.drain(&mut inbox);
            if !ipc_connected || inbox.contains(&Event::Shutdown) {
                break;
            }
            !inbox.is_empty()
        } else {
            inbox.clear();
            false
        };
        for event in &inbox {
            mind.apply(*event);
        }

        let now = Instant::now();
        let dt = (now - last_frame).as_secs_f32().min(0.1);
        // Персонаж сидит на своей линии и прыгает только по горизонтали:
        // тянуть его по вертикали за курсором значило бы, что он летает.
        let to_target = Vec2::new(desired.x - anchor.x, 0.0);
        if to_target.x.abs() > MOVING_THRESHOLD || had_events {
            last_motion = now;
        }
        mind.decay(dt);

        let activity = if to_target.x.abs() < STOP_DISTANCE && !hopper.airborne() && hands.idle() {
            if now - last_motion > SLEEP_AFTER {
                Activity::Asleep
            } else {
                Activity::Idle
            }
        } else if hopper.airborne()
            || to_target.x.abs() > rig.creature.width() as f32 * RUN_DISTANCE
        {
            // Бег и прыжок — самые быстрые движения, их нельзя показывать рвано.
            Activity::Active
        } else {
            Activity::Attentive
        };
        // Пока пользователь печатает или диктует, персонаж бодрствует, даже
        // если сам стоит на месте.
        let activity = if mind.typing > 0.05 || mind.listening {
            activity.max(Activity::Attentive)
        } else {
            activity
        };

        let Some(interval) = activity.frame_interval() else {
            overlay.wait(Duration::from_millis(120))?;
            last_frame = Instant::now();
            if (desired.x - anchor.x).abs() > 24.0 {
                last_motion = Instant::now();
            }
            continue;
        };
        if now - last_frame < interval {
            overlay.wait(interval.saturating_sub(now - last_frame))?;
            continue;
        }
        last_frame = now;
        let frame_started = Instant::now();

        let previous_anchor = anchor;
        // До курсора персонаж добирается шагом, а далёкую цель — бегом.
        // Прыжком он не передвигается вообще: прыжок остался эмоцией.
        let width = rig.creature.width() as f32;
        let distance = to_target.x.abs();
        let direction = if distance > 1.0 {
            to_target.x.signum()
        } else {
            0.0
        };
        let gait = if hopper.airborne() {
            Gait::Stand
        } else {
            Gait::choose(distance, width)
        };
        // Шаг не должен перелетать цель, иначе персонаж дрожит вокруг курсора.
        let mut travel = direction * gait.speed(width) * dt;
        if travel.abs() > distance {
            travel = to_target.x;
        }
        anchor.x += travel;
        walk.update(
            dt,
            travel.abs(),
            rig.creature.height() as f32 * 0.26,
            gait,
            direction,
        );
        orientation.update(dt, gait, direction);

        // Подпрыгивает от радости, а не по расстоянию до курсора.
        hopper.update(
            dt,
            mind.cheer > 0.7 && gait == Gait::Stand,
            rig.creature.height() as f32 * HOP_HEIGHT,
        );
        overlay.set_position(anchor.x as i32, anchor.y as i32)?;
        // Частицы живут в координатах холста; при переезде окна хвост должен
        // остаться на месте в экранных, поэтому сдвигаем на обратную величину.
        let drift = previous_anchor - anchor;
        clouds.pet.shift(drift);
        clouds.orb.shift(drift);

        let offset = orb_motion.update(dt, orbit_radius);
        let next_orb = rig.creature_centre + offset;
        // Якоря облака переезжают вместе с шариком, иначе оно отстанет. Во
        // время разлёта двигать нечего: частицы летят сами.
        clouds.orb.move_home(next_orb - orb_position);
        orb_position = next_orb;

        let emitter = rig.creature_centre + Vec2::new(0.0, rig.creature.height() as f32 * 0.18);
        // Кометный хвост тянется за бегом: на шаге его нет, иначе персонаж
        // всё время выглядел бы разогнавшимся.
        if gait == Gait::Run {
            clouds.pet.emit_trail(
                emitter,
                34.0 * rig.scale,
                Vec2::new(-direction * gait.speed(width) * 0.18, 0.0),
            );
        } else {
            clouds.pet.emit_trail(emitter, f32::MAX, Vec2::default());
        }
        // Облако у ушей и хвоста обязано следовать за телом: при прыжке
        // персонаж поднимается, и частицы должны подняться вместе с ним.
        clouds.pet.set_anchor_offset(Vec2::new(0.0, -hopper.height));
        clouds.pet.update(dt);
        clouds.orb.update(dt);
        let orb_visible = orb_life.update(dt, &mut clouds.orb, orb_position, orb_body_radius);

        let gaze = (
            (to_target.x / (rig.canvas_size.0 as f32 * 0.6)).clamp(-1.0, 1.0),
            (to_target.y / (rig.canvas_size.1 as f32 * 0.6)).clamp(-1.0, 1.0),
        );
        mood.update(dt, gaze, activity, &mind);
        body.update(dt, &mind, activity, hopper.squash);
        hands.update(dt, &mind, gait);

        // Печать отзывается в облаке: частицы у ушей вспыхивают в ритм ударов
        // по клавишам, а не просто светятся ярче.
        typing_pulse += dt * (3.0 + mind.typing * 9.0);
        let pulse = 1.0 + mind.typing * (0.35 + typing_pulse.sin() * 0.25);
        clouds.pet.set_brightness(pulse);
        clouds.orb.set_brightness(1.0 + mind.voice * 0.5);

        compose(
            &mut canvas,
            &Frame {
                rig: &rig,
                mood: &mood,
                hopper: &hopper,
                warp: body.warp(hopper.squash),
                walk: &walk,
                hands: &hands,
                orientation: &orientation,
                orb_position,
                orb_visible,
            },
            &mut clouds,
            &mut quads,
            &mut character_layers,
        );
        overlay.present(canvas.pixels())?;
        stats.record(frame_started.elapsed());

        if last_report.elapsed() >= Duration::from_secs(1) {
            eprintln!(
                "кадр: среднее {:.2} мс, пик {:.2} мс, кадров {}, частиц {}, режим {activity:?}",
                stats.average_ms(),
                stats.peak_ms(),
                stats.frames(),
                clouds.alive(),
            );
            stats.reset();
            last_report = Instant::now();
        }
    }
    Ok(())
}

/// Собирает кадр. Порядок слоёв: дальние частицы, спрайт, лицо, шарик,
/// ближние частицы — так облако проходит и за спиной, и перед персонажем.
/// Всё, что описывает один кадр. Собрано в структуру, чтобы `compose` не
/// разрастался списком аргументов.
struct Frame<'a> {
    rig: &'a Rig,
    mood: &'a Mood,
    hopper: &'a Hopper,
    /// Пластика тела на этот кадр.
    warp: Warp,
    walk: &'a Walk,
    hands: &'a Hands,
    orientation: &'a Orientation,
    orb_position: Vec2,
    orb_visible: bool,
}

struct CharacterLayers {
    front: Canvas,
    profile: Canvas,
}

impl CharacterLayers {
    fn new(size: (u32, u32)) -> Self {
        Self {
            front: Canvas::new(size.0, size.1),
            profile: Canvas::new(size.0, size.1),
        }
    }
}

fn compose(
    canvas: &mut Canvas,
    frame: &Frame<'_>,
    clouds: &mut Clouds,
    quads: &mut Vec<Quad>,
    character_layers: &mut CharacterLayers,
) {
    let Frame {
        rig,
        hopper,
        orb_position,
        orb_visible,
        ..
    } = *frame;
    canvas.clear();

    // Тень ложится первой и живёт вместе с шагом: чем выше персонаж над
    // землёй, тем она мельче и слабее. Это и читается как переступание.
    draw_shadow(canvas, rig, hopper);

    clouds.pet.collect(quads);
    draw_layer(canvas, quads, true);
    clouds.orb.collect(quads);
    draw_layer(canvas, quads, true);

    draw_character_transition(canvas, frame, character_layers);

    // Во время разлёта сфера не рисуется — на её месте живут частицы.
    if orb_visible {
        canvas.draw(
            &rig.orb,
            (orb_position.x - rig.orb.width() as f32 / 2.0) as i32,
            (orb_position.y - rig.orb.height() as f32 / 2.0) as i32,
        );
    }

    clouds.pet.collect(quads);
    draw_layer(canvas, quads, false);
    clouds.orb.collect(quads);
    draw_layer(canvas, quads, false);
}

fn draw_character_transition(canvas: &mut Canvas, frame: &Frame<'_>, layers: &mut CharacterLayers) {
    let mix = frame.orientation.profile;
    if mix <= 0.001 {
        draw_front_character(canvas, frame);
    } else if mix >= 0.999 {
        draw_profile_character(canvas, frame);
    } else {
        layers.front.clear();
        layers.profile.clear();
        draw_front_character(&mut layers.front, frame);
        draw_profile_character(&mut layers.profile, frame);
        canvas.draw_canvas_opacity(&layers.front, 1.0 - mix);
        canvas.draw_canvas_opacity(&layers.profile, mix);
    }
}

fn draw_front_character(canvas: &mut Canvas, frame: &Frame<'_>) {
    let Frame {
        rig,
        mood,
        hopper,
        warp,
        walk,
        hands,
        ..
    } = *frame;

    let lift = hopper.height + walk.body_lift(rig.creature.height() as f32 * 0.018);
    let sway = walk.body_sway(rig.creature.width() as f32 * 0.012);

    // Наклон корпуса по ходу движения. Верх уходит вперёд, низ остаётся на
    // ногах — именно этим бег отличается от быстрой ходьбы сильнее всего.
    // Смещение отрицательное, потому что сетка сдвигает точку выборки.
    let mut warp = warp;
    let lean = walk.body_lean() * 0.05;
    warp.add_row(0, -lean, 0.0);
    warp.add_row(1, -lean * 0.70, 0.0);
    warp.add_row(2, -lean * 0.35, 0.0);

    // Ноги рисуются до корпуса: низ куртки перекрывает срез бедра и стык не
    // виден. Дальняя нога идёт первой, чтобы ближняя её перекрыла.
    let (left_angle, right_angle) = walk.leg_angles(0.42);
    let leg_margin = (rig.creature.height() as f32 * 0.10).ceil() as i32 + 2;
    for (index, (sprite, pivot, position)) in rig.legs.iter().enumerate() {
        let angle = if index == 0 { left_angle } else { right_angle };
        canvas.draw_rotated(
            sprite,
            (position.x + sway) as i32,
            (position.y - lift) as i32,
            *pivot,
            angle,
            leg_margin,
        );
    }

    // Запас под деформацию: уши и хвост выезжают за исходный прямоугольник.
    let (extent_x, extent_y) = warp.extent();
    let margin = (extent_x.max(extent_y) * rig.creature.height() as f32).ceil() as i32 + 2;
    canvas.draw_warped(
        &rig.creature,
        (rig.creature_origin.x + sway) as i32,
        (rig.creature_origin.y - lift) as i32,
        &warp,
        margin,
    );

    let face = pet_face::render(&mood.face, rig.visor, rig.scale);
    canvas.draw_rgba(
        face.data(),
        face.width(),
        face.height(),
        (rig.visor_origin.x + sway) as i32,
        (rig.visor_origin.y - lift) as i32,
    );

    // Кисти поверх куртки: они выходят из карманов, значит, ближе корпуса.
    draw_hands(canvas, rig, hands, sway, lift);
}

fn draw_profile_character(canvas: &mut Canvas, frame: &Frame<'_>) {
    let rig = frame.rig;
    let pose = frame.orientation.profile_rig(rig);
    let lift = frame.hopper.height + frame.walk.body_lift(pose.creature.height() as f32 * 0.022);
    let sway = frame.walk.body_sway(pose.creature.width() as f32 * 0.008);
    let (far_angle, near_angle) = frame.walk.leg_angles(0.48);
    let far_angle = far_angle.clamp(-0.60, 0.60);
    let near_angle = near_angle.clamp(-0.60, 0.60);
    let leg_margin = (pose.creature.height() as f32 * 0.12).ceil() as i32 + 2;
    for (index, (sprite, pivot, position)) in pose.legs.iter().enumerate() {
        let angle = if index == 0 { far_angle } else { near_angle };
        canvas.draw_rotated(
            sprite,
            (position.x + sway) as i32,
            (position.y - lift) as i32,
            *pivot,
            angle,
            leg_margin,
        );
    }
    let arm_margin = (pose.creature.height() as f32 * 0.15).ceil() as i32 + 2;
    let arm_angles = (-far_angle * 0.86, -near_angle * 0.86);
    if let Some((sprite, pivot, position)) = pose.arms.first() {
        canvas.draw_rotated(
            sprite,
            (position.x + sway) as i32,
            (position.y - lift) as i32,
            *pivot,
            arm_angles.0,
            arm_margin,
        );
    }
    canvas.draw(
        &pose.creature,
        (pose.creature_origin.x + sway) as i32,
        (pose.creature_origin.y - lift) as i32,
    );
    let facing_right = frame.orientation.facing > 0.0;
    let face = pet_face::render_profile(&frame.mood.face, pose.visor, rig.scale, facing_right);
    canvas.draw_rgba(
        face.data(),
        face.width(),
        face.height(),
        (pose.visor_origin.x + sway) as i32,
        (pose.visor_origin.y - lift) as i32,
    );
    if let Some((sprite, pivot, position)) = pose.arms.get(1) {
        canvas.draw_rotated(
            sprite,
            (position.x + sway) as i32,
            (position.y - lift) as i32,
            *pivot,
            arm_angles.1,
            arm_margin,
        );
    }
}

/// Голографические кисти по обе стороны куртки.
///
/// Кисть приставляется к карману запястьем, а не углом холста: иначе при смене
/// жеста, когда меняется длина пальцев, она бы съезжала.
fn draw_hands(canvas: &mut Canvas, rig: &Rig, hands: &Hands, sway: f32, lift: f32) {
    let size = (rig.creature.width() as f32 * HAND_WIDTH).max(12.0) as u32;
    let reach = hands.clap_reach() * rig.creature.width() as f32 * 0.22;
    let height = rig.creature.height() as f32;

    for (index, pose) in [&hands.left, &hands.right].into_iter().enumerate() {
        let Some(pocket) = rig.pockets.get(index) else {
            continue;
        };
        let mirrored = index == 0;
        let Some(pixmap) = pet_hands::render(pose, size, mirrored, pet_hands::Glow::default())
        else {
            continue;
        };
        // При хлопке ладони сходятся к средней линии тела.
        let inward = if mirrored { reach } else { -reach };
        // Кисть выезжает из кармана вверх по мере проявления.
        let rise = pose.visibility() * height * 0.05;
        let wrist = Vec2::new(pocket.x + inward + sway, pocket.y - rise - lift);
        canvas.draw_rgba(
            pixmap.data(),
            pixmap.width(),
            pixmap.height(),
            (wrist.x - pixmap.width() as f32 / 2.0) as i32,
            (wrist.y - pixmap.height() as f32 * pet_hands::WRIST_Y) as i32,
        );
    }
}

/// Мягкая тень под персонажем.
///
/// Радиус и плотность обратны подъёму: на пике шага пятно уже и светлее, при
/// касании — шире и темнее. Без этого персонаж выглядит парящим.
fn draw_shadow(canvas: &mut Canvas, rig: &Rig, hopper: &Hopper) {
    let lift_ratio = (hopper.height / (rig.creature.height() as f32 * HOP_HEIGHT)).clamp(0.0, 1.0);
    // Чем выше персонаж, тем мельче и бледнее пятно. В приседе наоборот —
    // тень расплывается, потому что он ближе к земле и шире.
    let base_rx = rig.creature.width() as f32 * 0.34 / hopper.squash.max(0.5);
    let rx = base_rx * (1.0 - lift_ratio * 0.45);
    let ry = rx * 0.30;
    let alpha = (0.42 * (1.0 - lift_ratio * 0.65) * 255.0) as u32;
    let sway = 0.0_f32;
    // premultiplied: тень нейтрально-тёмная, слегка холодная
    let premultiply = |channel: u32| -> u8 { ((channel * alpha + 127) / 255) as u8 };
    canvas.fill_ellipse(
        rig.creature_centre.x + sway,
        rig.ground,
        rx,
        ry,
        [
            premultiply(28),
            premultiply(20),
            premultiply(16),
            alpha as u8,
        ],
    );
}

/// Рисует частицы одного слоя: `behind` — те, что дальше спрайта.
fn draw_layer(canvas: &mut Canvas, quads: &[Quad], behind: bool) {
    for quad in quads {
        if (quad.depth < 0.0) != behind {
            continue;
        }
        draw_quad(canvas, quad.position, quad);
    }
}

/// Квадратик с мягким ореолом.
///
/// В исходном арте частицы светятся, а не залиты плашкой. Дешёвый способ
/// получить то же ощущение — подложить под ядро квадрат вдвое шире с низкой
/// альфой; настоящее размытие здесь не окупается.
fn draw_quad(canvas: &mut Canvas, position: Vec2, quad: &Quad) {
    let core_alpha = quad.alpha.clamp(0.0, 1.0);
    if core_alpha <= 0.0 {
        return;
    }
    let colour_at = |alpha: f32| -> [u8; 4] {
        let a = (alpha.clamp(0.0, 1.0) * 255.0) as u32;
        let premultiply = |channel: u8| -> u8 {
            let value = u32::from(channel) * a;
            ((value + 1 + (value >> 8)) >> 8) as u8
        };
        [
            premultiply(quad.color.2),
            premultiply(quad.color.1),
            premultiply(quad.color.0),
            a as u8,
        ]
    };

    let core = quad.size.max(1.0) as i32;
    let halo = (core * 2).max(core + 2);
    canvas.fill_rect(
        position.x as i32 - halo / 2,
        position.y as i32 - halo / 2,
        halo,
        halo,
        colour_at(core_alpha * 0.22),
    );
    canvas.fill_rect(
        position.x as i32 - core / 2,
        position.y as i32 - core / 2,
        core,
        core,
        colour_at(core_alpha),
    );
}

/// Контрольный лист: тот же персонаж в разных масштабах и выражениях.
/// Нужен, чтобы видеть результат, не гоняя окно, — зародыш визуальной
/// лаборатории.
fn dump_sheet(path: &Path, scale: f32) -> Result<(), String> {
    let assets = Assets::load()?;
    type Apply = fn(&mut FaceState);
    let expressions: [(&str, Apply, f32); 4] = [
        ("обычное", |_| {}, scale),
        (
            "удивление",
            |face| {
                set_both(face, EyeGlyph::Circle);
                face.mouth_open = 0.8;
                face.mouth_curve = 0.2;
            },
            scale,
        ),
        (
            "прищур",
            |face| {
                set_both(face, EyeGlyph::Squint);
                face.mouth_curve = -0.5;
            },
            scale * 0.6,
        ),
        (
            "сон",
            |face| {
                set_both(face, EyeGlyph::Closed);
                face.glow = 0.45;
                face.mouth_curve = 0.15;
            },
            scale * 1.5,
        ),
    ];

    let mut tiles: Vec<Canvas> = Vec::new();
    for (name, apply, tile_scale) in expressions {
        let tile_scale = tile_scale.clamp(MIN_SCALE, MAX_SCALE);
        let rig = assets.build(tile_scale);
        let mut canvas = Canvas::new(rig.canvas_size.0, rig.canvas_size.1);
        let mut clouds = Clouds::new(&rig);
        for _ in 0..180 {
            clouds.pet.update(1.0 / 60.0);
            clouds.orb.update(1.0 / 60.0);
        }
        let mut mood = Mood::new(0x1234_5678);
        apply(&mut mood.face);
        // прижатые уши и вильнувший хвост — чтобы пластика была видна на листе
        let body = Body {
            ear_droop: if name == "сон" { 1.0 } else { 0.0 },
            wag: 1.0,
            tail: 1.2,
            breath: 0.0,
        };
        let mut quads = Vec::new();
        let mut character_layers = CharacterLayers::new(rig.canvas_size);
        compose(
            &mut canvas,
            &Frame {
                rig: &rig,
                mood: &mood,
                hopper: &Hopper::new(1),
                warp: body.warp(1.0),
                walk: &Walk::default(),
                hands: &Hands::new(1),
                orientation: &Orientation::default(),
                orb_position: rig.orb_rest,
                orb_visible: true,
            },
            &mut clouds,
            &mut quads,
            &mut character_layers,
        );
        eprintln!(
            "  {name}: масштаб {tile_scale:.2}, кадр {}x{}",
            rig.canvas_size.0, rig.canvas_size.1
        );
        tiles.push(canvas);
    }

    let width: u32 = tiles.iter().map(Canvas::width).sum();
    let height: u32 = tiles.iter().map(Canvas::height).max().unwrap_or(1);
    let mut sheet = Canvas::new(width, height);
    let mut x = 0_i32;
    for tile in &tiles {
        sheet.draw_canvas(tile, x, 0);
        x += tile.width() as i32;
    }
    pet_render::save_on_checkerboard(&sheet, path).map_err(|error| error.to_string())?;
    eprintln!("лист сохранён: {}", path.display());
    Ok(())
}

/// Полоса кадров ходьбы и бега. Нужна, чтобы видеть, что ноги действительно
/// переступают в противофазе, а корпус на бегу наклоняется вперёд.
///
/// Верхний ряд — шаг, нижний — бег: разницу в амплитуде и наклоне на статичном
/// кадре иначе не оценить.
fn dump_walk(path: &Path, scale: f32) -> Result<(), String> {
    let assets = Assets::load()?;
    let rig = assets.build(scale.clamp(MIN_SCALE, MAX_SCALE));
    if rig.legs.is_empty() {
        return Err("текущая поза не содержит ног: ходьбу показать нечем".to_owned());
    }
    const FRAMES: usize = 8;
    let width = rig.creature.width() as f32;
    let stride = rig.creature.height() as f32 * 0.26;
    let mut rows = Vec::new();
    for gait in [Gait::Walk, Gait::Run] {
        let speed = gait.speed(width);
        let mut tiles = Vec::new();
        for frame in 0..FRAMES {
            let mut walk = Walk::default();
            // Разгоняем цикл ровно тем же путём, что и в живом ходе: иначе
            // полоса показала бы не то, что видно на экране.
            for _ in 0..90 {
                walk.update(1.0 / 60.0, speed / 60.0, stride, gait, 1.0);
            }
            let travel = stride * 2.0 * frame as f32 / FRAMES as f32;
            walk.update(1.0 / 60.0, travel, stride, gait, 1.0);

            let mut canvas = Canvas::new(rig.canvas_size.0, rig.canvas_size.1);
            let mut clouds = Clouds::new(&rig);
            let mood = Mood::new(0x1234_5678);
            let mut quads = Vec::new();
            let orientation = Orientation {
                profile: 1.0,
                facing: 1.0,
            };
            let mut character_layers = CharacterLayers::new(rig.canvas_size);
            compose(
                &mut canvas,
                &Frame {
                    rig: &rig,
                    mood: &mood,
                    hopper: &Hopper::new(1),
                    warp: Warp::identity(),
                    walk: &walk,
                    hands: &Hands::new(1),
                    orientation: &orientation,
                    orb_position: rig.orb_rest,
                    orb_visible: true,
                },
                &mut clouds,
                &mut quads,
                &mut character_layers,
            );
            tiles.push(canvas);
        }
        rows.push(tiles);
    }
    let tile_width = rig.canvas_size.0;
    let tile_height = rig.canvas_size.1;
    let mut sheet = Canvas::new(tile_width * FRAMES as u32, tile_height * rows.len() as u32);
    for (row, tiles) in rows.iter().enumerate() {
        for (column, tile) in tiles.iter().enumerate() {
            sheet.draw_canvas(
                tile,
                (column as u32 * tile_width) as i32,
                (row as u32 * tile_height) as i32,
            );
        }
    }
    pet_render::save_on_checkerboard(&sheet, path).map_err(|error| error.to_string())?;
    eprintln!(
        "полоса ходьбы: {} (шаг и бег, по {FRAMES} кадров)",
        path.display()
    );
    Ok(())
}

/// Лист жестов. Без него голографические кисти можно проверить только вживую,
/// а они появляются по событиям и держатся полторы секунды.
fn dump_hands(path: &Path, scale: f32) -> Result<(), String> {
    let assets = Assets::load()?;
    let rig = assets.build(scale.clamp(MIN_SCALE, MAX_SCALE));
    let poses: [(&str, Gesture, Gesture); 5] = [
        ("класс", Gesture::Hidden, Gesture::ThumbsUp),
        ("фак", Gesture::Hidden, Gesture::MiddleFinger),
        ("мир", Gesture::Hidden, Gesture::Peace),
        ("тычок", Gesture::Hidden, Gesture::Point),
        ("хлопок", Gesture::Open, Gesture::Open),
    ];

    let mut tiles = Vec::new();
    for (name, left, right) in poses {
        let mut hands = Hands::new(1);
        hands.show(left, right, 3.0);
        // Хлопок показываем в момент схождения ладоней, иначе на статичном
        // кадре он неотличим от двух раскрытых рук.
        hands.clapping = left == Gesture::Open && right == Gesture::Open;
        for _ in 0..40 {
            hands.left.update(1.0 / 60.0);
            hands.right.update(1.0 / 60.0);
        }
        hands.clap_phase = std::f32::consts::FRAC_PI_2;

        let mut canvas = Canvas::new(rig.canvas_size.0, rig.canvas_size.1);
        let mut clouds = Clouds::new(&rig);
        let mood = Mood::new(0x1234_5678);
        let mut quads = Vec::new();
        let mut character_layers = CharacterLayers::new(rig.canvas_size);
        compose(
            &mut canvas,
            &Frame {
                rig: &rig,
                mood: &mood,
                hopper: &Hopper::new(1),
                warp: Warp::identity(),
                walk: &Walk::default(),
                hands: &hands,
                orientation: &Orientation::default(),
                orb_position: rig.orb_rest,
                orb_visible: false,
            },
            &mut clouds,
            &mut quads,
            &mut character_layers,
        );
        eprintln!("  жест {name}");
        tiles.push(canvas);
    }

    let width: u32 = tiles.iter().map(Canvas::width).sum();
    let height: u32 = tiles.iter().map(Canvas::height).max().unwrap_or(1);
    let mut sheet = Canvas::new(width, height);
    let mut x = 0_i32;
    for tile in &tiles {
        sheet.draw_canvas(tile, x, 0);
        x += tile.width() as i32;
    }
    pet_render::save_on_checkerboard(&sheet, path).map_err(|error| error.to_string())?;
    eprintln!("лист жестов: {}", path.display());
    Ok(())
}

fn set_both(face: &mut FaceState, glyph: EyeGlyph) {
    face.left.glyph = glyph;
    face.left.next = glyph;
    face.right.glyph = glyph;
    face.right.next = glyph;
}

fn asset_path(name: &str) -> PathBuf {
    if let Some(root) = std::env::var_os("DESKTOP_PET_ASSETS") {
        return PathBuf::from(root).join(name);
    }

    let installed = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join("assets")
        .join(name);
    if installed.is_file() {
        return installed;
    }

    // Development builds keep generated runtime assets in the workspace. A
    // release build deliberately has no source-tree fallback: publishing must
    // prove that the exact allow-listed asset set was shipped beside the EXE.
    #[cfg(debug_assertions)]
    {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        base.canonicalize().unwrap_or(base).join(name)
    }

    #[cfg(not(debug_assertions))]
    installed
}
