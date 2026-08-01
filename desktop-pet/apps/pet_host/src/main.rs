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

const ORB_PARTICLES: usize = 320;
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
    let walk_style = match argument_value(&arguments, "--walk-style") {
        Some(value) => match WalkStyle::parse(&value) {
            Some(style) => style,
            None => {
                eprintln!("pet_host: unknown walk style '{value}'; expected confident or woodland");
                std::process::exit(2);
            }
        },
        None => WalkStyle::default(),
    };

    let result = match (
        argument_value(&arguments, "--dump"),
        argument_value(&arguments, "--walk"),
        argument_value(&arguments, "--hands"),
    ) {
        (_, _, Some(path)) => dump_hands(Path::new(&path), scale),
        (_, Some(path), None) => dump_walk(Path::new(&path), scale, walk_style),
        (Some(path), None, None) => dump_sheet(Path::new(&path), scale),
        (None, None, None) => run(scale, stdin_control, walk_style),
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
    leg_chains: Vec<ProfileLegChain>,
    /// Profile depth is explicit because a flattened master may contain only
    /// one cleanly separable arm.
    arm_far: Option<(Sprite, (f32, f32), Vec2)>,
    arm_near: Option<(Sprite, (f32, f32), Vec2)>,
    /// Tail is a single continuous layer behind the torso.  Keeping it out of
    /// the body mesh prevents a wag from pulling the pelvis and upper back.
    tail: Option<(Sprite, (f32, f32), Vec2)>,
}

/// Нога в полном разрешении со своей геометрией.
struct LegSource {
    sprite: SpriteSource,
    /// Бедро в координатах спрайта ноги.
    pivot: (f32, f32),
    /// Где нога стоит относительно корпуса, в координатах исходного арта.
    origin: (f32, f32),
}

struct LegChainSource {
    sprite: SpriteSource,
    /// Crop origin in the shared source canvas.
    origin: (f32, f32),
    /// Joints in the cropped continuous leg texture.
    hip: (f32, f32),
    knee: (f32, f32),
    ankle: (f32, f32),
}

struct ProfileLegChain {
    sprite: Sprite,
    origin: Vec2,
    hip: (f32, f32),
    knee: (f32, f32),
    ankle: (f32, f32),
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
    leg_chains: Vec<LegChainSource>,
    arm_far: Option<LegSource>,
    arm_near: Option<LegSource>,
    tail: Option<LegSource>,
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
        let load_limb = |limb: &manifest::LegManifest| -> Result<LegSource, String> {
            let sprite = SpriteSource::load(&asset_path(&limb.sprite), 10)
                .map_err(|error| format!("{}: {error}", limb.sprite))?;
            let (crop_x, crop_y) = sprite.offset();
            Ok(LegSource {
                pivot: (
                    limb.pivot.0 - limb.origin.0 - crop_x as f32,
                    limb.pivot.1 - limb.origin.1 - crop_y as f32,
                ),
                origin: limb.origin,
                sprite,
            })
        };
        let arm_far = manifest.arms.far.as_ref().map(load_limb).transpose()?;
        let arm_near = manifest.arms.near.as_ref().map(load_limb).transpose()?;
        let tail = manifest.tail.as_ref().map(load_limb).transpose()?;
        let mut leg_chains = Vec::new();
        for leg in &manifest.leg_chains {
            let sprite = SpriteSource::load(&asset_path(&leg.sprite), 10)
                .map_err(|error| format!("{}: {error}", leg.sprite))?;
            let (crop_x, crop_y) = sprite.offset();
            let local = |joint: (f32, f32)| (joint.0 - crop_x as f32, joint.1 - crop_y as f32);
            leg_chains.push(LegChainSource {
                sprite,
                origin: (crop_x as f32, crop_y as f32),
                hip: local(leg.hip),
                knee: local(leg.knee),
                ankle: local(leg.ankle),
            });
        }
        Ok(Self {
            creature,
            legs,
            leg_chains,
            arm_far,
            arm_near,
            tail,
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
        let build_limb = |limb: &LegSource| {
            let sprite = limb.sprite.scaled(scale);
            let (crop_x, crop_y) = limb.sprite.offset();
            let position = Vec2::new(
                creature_origin.x + (limb.origin.0 + crop_x as f32 - offset_x as f32) * scale,
                creature_origin.y + (limb.origin.1 + crop_y as f32 - offset_y as f32) * scale,
            );
            (
                sprite,
                (limb.pivot.0 * scale, limb.pivot.1 * scale),
                position,
            )
        };
        let arm_far = self.arm_far.as_ref().map(build_limb);
        let arm_near = self.arm_near.as_ref().map(build_limb);
        let tail = self.tail.as_ref().map(build_limb);
        let leg_chains = self
            .leg_chains
            .iter()
            .map(|leg| ProfileLegChain {
                sprite: leg.sprite.scaled(scale),
                origin: Vec2::new(
                    creature_origin.x + (leg.origin.0 - offset_x as f32) * scale,
                    creature_origin.y + (leg.origin.1 - offset_y as f32) * scale,
                ),
                hip: (leg.hip.0 * scale, leg.hip.1 * scale),
                knee: (leg.knee.0 * scale, leg.knee.1 * scale),
                ankle: (leg.ankle.0 * scale, leg.ankle.1 * scale),
            })
            .collect();
        ProfileRig {
            creature,
            creature_origin,
            visor_origin,
            visor: self.manifest.visor_geometry(),
            legs,
            leg_chains,
            arm_far,
            arm_near,
            tail,
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

/// Character of the ordinary walking cycle. Both variants use the same
/// articulated rig and IK solver; only physically meaningful gait parameters
/// differ, so adding a style cannot reintroduce detached limbs or foot sliding.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum WalkStyle {
    /// Springy, self-assured gait used by the hacker character by default.
    #[default]
    Confident,
    /// Quieter, longer-support stride based on the local woods-walk reference.
    Woodland,
}

impl WalkStyle {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "confident" | "default" => Some(Self::Confident),
            "woodland" | "woods" => Some(Self::Woodland),
            _ => None,
        }
    }

    fn profile(self) -> WalkProfile {
        match self {
            Self::Confident => WalkProfile {
                stance: 0.60,
                stride_scale: 0.46,
                swing_height: 1.0,
                rise: 1.0,
                swagger: 1.0,
                speed: 1.0,
            },
            Self::Woodland => WalkProfile {
                stance: 0.66,
                stride_scale: 0.40,
                swing_height: 0.72,
                rise: 0.78,
                swagger: 0.62,
                speed: 0.78,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct WalkProfile {
    /// Fraction of a leg cycle during which the paw carries weight.
    stance: f32,
    /// Step length relative to the articulated leg length.
    stride_scale: f32,
    /// Swing-paw ground clearance.
    swing_height: f32,
    /// Rise-and-fall amplitude of the torso.
    rise: f32,
    /// Shoulder counter-rotation amplitude.
    swagger: f32,
    /// Translation speed without changing contact geometry.
    speed: f32,
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
    fn speed(self, width: f32, walk_style: WalkStyle) -> f32 {
        match self {
            Self::Stand => 0.0,
            Self::Walk => width * WALK_SPEED * walk_style.profile().speed,
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
/// Ходьба и бег используют один профильный риг, но разные траектории стопы.
/// У бега есть настоящая фаза полёта; ускорять обычный шаг для этого нельзя.
#[derive(Clone, Copy, Default)]
struct LegMotion {
    /// -0.5 is the trailing contact, +0.5 the forward contact.
    horizontal: f32,
    /// Ground clearance during swing, normalized to 0..=1.
    lift: f32,
    /// Toe lift during swing, normalized to 0..=1.
    toe_lift: f32,
    /// The paw is carrying weight and must remain locked to the ground plane.
    grounded: bool,
    /// Small loading-response flex. A planted leg is otherwise almost straight;
    /// the visible knee belongs to the lifted swing leg.
    support_flex: f32,
}

#[derive(Clone, Copy, Default)]
struct SolvedLeg {
    hip: f32,
    knee: f32,
    foot: f32,
}

#[derive(Clone, Copy, Default)]
struct LegGeometry {
    knee: (f32, f32),
    ankle: (f32, f32),
}

#[derive(Clone, Copy, Default)]
struct ProfilePelvis {
    horizontal: f32,
    lift: f32,
}

#[derive(Clone, Copy)]
struct ProfileLegFrame {
    motion: LegMotion,
    facing_right: bool,
    run: f32,
    blend: f32,
    pelvis: ProfilePelvis,
    track_x: f32,
    stride: f32,
}

#[derive(Clone, Copy, Debug)]
struct Walk {
    phase: f32,
    /// Насколько цикл активен: 0 — стоит, 1 — идёт.
    blend: f32,
    /// Насколько это бег, а не шаг: 0…1. Плавный, чтобы разгон был виден.
    run: f32,
    /// Наклон корпуса по ходу движения, -1…1.
    lean: f32,
    style: WalkStyle,
}

impl Default for Walk {
    fn default() -> Self {
        Self {
            phase: 0.0,
            blend: 0.0,
            run: 0.0,
            lean: 0.0,
            style: WalkStyle::default(),
        }
    }
}

impl Walk {
    /// `travelled` — путь за кадр, `stride` — длина шага в пикселях,
    /// `direction` — знак движения по горизонтали.
    fn update(&mut self, dt: f32, travelled: f32, stride: f32, gait: Gait, direction: f32) {
        // Phase follows travelled distance. The divisor changes with the same
        // stride profile as the feet, keeping a planted paw from skating when a
        // quieter walking style is selected. Running deliberately covers more
        // ground per cycle than either walk.
        let walk_stride = self.style.profile().stride_scale / 0.46;
        let run_stride = 1.60;
        let stride = stride.max(1.0) * mix(walk_stride, run_stride, self.run);
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

    /// One complete locomotion cycle. Walk and run trajectories are evaluated
    /// separately and blended during acceleration; the joints are solved later
    /// by IK, so this function describes the paw rather than arbitrary angles.
    fn profile_leg_motion(&self, index: usize) -> LegMotion {
        let walk = self.profile_walk_leg_motion(index);
        if self.run <= 1e-4 {
            return walk;
        }
        let run = self.profile_run_leg_motion(index);
        if self.run >= 1.0 - 1e-4 {
            return run;
        }
        let amount = smooth_unit(self.run);
        LegMotion {
            horizontal: mix(walk.horizontal, run.horizontal, amount),
            lift: mix(walk.lift, run.lift, amount),
            toe_lift: mix(walk.toe_lift, run.toe_lift, amount),
            // Contact semantics cannot be fractional: below the midpoint the
            // walking support leg owns the ground, above it the run contact does.
            grounded: if amount < 0.5 {
                walk.grounded
            } else {
                run.grounded
            },
            support_flex: mix(walk.support_flex, run.support_flex, amount),
        }
    }

    fn profile_walk_leg_motion(&self, index: usize) -> LegMotion {
        let profile = self.style.profile();
        let stance = profile.stance;
        let offset = if index == 0 { 0.0 } else { 0.5 };
        let cycle = (self.phase / std::f32::consts::TAU + offset).rem_euclid(1.0);
        if cycle < stance {
            // A planted paw travels backwards at a constant rate relative to
            // the moving pelvis. Easing this part made the foot visibly skate.
            let amount = (cycle / stance).clamp(0.0, 1.0);
            let support_flex = if amount < 0.18 {
                smooth_unit(amount / 0.18)
            } else if amount < 0.62 {
                1.0 - smooth_unit((amount - 0.18) / 0.44)
            } else {
                smooth_unit((amount - 0.62) / 0.38) * 0.35
            };
            LegMotion {
                horizontal: 0.5 - amount,
                lift: 0.0,
                toe_lift: 0.0,
                grounded: true,
                support_flex,
            }
        } else {
            let amount = ((cycle - stance) / (1.0 - stance)).clamp(0.0, 1.0);
            // Flexion peaks shortly after the paw passes the support leg, then
            // the knee opens while the heel reaches for the next contact.
            let lift = (if amount < 0.56 {
                smooth_unit(amount / 0.56)
            } else {
                1.0 - smooth_unit((amount - 0.56) / 0.44)
            }) * profile.swing_height;
            let advance =
                smooth_unit(amount + (amount * std::f32::consts::PI).sin().max(0.0) * 0.08);
            LegMotion {
                horizontal: -0.5 + advance,
                lift,
                toe_lift: lift,
                grounded: false,
                support_flex: 0.0,
            }
        }
    }

    /// Running spends less time on the ground and includes a real aerial
    /// interval. Each leg absorbs one contact, drives backwards through takeoff,
    /// folds underneath the body, then opens for the next landing.
    fn profile_run_leg_motion(&self, index: usize) -> LegMotion {
        const CONTACT: f32 = 0.36;
        let offset = if index == 0 { 0.0 } else { 0.5 };
        let cycle = (self.phase / std::f32::consts::TAU + offset).rem_euclid(1.0);
        if cycle < CONTACT {
            let amount = (cycle / CONTACT).clamp(0.0, 1.0);
            LegMotion {
                horizontal: 0.56 - amount * 1.12,
                lift: 0.0,
                toe_lift: 0.0,
                grounded: true,
                support_flex: (amount * std::f32::consts::PI).sin().max(0.0),
            }
        } else {
            let amount = ((cycle - CONTACT) / (1.0 - CONTACT)).clamp(0.0, 1.0);
            let lift = if amount < 0.46 {
                smooth_unit(amount / 0.46)
            } else {
                1.0 - smooth_unit((amount - 0.46) / 0.54)
            };
            let advance = smooth_unit(amount);
            LegMotion {
                horizontal: -0.56 + advance * 1.12,
                lift,
                toe_lift: lift,
                grounded: false,
                support_flex: 0.0,
            }
        }
    }

    /// Vertical weight transfer follows the four animation poses: contact,
    /// down, passing and up. A piecewise curve prevents the body from floating
    /// on an unrelated sine while the foot is supposed to carry weight.
    fn body_lift(&self, amount: f32) -> f32 {
        let phase = (self.phase / std::f32::consts::PI).rem_euclid(1.0);
        let normalized = if phase < 0.16 {
            mix(0.0, -0.35, smooth_unit(phase / 0.16))
        } else if phase < 0.50 {
            mix(-0.35, 0.45, smooth_unit((phase - 0.16) / 0.34))
        } else if phase < 0.75 {
            mix(0.45, 1.0, smooth_unit((phase - 0.50) / 0.25))
        } else {
            mix(1.0, 0.0, smooth_unit((phase - 0.75) / 0.25))
        };
        normalized * amount * self.blend * mix(self.style.profile().rise, 1.30, self.run)
    }

    /// Покачивание в сторону опорной ноги.
    fn body_sway(&self, amount: f32) -> f32 {
        self.phase.sin() * amount * self.blend
    }

    /// Deliberate shoulder-led swagger. It follows the supporting leg and is
    /// stronger at a walk than at a sprint, where the torso instead leans.
    fn swagger(&self) -> f32 {
        self.phase.sin() * self.blend * mix(self.style.profile().swagger, 0.65, self.run)
    }

    /// Aerial height for the run. `step_phase` repeats once per footfall; the
    /// last 28% is exactly the interval where neither run paw is grounded.
    fn run_flight_lift(&self) -> f32 {
        let step_phase = (self.phase / std::f32::consts::PI).rem_euclid(1.0);
        if step_phase <= 0.72 {
            return 0.0;
        }
        let air = ((step_phase - 0.72) / 0.28).clamp(0.0, 1.0);
        (air * std::f32::consts::PI).sin().max(0.0) * self.run * self.blend
    }

    /// Наклон корпуса по ходу движения, в долях размера спрайта.
    fn body_lean(&self) -> f32 {
        self.lean
    }
}

fn smooth_unit(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn mix(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
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
    fn update(&mut self, dt: f32, mind: &Engagement, activity: Activity, gait: Gait, squash: f32) {
        self.breath += dt * (1.1 + mind.typing * 0.9);
        self.tail += dt * (2.0 + self.wag * 6.0);

        // Уши прижимаются во сне и от досады, торчат при внимании.
        let target_droop = if activity == Activity::Asleep {
            1.0
        } else {
            mind.upset * 0.75
        };
        self.ear_droop += (target_droop - self.ear_droop) * (dt * 3.5).min(1.0);

        // A walking animal does not hold its tail like a rigid prop. Walking
        // produces a relaxed wag, while a sprint keeps it steadier for balance.
        let locomotion_wag: f32 = match gait {
            Gait::Stand => 0.0,
            Gait::Walk => 0.58,
            Gait::Run => 0.28,
        };
        let target_wag = (mind.cheer * 0.9 + mind.typing * 0.5)
            .min(1.0)
            .max(locomotion_wag);
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

        warp
    }

    /// Normalized tail phase shared by the front mesh and articulated profile
    /// tail. A slow envelope makes the wag come and go rather than ticking at a
    /// mechanical constant amplitude on every step.
    fn tail_swing(&self) -> f32 {
        let wave = (self.tail.sin() + (self.tail * 2.07 + 0.4).sin() * 0.22) / 1.22;
        let envelope = 0.55 + (self.tail * 0.23).sin() * 0.45;
        wave * self.wag * envelope
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
    orb: Particles,
}

impl Clouds {
    fn new(rig: &Rig) -> Self {
        let mut orb = Particles::with_capacity(ORB_PARTICLES, 0x0B1D_2E3F, rig.scale);
        let orb_half = rig.orb.width().max(rig.orb.height()) as f32 / 2.0;
        orb.seed_orbit(
            rig.orb_rest,
            orb_half * 1.05,
            orb_half * 1.8,
            ORB_ORBIT_COUNT,
        );
        Self { orb }
    }

    fn alive(&self) -> usize {
        self.orb.alive()
    }
}

fn run(initial_scale: f32, stdin_control: bool, walk_style: WalkStyle) -> Result<(), String> {
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
    let mut character_layers = CharacterLayers::new(&rig);
    let mut clouds = Clouds::new(&rig);
    let mut mood = Mood::new(0x1234_5678);
    let mut quads: Vec<Quad> = Vec::with_capacity(ORB_PARTICLES);
    // По умолчанию приложение полностью автономно. stdin активируется только
    // явным флагом и не связывает жизненный цикл с каким-либо другим продуктом.
    let events = stdin_control.then(Events::from_stdin);
    let mut inbox: Vec<Event> = Vec::new();
    let mut mind = Engagement::default();
    let mut body = Body::default();
    let mut walk = Walk {
        style: walk_style,
        ..Walk::default()
    };
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
        let mut travel = direction * gait.speed(width, walk.style) * dt;
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
        // Разлетевшиеся частицы сферы живут в экранных координатах. При
        // переезде окна компенсируем его движение, чтобы взрыв не ехал следом.
        let drift = previous_anchor - anchor;
        clouds.orb.shift(drift);

        let offset = orb_motion.update(dt, orbit_radius);
        let next_orb = rig.creature_centre + offset;
        // Якоря облака переезжают вместе с шариком, иначе оно отстанет. Во
        // время разлёта двигать нечего: частицы летят сами.
        clouds.orb.move_home(next_orb - orb_position);
        orb_position = next_orb;

        clouds.orb.update(dt);
        let orb_visible = orb_life.update(dt, &mut clouds.orb, orb_position, orb_body_radius);

        let gaze = (
            (to_target.x / (rig.canvas_size.0 as f32 * 0.6)).clamp(-1.0, 1.0),
            (to_target.y / (rig.canvas_size.1 as f32 * 0.6)).clamp(-1.0, 1.0),
        );
        mood.update(dt, gaze, activity, &mind);
        body.update(dt, &mind, activity, gait, hopper.squash);
        hands.update(dt, &mind, gait);

        // Свободные пиксельные частицы принадлежат только сфере. Вокруг тела
        // персонажа их нет: контактные эффекты ходьбы и бега будут отдельными
        // формами (пыль, линии скорости, ударные дуги), а не квадратным шумом.
        clouds.orb.set_brightness(1.0 + mind.voice * 0.5);

        compose(
            &mut canvas,
            &Frame {
                rig: &rig,
                mood: &mood,
                hopper: &hopper,
                warp: body.warp(hopper.squash),
                tail_swing: body.tail_swing(),
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
    /// Normalized, smoothly enveloped tail motion kept separate from the torso.
    tail_swing: f32,
    walk: &'a Walk,
    hands: &'a Hands,
    orientation: &'a Orientation,
    orb_position: Vec2,
    orb_visible: bool,
}

struct CharacterLayers {
    front: Canvas,
    profile: Canvas,
    profile_body: Canvas,
}

impl CharacterLayers {
    fn new(rig: &Rig) -> Self {
        Self {
            front: Canvas::new(rig.canvas_size.0, rig.canvas_size.1),
            profile: Canvas::new(rig.canvas_size.0, rig.canvas_size.1),
            profile_body: Canvas::new(
                rig.profile_right.creature.width(),
                rig.profile_right.creature.height(),
            ),
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

    clouds.orb.collect(quads);
    draw_layer(canvas, quads, false);
}

fn draw_character_transition(canvas: &mut Canvas, frame: &Frame<'_>, layers: &mut CharacterLayers) {
    let mix = frame.orientation.profile;
    if mix <= 0.001 {
        draw_front_character(canvas, frame);
    } else if mix >= 0.999 {
        draw_profile_character(canvas, frame, &mut layers.profile_body);
    } else {
        layers.front.clear();
        layers.profile.clear();
        draw_front_character(&mut layers.front, frame);
        draw_profile_character(&mut layers.profile, frame, &mut layers.profile_body);
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
        tail_swing,
        walk,
        hands,
        ..
    } = *frame;

    let lift = hopper.height + walk.body_lift(rig.creature.height() as f32 * 0.018);
    let sway = walk.body_sway(rig.creature.width() as f32 * 0.012);

    // Наклон корпуса по ходу движения. Верх уходит вперёд, низ остаётся на
    // ногах — именно этим бег отличается от быстрой ходьбы сильнее всего.
    // Смещение отрицательное, потому что сетка сдвигает точку выборки.
    let mut warp = front_tail_warp(warp, tail_swing);
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

/// The approved front master still contains its tail in the body painting.
/// Apply that local deformation only in the front pose; the profile uses a
/// dedicated articulated tail and must never inherit these right-side nodes.
fn front_tail_warp(mut warp: Warp, swing: f32) -> Warp {
    warp.add(3, 3, swing * 0.050, 0.0);
    warp.add(3, WARP_ROWS - 1, swing * 0.035, 0.0);
    warp
}

fn draw_profile_character(canvas: &mut Canvas, frame: &Frame<'_>, body_layer: &mut Canvas) {
    let rig = frame.rig;
    let pose = frame.orientation.profile_rig(rig);
    let facing_right = frame.orientation.facing > 0.0;
    let facing = if facing_right { 1.0 } else { -1.0 };
    let swagger = frame.walk.swagger();
    let leg_motions = [
        frame.walk.profile_leg_motion(0),
        frame.walk.profile_leg_motion(1),
    ];
    // Pelvis height is derived from the planted leg, not from an unrelated
    // sine. The same value is fed back into the IK target, locking the support
    // paw to the ground while the body transfers its weight above it.
    let pelvis = if pose.leg_chains.is_empty() {
        ProfilePelvis {
            horizontal: frame.walk.body_sway(pose.creature.width() as f32 * 0.014),
            lift: frame.walk.body_lift(pose.creature.height() as f32 * 0.022),
        }
    } else {
        profile_pelvis_for_walk(&pose.leg_chains, leg_motions, facing_right, frame.walk)
    };
    let leg_tracks = profile_leg_tracks(&pose.leg_chains, facing_right);
    let leg_stride = profile_common_stride(&pose.leg_chains, frame.walk);
    let lift = frame.hopper.height + pelvis.lift;
    let offset = Vec2::new(pelvis.horizontal, -lift);
    let body_warp = profile_body_warp(frame.warp, frame.walk, facing_right);
    let leg_margin = (pose.creature.height() as f32 * 0.12).ceil() as i32 + 2;

    if pose.leg_chains.is_empty() {
        // Compatibility with an older asset pack. New production manifests
        // always provide articulated legs and never enter this branch.
        let (far_angle, near_angle) = frame.walk.leg_angles(0.20);
        for (index, (sprite, pivot, position)) in pose.legs.iter().enumerate() {
            let angle = if index == 0 { far_angle } else { near_angle };
            canvas.draw_rotated(
                sprite,
                (position.x + offset.x) as i32,
                (position.y + offset.y) as i32,
                *pivot,
                angle,
                leg_margin,
            );
            if index == 0 {
                draw_profile_tail(canvas, pose, offset, &body_warp, frame, facing);
            }
        }
        if pose.legs.is_empty() {
            draw_profile_tail(canvas, pose, offset, &body_warp, frame, facing);
        }
    } else {
        // Far leg first, near leg second. The jacket overlaps both hip joints,
        // and the body is drawn afterwards, hiding the mechanical cut lines.
        for (index, chain) in pose.leg_chains.iter().enumerate() {
            draw_profile_leg_chain(
                canvas,
                chain,
                offset,
                ProfileLegFrame {
                    motion: leg_motions[index.min(leg_motions.len() - 1)],
                    facing_right,
                    run: frame.walk.run,
                    blend: frame.walk.blend,
                    pelvis,
                    track_x: leg_tracks[index.min(leg_tracks.len() - 1)],
                    stride: leg_stride,
                },
            );
            if index == 0 {
                draw_profile_tail(canvas, pose, offset, &body_warp, frame, facing);
            }
        }
    }

    // The torso follows the supporting leg, while shoulders counter-rotate.
    // A profile silhouette reads this as deliberate swagger instead of a
    // frozen sticker sliding across the desktop. The head moves less than the
    // shoulder row, so the character still appears self-assured and focused.
    // Arm swing stays subordinate to the ribcage twist below. A large
    // independent sleeve rotation reads as a detached arm on this flat rig.
    let shoulder_angle = swagger * facing * (0.11 + frame.walk.run * 0.05);
    let arm_margin = (pose.creature.height() as f32 * 0.15).ceil() as i32 + 2;
    let arm_angles = (shoulder_angle, -shoulder_angle);
    if let Some((sprite, pivot, position)) = &pose.arm_far {
        let attachment = warped_profile_attachment(
            &body_warp,
            pose.creature_origin,
            (pose.creature.width(), pose.creature.height()),
            *position,
            *pivot,
        );
        canvas.draw_rotated(
            sprite,
            (position.x + offset.x + attachment.x) as i32,
            (position.y + offset.y + attachment.y) as i32,
            *pivot,
            arm_angles.0,
            arm_margin,
        );
    }

    // The near sleeve is also drawn before the torso. The torso's retained
    // shoulder cap then overlaps the rotating sleeve and makes the two parts
    // one continuous silhouette instead of exposing a cut line.
    if let Some((sprite, pivot, position)) = &pose.arm_near {
        let attachment = warped_profile_attachment(
            &body_warp,
            pose.creature_origin,
            (pose.creature.width(), pose.creature.height()),
            *position,
            *pivot,
        );
        canvas.draw_rotated(
            sprite,
            (position.x + offset.x + attachment.x) as i32,
            (position.y + offset.y + attachment.y) as i32,
            *pivot,
            arm_angles.1,
            arm_margin,
        );
    }

    // Composite the expression into the torso before deformation. Previously
    // the body warped and the procedural face did not, which made the visor
    // appear to slide across the profile sprite during a step.
    body_layer.clear();
    body_layer.draw(&pose.creature, 0, 0);
    let face = pet_face::render_profile(&frame.mood.face, pose.visor, rig.scale, facing_right);
    body_layer.draw_rgba(
        face.data(),
        face.width(),
        face.height(),
        (pose.visor_origin.x - pose.creature_origin.x) as i32,
        (pose.visor_origin.y - pose.creature_origin.y) as i32,
    );
    let (extent_x, extent_y) = body_warp.extent();
    let body_margin = (extent_x.max(extent_y) * pose.creature.height() as f32).ceil() as i32 + 2;
    canvas.draw_canvas_warped(
        body_layer,
        (pose.creature_origin.x + offset.x) as i32,
        (pose.creature_origin.y + offset.y) as i32,
        &body_warp,
        body_margin,
    );
}

/// Draws the continuous tail between the far and near leg.  The root follows
/// the exact pelvis/torso attachment, while the tail itself rotates as one
/// piece; no body pixels are stretched to fake the wag.
fn draw_profile_tail(
    canvas: &mut Canvas,
    pose: &ProfileRig,
    offset: Vec2,
    body_warp: &Warp,
    frame: &Frame<'_>,
    facing: f32,
) {
    let Some((sprite, pivot, position)) = &pose.tail else {
        return;
    };
    let attachment = warped_profile_attachment(
        body_warp,
        pose.creature_origin,
        (pose.creature.width(), pose.creature.height()),
        *position,
        *pivot,
    );
    let angle = frame.tail_swing * facing * (0.18 + frame.walk.blend * 0.10);
    let margin = (pose.creature.height() as f32 * 0.16).ceil() as i32 + 2;
    canvas.draw_rotated(
        sprite,
        (position.x + offset.x + attachment.x) as i32,
        (position.y + offset.y + attachment.y) as i32,
        *pivot,
        angle,
        margin,
    );
}

/// Maps an articulated attachment through the exact torso deformation used
/// for rendering. This is the hierarchy boundary: a sleeve can rotate around
/// its shoulder, but the shoulder itself is owned by the ribcage and therefore
/// can never drift away from it.
fn warped_profile_attachment(
    warp: &Warp,
    body_origin: Vec2,
    body_size: (u32, u32),
    limb_origin: Vec2,
    limb_pivot: (f32, f32),
) -> Vec2 {
    let width = body_size.0.max(1) as f32;
    let height = body_size.1.max(1) as f32;
    let u = (limb_origin.x + limb_pivot.0 - body_origin.x) / width;
    let v = (limb_origin.y + limb_pivot.1 - body_origin.y) / height;
    let (du, dv) = warp.sample(u, v);
    Vec2::new(-du * width, -dv * height)
}

/// Builds the articulated upper-body pose from the same gait phase that drives
/// the feet. The body is one painted layer, so a shallow deformation down the
/// spine supplies the thorax/pelvis counter-rotation without separating the
/// head, jacket and shoulders into mechanical-looking pieces.
fn profile_body_warp(mut warp: Warp, walk: &Walk, facing_right: bool) -> Warp {
    let facing = if facing_right { 1.0 } else { -1.0 };
    let swagger = walk.swagger();

    // The hood and head occupy the first two control rows.  They are a single
    // painted solid, so unequal row offsets stretch the skull and upper back.
    // Copying the complete row preserves any ear expression but makes this
    // vertical band rigid before locomotion is layered on top.
    warp.copy_row(0, 1);

    let extension = walk.body_lift(1.0);
    let lean = walk.body_lean() * 0.035;
    let head_follow = swagger * facing * 0.010;
    let head_y = extension * 0.0105;
    for row in 0..=1 {
        warp.add_row(row, -head_follow - lean, head_y);
    }

    // Approximate a shallow 3D thorax turn below the neck.  No column-specific
    // twist touches the rigid head band; the two shoulders only diverge at the
    // chest row and settle again towards the pelvis.
    const SHOULDER_X: [f32; 4] = [0.028, 0.038, -0.038, -0.028];
    const SHOULDER_Y: [f32; 4] = [0.006, 0.009, -0.009, -0.006];
    for column in 0..4 {
        let dx = SHOULDER_X[column] * swagger;
        let dy = SHOULDER_Y[column] * swagger;
        warp.add(column, 2, -dx, -dy);
        warp.add(column, 3, -dx * 0.24, -dy * 0.20);
    }

    // Chest and pelvis follow the supporting leg as connected masses.  Their
    // smaller uniform translation joins the rigid head block to the articulated
    // lower body without introducing a rubber hinge at the neck.
    let chest_follow = swagger * facing;
    warp.add_row(2, -chest_follow * 0.003, 0.0);
    warp.add_row(3, chest_follow * 0.004, 0.0);

    // Root motion provides the main rise and fall. This tiny chest compression
    // adds follow-through while the head itself stays shape-preserving.
    warp.add_row(2, 0.0, extension * 0.0040);
    warp.add_row(2, -lean * 0.42, 0.0);
    warp
}

fn draw_profile_leg_chain(
    canvas: &mut Canvas,
    chain: &ProfileLegChain,
    offset: Vec2,
    frame: ProfileLegFrame,
) {
    let pose = solve_profile_leg(chain, frame);
    canvas.draw_skinned_limb(
        &chain.sprite,
        (chain.origin.x + offset.x).round() as i32,
        (chain.origin.y + offset.y).round() as i32,
        chain.hip,
        chain.knee,
        chain.ankle,
        pose.hip,
        pose.knee,
        pose.foot,
    );
}

/// Computes pelvis height from whichever paw currently carries weight.
///
/// The planted leg stays close to full extension. At contact the lateral
/// reach lowers the pelvis slightly; near mid-stance it rises above the
/// support paw. During double support we choose the lower valid pelvis height
/// so neither leg can be pulled beyond its anatomical reach.
fn profile_pelvis_motion(
    chains: &[ProfileLegChain],
    motions: [LegMotion; 2],
    facing_right: bool,
    walk: &Walk,
) -> ProfilePelvis {
    if walk.blend <= 1e-4 {
        return ProfilePelvis::default();
    }
    let facing = if facing_right { 1.0 } else { -1.0 };
    let tracks = profile_leg_tracks(chains, facing_right);
    let stride = profile_common_stride(chains, walk);
    let mut support_horizontal = 0.0_f32;
    let mut support_count = 0_u32;
    for index in 0..chains.len() {
        let motion = motions[index.min(motions.len() - 1)];
        if motion.grounded {
            support_horizontal += facing * motion.horizontal * stride;
            support_count += 1;
        }
    }
    // During double support the pelvis sits between the two planted paws.
    // With one support paw it keeps travelling over that leg instead of
    // snapping directly above it.
    let horizontal = if support_count > 1 {
        support_horizontal / support_count as f32 * walk.blend
    } else {
        0.0
    };
    let mut support_lift: Option<f32> = None;
    let mut reference_length = 0.0_f32;
    for (index, chain) in chains.iter().enumerate() {
        let motion = motions[index.min(motions.len() - 1)];
        let upper = point_distance(chain.hip, chain.knee);
        let lower = point_distance(chain.knee, chain.ankle);
        let length = upper + lower;
        reference_length = reference_length.max(length);
        if !motion.grounded {
            continue;
        }
        let gait_target_x =
            tracks[index.min(tracks.len() - 1)] + facing * motion.horizontal * stride;
        let target_x = mix(chain.ankle.0, gait_target_x, walk.blend) - horizontal;
        let horizontal = target_x - chain.hip.0;
        // Roughly 4-8 degrees of planted-knee flexion: enough to absorb the
        // contact without drawing a conspicuous crouched knee on the ground.
        let reach_ratio = 0.999 - motion.support_flex * 0.0015;
        // Stay below the solver's hard extension boundary. Otherwise IK would
        // shorten the requested target by a fraction of a pixel and the paw
        // would no longer be mathematically ground-locked.
        let reach = (length * reach_ratio).min(length - 0.30);
        let vertical = (reach * reach - horizontal * horizontal).max(0.0).sqrt();
        let neutral_vertical = chain.ankle.1 - chain.hip.1;
        let candidate = vertical - neutral_vertical;
        support_lift = Some(match support_lift {
            Some(current) => current.min(candidate),
            None => candidate,
        });
    }
    // The shared side-view track deliberately moves the painted three-quarter
    // hips towards one sagittal axis. A run landing has a wider reach and a
    // deeper loading response than a walk, so its anatomical root allowance is
    // larger. This is still root translation, not a backwards knee bend.
    let limit = reference_length * mix(0.12, 0.16, walk.run);
    let mut lift = support_lift.unwrap_or(0.0).clamp(-limit, limit) * walk.blend;
    if support_count == 0 {
        // During a true run both paws leave the desktop. The whole body follows
        // one compact arc; legs remain free to fold because no ground lock is
        // active in this interval.
        lift += reference_length * 0.075 * walk.run_flight_lift();
    }
    ProfilePelvis { horizontal, lift }
}

/// Couples lower- and upper-body motion. IK-derived pelvis displacement keeps
/// every support paw planted and naturally produces contact/down/passing/up;
/// a run-only aerial arc is added only while neither paw owns the ground.
fn profile_pelvis_for_walk(
    chains: &[ProfileLegChain],
    motions: [LegMotion; 2],
    facing_right: bool,
    walk: &Walk,
) -> ProfilePelvis {
    profile_pelvis_motion(chains, motions, facing_right, walk)
}

fn solve_profile_leg(chain: &ProfileLegChain, frame: ProfileLegFrame) -> SolvedLeg {
    let geometry = solve_profile_leg_geometry(chain, frame);
    let facing = if frame.facing_right { 1.0 } else { -1.0 };
    let source_thigh = point_angle(chain.knee.0 - chain.hip.0, chain.knee.1 - chain.hip.1);
    let source_shin = point_angle(chain.ankle.0 - chain.knee.0, chain.ankle.1 - chain.knee.1);
    let desired_thigh = point_angle(geometry.knee.0 - chain.hip.0, geometry.knee.1 - chain.hip.1);
    let desired_shin = point_angle(
        geometry.ankle.0 - geometry.knee.0,
        geometry.ankle.1 - geometry.knee.1,
    );
    let hip = normalize_angle(desired_thigh - source_thigh);
    let shin = normalize_angle(desired_shin - source_shin);
    SolvedLeg {
        hip,
        knee: normalize_angle(shin - hip),
        foot: -facing * frame.motion.toe_lift * (0.13 + frame.run * 0.05) * frame.blend,
    }
}

fn solve_profile_leg_geometry(chain: &ProfileLegChain, frame: ProfileLegFrame) -> LegGeometry {
    let upper = point_distance(chain.hip, chain.knee);
    let lower = point_distance(chain.knee, chain.ankle);
    let length = upper + lower;
    let facing = if frame.facing_right { 1.0 } else { -1.0 };
    let clearance = length * (0.19 + frame.run * 0.06) * frame.motion.lift * frame.blend;
    let gait_target_x = frame.track_x + facing * frame.motion.horizontal * frame.stride;
    let mut target = (
        mix(chain.ankle.0, gait_target_x, frame.blend) - frame.pelvis.horizontal,
        chain.ankle.1 + frame.pelvis.lift - clearance,
    );

    let mut direction = (target.0 - chain.hip.0, target.1 - chain.hip.1);
    let mut reach = direction.0.hypot(direction.1).max(1e-4);
    let minimum = (upper - lower).abs() + 0.25;
    let maximum = upper + lower - 0.25;
    if reach < minimum || reach > maximum {
        let clamped = reach.clamp(minimum, maximum);
        let scale = clamped / reach;
        direction.0 *= scale;
        direction.1 *= scale;
        target = (chain.hip.0 + direction.0, chain.hip.1 + direction.1);
        reach = clamped;
    }

    let unit = (direction.0 / reach, direction.1 / reach);
    let along = (upper * upper - lower * lower + reach * reach) / (2.0 * reach);
    let bend = (upper * upper - along * along).max(0.0).sqrt();
    // The only allowed branch puts the knee in front of the leg. There is no
    // second solution to interpolate through, so hyperextension backwards is
    // impossible even at the contact/swing boundary.
    let perpendicular = (unit.1 * facing, -unit.0 * facing);
    let desired_knee = (
        chain.hip.0 + unit.0 * along + perpendicular.0 * bend,
        chain.hip.1 + unit.1 * along + perpendicular.1 * bend,
    );
    debug_assert!(
        cross_product(
            direction,
            (desired_knee.0 - chain.hip.0, desired_knee.1 - chain.hip.1)
        ) * facing
            <= 1e-3,
        "profile IK selected a backwards knee branch"
    );

    LegGeometry {
        knee: desired_knee,
        ankle: target,
    }
}

fn point_distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Both feet in a side-view gait use the exact same sagittal track. Depth is
/// conveyed by draw order, never by shortening or shifting the far step: an
/// x-offset here made that leg appear to limp even though its phase was valid.
fn profile_leg_tracks(chains: &[ProfileLegChain], _facing_right: bool) -> [f32; 2] {
    if chains.is_empty() {
        return [0.0, 0.0];
    }
    let count = chains.len().min(2) as f32;
    let centre = chains
        .iter()
        .take(2)
        .map(|chain| chain.origin.x + chain.hip.0)
        .sum::<f32>()
        / count;
    [
        centre - chains[0].origin.x,
        centre
            - chains
                .get(1)
                .map_or(chains[0].origin.x, |chain| chain.origin.x),
    ]
}

fn profile_common_stride(chains: &[ProfileLegChain], walk: &Walk) -> f32 {
    if chains.is_empty() {
        return 0.0;
    }
    let count = chains.len().min(2) as f32;
    let mean_length = chains
        .iter()
        .take(2)
        .map(|chain| {
            point_distance(chain.hip, chain.knee) + point_distance(chain.knee, chain.ankle)
        })
        .sum::<f32>()
        / count;
    // A contact pose needs visible separation between the two large boots.
    // Both legs deliberately use this one mean length, otherwise small source
    // rig differences turn into a visible asymmetric stride.
    mean_length * mix(walk.style.profile().stride_scale, 0.58, walk.run)
}

fn point_angle(x: f32, y: f32) -> f32 {
    y.atan2(x)
}

fn cross_product(a: (f32, f32), b: (f32, f32)) -> f32 {
    a.0 * b.1 - a.1 * b.0
}

fn normalize_angle(mut angle: f32) -> f32 {
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    while angle < -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    angle
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
        let mut character_layers = CharacterLayers::new(&rig);
        compose(
            &mut canvas,
            &Frame {
                rig: &rig,
                mood: &mood,
                hopper: &Hopper::new(1),
                warp: body.warp(1.0),
                tail_swing: body.tail_swing(),
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
fn dump_walk(path: &Path, scale: f32, walk_style: WalkStyle) -> Result<(), String> {
    let assets = Assets::load()?;
    let rig = assets.build(scale.clamp(MIN_SCALE, MAX_SCALE));
    if rig.profile_right.legs.is_empty() && rig.profile_right.leg_chains.is_empty() {
        return Err("профильная поза не содержит ног: ходьбу показать нечем".to_owned());
    }
    const FRAMES: usize = 8;
    let mut rows = Vec::new();
    for gait in [Gait::Walk, Gait::Run] {
        let mut tiles = Vec::new();
        for frame in 0..FRAMES {
            // Exact evenly-spaced phases make contact/down/passing/up directly
            // comparable and keep the QA sheet deterministic.
            let run = if gait == Gait::Run { 1.0 } else { 0.0 };
            let walk = Walk {
                phase: frame as f32 / FRAMES as f32 * std::f32::consts::TAU,
                blend: 1.0,
                run,
                lean: 0.30 + run * 0.70,
                style: walk_style,
            };

            let mut canvas = Canvas::new(rig.canvas_size.0, rig.canvas_size.1);
            let mut clouds = Clouds::new(&rig);
            let mood = Mood::new(0x1234_5678);
            let mut quads = Vec::new();
            let orientation = Orientation {
                profile: 1.0,
                facing: 1.0,
            };
            let mut character_layers = CharacterLayers::new(&rig);
            compose(
                &mut canvas,
                &Frame {
                    rig: &rig,
                    mood: &mood,
                    hopper: &Hopper::new(1),
                    warp: Warp::identity(),
                    tail_swing: (walk.phase * 1.37).sin() * 0.48,
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
        "полоса ходьбы: {} ({walk_style:?}, шаг и бег, по {FRAMES} кадров)",
        path.display(),
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
        let mut character_layers = CharacterLayers::new(&rig);
        compose(
            &mut canvas,
            &Frame {
                rig: &rig,
                mood: &mood,
                hopper: &Hopper::new(1),
                warp: Warp::identity(),
                tail_swing: 0.0,
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

#[cfg(test)]
mod gait_tests {
    use super::*;

    fn joint_flexion(chain: &ProfileLegChain, geometry: LegGeometry) -> f32 {
        let thigh = point_angle(geometry.knee.0 - chain.hip.0, geometry.knee.1 - chain.hip.1);
        let shin = point_angle(
            geometry.ankle.0 - geometry.knee.0,
            geometry.ankle.1 - geometry.knee.1,
        );
        normalize_angle(shin - thigh).abs()
    }

    #[test]
    fn profile_step_locks_stance_and_lifts_swing() {
        let mut walk = Walk {
            blend: 1.0,
            ..Walk::default()
        };

        walk.phase = 0.0;
        let contact = walk.profile_leg_motion(0);
        assert!(contact.grounded);
        assert_eq!(contact.lift, 0.0);
        assert!((contact.horizontal - 0.5).abs() < 1e-6);

        walk.phase = std::f32::consts::TAU * 0.824;
        let passing = walk.profile_leg_motion(0);
        assert!(!passing.grounded);
        assert!(passing.lift > 0.99);
        assert!(passing.horizontal > 0.1 && passing.horizontal < 0.3);

        walk.phase = std::f32::consts::TAU * 0.999;
        let landing = walk.profile_leg_motion(0);
        assert!(!landing.grounded);
        assert!(landing.lift < 0.01);
        assert!(landing.horizontal > 0.49);
    }

    #[test]
    fn woodland_walk_holds_support_longer_and_swings_lower() {
        assert_eq!(WalkStyle::parse("woods"), Some(WalkStyle::Woodland));

        let confident = Walk {
            phase: std::f32::consts::TAU * 0.62,
            blend: 1.0,
            style: WalkStyle::Confident,
            ..Walk::default()
        };
        let woodland = Walk {
            style: WalkStyle::Woodland,
            ..confident
        };
        assert!(!confident.profile_leg_motion(0).grounded);
        assert!(woodland.profile_leg_motion(0).grounded);

        let confident = Walk {
            phase: std::f32::consts::TAU * 0.82,
            ..confident
        };
        let woodland = Walk {
            style: WalkStyle::Woodland,
            ..confident
        };
        assert!(woodland.profile_leg_motion(0).lift < confident.profile_leg_motion(0).lift * 0.85);
    }

    #[test]
    fn running_has_a_true_aerial_phase() {
        let assets = Assets::load().expect("production assets load");
        let rig = assets.build(0.22);
        let walk = Walk {
            phase: std::f32::consts::TAU * 0.43,
            blend: 1.0,
            run: 1.0,
            lean: 1.0,
            ..Walk::default()
        };
        let motions = [walk.profile_leg_motion(0), walk.profile_leg_motion(1)];
        assert!(motions.iter().all(|motion| !motion.grounded));
        assert!(walk.run_flight_lift() > 0.95);

        let pelvis = profile_pelvis_for_walk(&rig.profile_right.leg_chains, motions, true, &walk);
        assert!(pelvis.lift > 0.0, "run flight did not lift the body root");
    }

    #[test]
    fn run_contacts_stay_locked_between_aerial_phases() {
        let assets = Assets::load().expect("production assets load");
        let rig = assets.build(0.22);
        let pose = &rig.profile_right;
        let tracks = profile_leg_tracks(&pose.leg_chains, true);
        let mut flight_samples = 0;

        for sample in 0..240 {
            let walk = Walk {
                phase: sample as f32 / 240.0 * std::f32::consts::TAU,
                blend: 1.0,
                run: 1.0,
                lean: 1.0,
                ..Walk::default()
            };
            let motions = [walk.profile_leg_motion(0), walk.profile_leg_motion(1)];
            if motions.iter().all(|motion| !motion.grounded) {
                flight_samples += 1;
            }
            let pelvis = profile_pelvis_for_walk(&pose.leg_chains, motions, true, &walk);
            let stride = profile_common_stride(&pose.leg_chains, &walk);
            for (index, chain) in pose.leg_chains.iter().enumerate() {
                if !motions[index].grounded {
                    continue;
                }
                let geometry = solve_profile_leg_geometry(
                    chain,
                    ProfileLegFrame {
                        motion: motions[index],
                        facing_right: true,
                        run: walk.run,
                        blend: walk.blend,
                        pelvis,
                        track_x: tracks[index],
                        stride,
                    },
                );
                let world_y = chain.origin.y - pelvis.lift + geometry.ankle.1;
                let ground_y = chain.origin.y + chain.ankle.1;
                assert!(
                    (world_y - ground_y).abs() < 0.05,
                    "sample {sample} leg {index} moved run contact by {:.3}px",
                    world_y - ground_y
                );
            }
        }

        assert!(
            (48..=80).contains(&flight_samples),
            "unexpected flight duration: {flight_samples}/240 samples"
        );
    }

    #[test]
    fn profile_ik_never_reverses_knee_and_keeps_support_paw_grounded() {
        let assets = Assets::load().expect("production assets load");
        let rig = assets.build(0.22);
        for (pose, facing_right) in [(&rig.profile_right, true), (&rig.profile_left, false)] {
            let facing = if facing_right { 1.0 } else { -1.0 };
            let stride_walk = Walk {
                blend: 1.0,
                ..Walk::default()
            };
            let stride = profile_common_stride(&pose.leg_chains, &stride_walk);
            for sample in 0..240 {
                let walk = Walk {
                    phase: sample as f32 / 240.0 * std::f32::consts::TAU,
                    blend: 1.0,
                    run: 0.0,
                    lean: 0.3 * facing,
                    style: WalkStyle::default(),
                };
                let motions = [walk.profile_leg_motion(0), walk.profile_leg_motion(1)];
                let pelvis =
                    profile_pelvis_for_walk(&pose.leg_chains, motions, facing_right, &walk);
                let tracks = profile_leg_tracks(&pose.leg_chains, facing_right);
                for (index, chain) in pose.leg_chains.iter().enumerate() {
                    let motion = motions[index.min(1)];
                    let geometry = solve_profile_leg_geometry(
                        chain,
                        ProfileLegFrame {
                            motion,
                            facing_right,
                            run: walk.run,
                            blend: walk.blend,
                            pelvis,
                            track_x: tracks[index.min(1)],
                            stride,
                        },
                    );
                    let leg_axis = (
                        geometry.ankle.0 - chain.hip.0,
                        geometry.ankle.1 - chain.hip.1,
                    );
                    let knee_axis = (geometry.knee.0 - chain.hip.0, geometry.knee.1 - chain.hip.1);
                    assert!(
                        cross_product(leg_axis, knee_axis) * facing <= 1e-3,
                        "sample {sample} selected a backwards knee"
                    );
                    if motion.grounded {
                        let world_ankle = chain.origin.y - pelvis.lift + geometry.ankle.1;
                        let neutral_ground = chain.origin.y + chain.ankle.1;
                        assert!(
                            (world_ankle - neutral_ground).abs() < 0.05,
                            "sample {sample} moved a planted paw by {} px",
                            world_ankle - neutral_ground
                        );
                        let expected_x = chain.origin.x
                            + tracks[index.min(1)]
                            + facing * motion.horizontal * stride;
                        let world_x = chain.origin.x + pelvis.horizontal + geometry.ankle.0;
                        assert!((world_x - expected_x).abs() < 0.05);
                        let flexion = joint_flexion(chain, geometry);
                        assert!(
                            flexion < 0.38,
                            "sample {sample} leg {index} exposed {flexion:.3} rad of crouched knee flexion on the ground"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn lifted_paw_raises_and_flexes_the_knee() {
        let assets = Assets::load().expect("production assets load");
        let rig = assets.build(0.22);
        let chain = &rig.profile_right.leg_chains[0];

        let contact_walk = Walk {
            phase: 0.0,
            blend: 1.0,
            ..Walk::default()
        };
        let contact_motions = [
            contact_walk.profile_leg_motion(0),
            contact_walk.profile_leg_motion(1),
        ];
        let contact_pelvis = profile_pelvis_for_walk(
            &rig.profile_right.leg_chains,
            contact_motions,
            true,
            &contact_walk,
        );
        let tracks = profile_leg_tracks(&rig.profile_right.leg_chains, true);
        let stride = profile_common_stride(&rig.profile_right.leg_chains, &contact_walk);
        let contact = solve_profile_leg_geometry(
            chain,
            ProfileLegFrame {
                motion: contact_motions[0],
                facing_right: true,
                run: 0.0,
                blend: 1.0,
                pelvis: contact_pelvis,
                track_x: tracks[0],
                stride,
            },
        );

        let passing_walk = Walk {
            phase: std::f32::consts::TAU * 0.824,
            blend: 1.0,
            ..Walk::default()
        };
        let passing_motions = [
            passing_walk.profile_leg_motion(0),
            passing_walk.profile_leg_motion(1),
        ];
        let passing_pelvis = profile_pelvis_for_walk(
            &rig.profile_right.leg_chains,
            passing_motions,
            true,
            &passing_walk,
        );
        let passing = solve_profile_leg_geometry(
            chain,
            ProfileLegFrame {
                motion: passing_motions[0],
                facing_right: true,
                run: 0.0,
                blend: 1.0,
                pelvis: passing_pelvis,
                track_x: tracks[0],
                stride,
            },
        );

        assert!(passing.ankle.1 < contact.ankle.1);
        assert!(passing.knee.1 < contact.knee.1);
        assert!(joint_flexion(chain, passing) > joint_flexion(chain, contact) + 0.25);
    }

    #[test]
    fn each_swing_paw_advances_past_the_support_paw() {
        let assets = Assets::load().expect("production assets load");
        let rig = assets.build(0.22);
        for (pose, facing_right) in [(&rig.profile_right, true), (&rig.profile_left, false)] {
            let facing = if facing_right { 1.0 } else { -1.0 };
            let tracks = profile_leg_tracks(&pose.leg_chains, facing_right);
            let stride_walk = Walk {
                blend: 1.0,
                ..Walk::default()
            };
            let stride = profile_common_stride(&pose.leg_chains, &stride_walk);
            let world_tracks = [
                pose.leg_chains[0].origin.x + tracks[0],
                pose.leg_chains[1].origin.x + tracks[1],
            ];
            assert!(
                (world_tracks[0] - world_tracks[1]).abs() < 0.01,
                "far and near feet do not share one sagittal track"
            );
            let mut advances = [0.0_f32; 2];
            for (swing_index, cycle) in [(0, 0.90_f32), (1, 0.40_f32)] {
                let walk = Walk {
                    phase: std::f32::consts::TAU * cycle,
                    blend: 1.0,
                    ..Walk::default()
                };
                let motions = [walk.profile_leg_motion(0), walk.profile_leg_motion(1)];
                let support_index = 1 - swing_index;
                assert!(!motions[swing_index].grounded);
                assert!(motions[support_index].grounded);
                let pelvis =
                    profile_pelvis_for_walk(&pose.leg_chains, motions, facing_right, &walk);
                let ankle_x = |index: usize| {
                    let geometry = solve_profile_leg_geometry(
                        &pose.leg_chains[index],
                        ProfileLegFrame {
                            motion: motions[index],
                            facing_right,
                            run: 0.0,
                            blend: 1.0,
                            pelvis,
                            track_x: tracks[index],
                            stride,
                        },
                    );
                    pose.leg_chains[index].origin.x + pelvis.horizontal + geometry.ankle.0
                };
                let advance = (ankle_x(swing_index) - ankle_x(support_index)) * facing;
                advances[swing_index] = advance;
                assert!(
                    advance > stride * 0.18,
                    "swing leg {swing_index} stayed behind support leg by {:.2}px",
                    -advance
                );
            }
            assert!(
                (advances[0] - advances[1]).abs() < 0.05,
                "asymmetric stride: far={:.2}px near={:.2}px",
                advances[0],
                advances[1]
            );
        }
    }

    #[test]
    fn profile_head_back_and_shoulders_follow_the_step() {
        let assets = Assets::load().expect("production assets load");
        let rig = assets.build(0.22);
        let pose = &rig.profile_right;
        let body_size = (pose.creature.width(), pose.creature.height());

        let contact = Walk {
            phase: 0.0,
            blend: 1.0,
            ..Walk::default()
        };
        let up = Walk {
            phase: std::f32::consts::TAU * 0.375,
            blend: 1.0,
            ..Walk::default()
        };
        let world_y = |walk: &Walk, row: f32| {
            let motions = [walk.profile_leg_motion(0), walk.profile_leg_motion(1)];
            let pelvis = profile_pelvis_for_walk(&pose.leg_chains, motions, true, walk);
            let warp = profile_body_warp(Warp::identity(), walk, true);
            let (_, inverse_dy) = warp.sample(0.5, row);
            -pelvis.lift - inverse_dy * body_size.1 as f32
        };

        let head_travel = (world_y(&up, 0.08) - world_y(&contact, 0.08)).abs();
        let back_travel = (world_y(&up, 0.36) - world_y(&contact, 0.36)).abs();
        assert!(
            head_travel > body_size.1 as f32 * 0.014,
            "head stayed fixed: travel={head_travel:.2}px"
        );
        assert!(
            back_travel > body_size.1 as f32 * 0.012,
            "back stayed fixed: travel={back_travel:.2}px"
        );

        let swagger = Walk {
            phase: std::f32::consts::FRAC_PI_2,
            blend: 1.0,
            ..Walk::default()
        };
        let warp = profile_body_warp(Warp::identity(), &swagger, true);
        let visible = |u: f32, v: f32| {
            let (dx, dy) = warp.sample(u, v);
            Vec2::new(-dx * body_size.0 as f32, -dy * body_size.1 as f32)
        };
        // The hood/head band is rigid: every point at its top and base receives
        // the exact same translation, so the skull cannot rubber-stretch.
        for u in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let head_top = visible(u, 0.0);
            let head_base = visible(u, 0.25);
            assert!((head_top.x - head_base.x).abs() < 0.01);
            assert!((head_top.y - head_base.y).abs() < 0.01);
        }

        // Shoulder counter-rotation starts below the rigid head band.
        let near_shoulder = visible(0.38, 0.50);
        let far_shoulder = visible(0.62, 0.50);
        assert!(
            near_shoulder.x * far_shoulder.x < 0.0,
            "shoulders did not rotate in opposite directions"
        );
        assert!(
            near_shoulder.y * far_shoulder.y < 0.0,
            "shoulder line did not move diagonally"
        );
        assert!(near_shoulder.x.abs() > body_size.0 as f32 * 0.012);
        assert!(far_shoulder.x.abs() > body_size.0 as f32 * 0.012);
        let head = visible(0.5, 0.0);
        let pelvis = visible(0.5, 0.75);
        assert!(head.x * pelvis.x < 0.0, "pelvis did not counter-rotate");
    }

    #[test]
    fn walking_drives_a_smooth_non_constant_tail_wag() {
        let mut body = Body::default();
        let mind = Engagement::default();
        let mut minimum = f32::INFINITY;
        let mut maximum = f32::NEG_INFINITY;
        for _ in 0..240 {
            body.update(1.0 / 60.0, &mind, Activity::Attentive, Gait::Walk, 1.0);
            let swing = body.tail_swing();
            minimum = minimum.min(swing);
            maximum = maximum.max(swing);
        }
        assert!(body.wag > 0.50, "walk did not raise wag amplitude");
        assert!(maximum > 0.20 && minimum < -0.20);
        assert!(maximum - minimum < 1.5, "tail motion is too violent");
    }

    #[test]
    fn shoulder_attachment_uses_the_exact_torso_warp() {
        let mut warp = Warp::identity();
        warp.add_row(2, -0.10, 0.05);
        let attachment = warped_profile_attachment(
            &warp,
            Vec2::new(100.0, 50.0),
            (200, 400),
            Vec2::new(120.0, 100.0),
            (80.0, 150.0),
        );
        assert!((attachment.x - 20.0).abs() < 1e-4);
        assert!((attachment.y + 20.0).abs() < 1e-4);
    }
}
