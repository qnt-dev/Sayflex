//! The existing desktop-pet orb as a reusable actor.
//!
//! This module deliberately preserves the motion, particle and life-cycle
//! constants that originally lived in `pet_host`. Hosts provide only the
//! sprite and scene anchor; no host is allowed to substitute another drawing.

use pet_particles::{Particles, Quad, Rng, Vec2};
use pet_render::{Canvas, Sprite};

pub const PARTICLE_CAPACITY: usize = 320;
pub const ORBIT_PARTICLE_COUNT: usize = 20;
pub const BURST_FRAGMENT_COUNT: usize = 128;
const BURST_SPEED_MIN: f32 = 10.5;
const BURST_SPEED_MAX: f32 = 16.0;
const BURST_CHAMBER_RADIUS: f32 = 9.0;

/// The complete sphere actor: autonomous motion, orbiting particles and the
/// periodic burst/reassembly scene used by the desktop pet.
pub struct OrbActor {
    particles: Particles,
    quads: Vec<Quad>,
    motion: OrbMotion,
    life: OrbLife,
    position: Vec2,
    body_radius: f32,
    motion_paused: bool,
    dragging: bool,
    drag_tail_active: bool,
    expression: OrbExpression,
}

impl OrbActor {
    #[must_use]
    pub fn new(rest: Vec2, motion_radius: f32, body_radius: f32, scale: f32) -> Self {
        let mut particles = Particles::with_capacity(PARTICLE_CAPACITY, 0x0B1D_2E3F, scale);
        particles.seed_orbit(
            rest,
            body_radius * 1.05,
            body_radius * 1.8,
            ORBIT_PARTICLE_COUNT,
        );
        Self {
            particles,
            quads: Vec::with_capacity(PARTICLE_CAPACITY),
            motion: OrbMotion::new(0x00C0_FFEE, motion_radius),
            life: OrbLife::new(0x0DEF_ACED),
            position: rest,
            body_radius,
            motion_paused: false,
            dragging: false,
            drag_tail_active: false,
            expression: OrbExpression::default(),
        }
    }

    pub fn update(&mut self, dt: f32, anchor: Vec2, brightness: f32) {
        let next = if self.life.free_flight() && !self.motion_paused {
            anchor + self.motion.update(dt)
        } else {
            self.position
        };
        self.particles.move_home(next - self.position);
        self.position = next;
        self.particles.set_brightness(brightness);
        if self.dragging
            && self.drag_tail_active
            && !self
                .particles
                .advance_drag_tail(self.position, dt, self.body_radius * 13.0)
        {
            self.drag_tail_active = false;
        }
        self.particles.update(dt);
        if !self.motion_paused {
            self.life
                .update(dt, &mut self.particles, self.position, self.body_radius);
        }
        self.expression.update(dt);
    }

    /// Keeps burst and trail particles fixed in screen space while the host
    /// window moves. Orbit particles remain attached to the actor.
    pub fn shift_world(&mut self, delta: Vec2) {
        self.particles.shift(delta);
    }

    pub fn draw_behind(&mut self, canvas: &mut Canvas) {
        self.particles.collect(&mut self.quads);
        draw_layer(canvas, &self.quads, true);
    }

    pub fn draw_sprite(&self, canvas: &mut Canvas, growth_frames: &[Sprite]) {
        if growth_frames.is_empty() {
            return;
        }
        let opacity = self.life.opacity();
        if opacity > 0.0 {
            let growth = self.life.appearance();
            let frame_index = (growth * (growth_frames.len() - 1) as f32).round() as usize;
            let sprite = &growth_frames[frame_index.min(growth_frames.len() - 1)];
            let x = (self.position.x - sprite.width() as f32 / 2.0) as i32;
            let y = (self.position.y - sprite.height() as f32 / 2.0) as i32;
            if opacity >= 0.995 && growth >= 0.98 {
                canvas.draw_rotated(
                    sprite,
                    x,
                    y,
                    (sprite.width() as f32 / 2.0, sprite.height() as f32 / 2.0),
                    self.expression.tilt(),
                    5,
                );
                self.expression.draw_face(canvas, sprite, self.position);
            } else {
                canvas.draw_opacity(sprite, x, y, opacity);
            }
        }
    }

    pub fn draw_front(&mut self, canvas: &mut Canvas) {
        self.particles.collect(&mut self.quads);
        draw_layer(canvas, &self.quads, false);
    }

    /// Brief pressure-release spark between the final hot point and the first
    /// growth frame. It is drawn from two cheap premultiplied ellipses and
    /// allocates nothing per frame.
    pub fn draw_flash(&self, canvas: &mut Canvas) {
        let amount = self.life.flash_amount();
        if amount <= 0.0 {
            return;
        }
        let colour = |red: u8, green: u8, blue: u8, alpha: f32| -> [u8; 4] {
            let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
            let premultiply = |channel: u8| -> u8 { ((u32::from(channel) * a + 127) / 255) as u8 };
            [
                premultiply(blue),
                premultiply(green),
                premultiply(red),
                a as u8,
            ]
        };
        let outer = self.body_radius * (0.18 + amount * 0.85);
        canvas.fill_ellipse(
            self.position.x,
            self.position.y,
            outer,
            outer,
            colour(117, 88, 255, amount * 0.34),
        );
        let core = self.body_radius * (0.08 + amount * 0.24);
        canvas.fill_ellipse(
            self.position.x,
            self.position.y,
            core,
            core,
            colour(232, 250, 255, amount * 0.92),
        );
    }

    #[must_use]
    pub fn alive_particles(&self) -> usize {
        self.particles.alive()
    }

    #[must_use]
    pub const fn position(&self) -> Vec2 {
        self.position
    }

    #[must_use]
    pub const fn body_radius(&self) -> f32 {
        self.body_radius
    }

    pub fn set_motion_paused(&mut self, paused: bool) {
        self.motion_paused = paused;
    }

    /// Puts the orb into the fixed recording pose. The desktop companion's
    /// free-flight orbit is intentionally not reused here: recording has a
    /// stable visual anchor and the host draws its audio-reactive satellites
    /// separately.
    pub fn enter_recording_pose(&mut self, position: Vec2) {
        self.position = position;
        self.motion_paused = true;
        self.dragging = false;
        self.drag_tail_active = false;
        self.particles.clear();
        self.life.reset_whole();
        self.expression.set_gaze(Vec2::default());
    }

    /// Restores the normal desktop-pet pose after recording ends.
    pub fn resume_home_pose(&mut self, position: Vec2) {
        self.position = position;
        self.motion_paused = false;
        self.dragging = false;
        self.drag_tail_active = false;
        self.particles.reseed_orbit(
            position,
            self.body_radius * 1.05,
            self.body_radius * 1.8,
            ORBIT_PARTICLE_COUNT,
        );
        self.life.reset_whole();
    }

    /// Updates the direction the face is interested in. Components are
    /// normalized screen-space offsets in the range -1..1.
    pub fn look_towards(&mut self, direction: Vec2) {
        self.expression.set_gaze(direction);
    }

    /// Starts a modifier drag. The existing orbit cloud becomes the trail;
    /// no second cloud is spawned on top of it.
    pub fn begin_drag(&mut self) {
        self.motion_paused = true;
        self.dragging = true;
        self.drag_tail_active = false;
    }

    /// Keeps the orbit squares on the real screen-space path while the native
    /// window moves and arranges those same squares into two woven strands.
    pub fn drag_window(&mut self, movement: Vec2) {
        if movement.length() <= f32::EPSILON {
            return;
        }
        if !self.drag_tail_active {
            self.particles.begin_drag_tail(self.position);
            self.drag_tail_active = true;
        }
        self.particles.drag_tail(self.position, movement, 6.5);
        self.expression.push_drag(movement);
    }

    /// Releases the sphere and lets its squares flow back into orbit.
    pub fn end_drag(&mut self) {
        if self.drag_tail_active {
            self.particles.end_drag_tail(self.position);
        }
        self.dragging = false;
        self.drag_tail_active = false;
        self.motion_paused = false;
    }

    /// The image itself can be grabbed, but the invisible reassembly scene
    /// cannot. This keeps the modifier gesture spatially honest.
    #[must_use]
    pub fn is_grabbable(&self) -> bool {
        self.life.opacity() >= 0.98
    }
}

struct OrbExpression {
    elapsed: f32,
    gaze: Vec2,
    gaze_target: Vec2,
    drag_lean: Vec2,
    blink_in: f32,
    blink_elapsed: Option<f32>,
}

impl Default for OrbExpression {
    fn default() -> Self {
        Self {
            elapsed: 0.0,
            gaze: Vec2::default(),
            gaze_target: Vec2::default(),
            drag_lean: Vec2::default(),
            blink_in: 2.35,
            blink_elapsed: None,
        }
    }
}

impl OrbExpression {
    fn set_gaze(&mut self, direction: Vec2) {
        let length = direction.length().max(1.0);
        self.gaze_target = Vec2::new(
            (direction.x / length).clamp(-1.0, 1.0),
            (direction.y / length).clamp(-1.0, 1.0),
        );
    }

    fn push_drag(&mut self, movement: Vec2) {
        let length = movement.length().max(1.0);
        self.drag_lean = Vec2::new(
            (movement.x / length).clamp(-1.0, 1.0),
            (movement.y / length).clamp(-1.0, 1.0),
        );
    }

    fn update(&mut self, dt: f32) {
        self.elapsed += dt;
        let gaze_response = 1.0 - (-dt * 7.5).exp();
        self.gaze = self.gaze + (self.gaze_target - self.gaze) * gaze_response;
        self.drag_lean = self.drag_lean * (-dt * 4.6).exp();

        if let Some(elapsed) = &mut self.blink_elapsed {
            *elapsed += dt;
            if *elapsed >= 0.22 {
                self.blink_elapsed = None;
                let variation = (self.elapsed * 1.618).sin().abs();
                self.blink_in = 2.4 + variation * 3.6;
            }
        } else {
            self.blink_in -= dt;
            if self.blink_in <= 0.0 {
                self.blink_elapsed = Some(0.0);
            }
        }
    }

    fn blink_amount(&self) -> f32 {
        let Some(elapsed) = self.blink_elapsed else {
            return 0.0;
        };
        let progress = (elapsed / 0.22).clamp(0.0, 1.0);
        if progress < 0.42 {
            progress / 0.42
        } else {
            (1.0 - progress) / 0.58
        }
    }

    fn tilt(&self) -> f32 {
        let idle = (self.elapsed * 0.72).sin() * 0.018;
        (idle + self.gaze.x * 0.035 + self.drag_lean.x * 0.075).clamp(-0.11, 0.11)
    }

    fn draw_face(&self, canvas: &mut Canvas, sprite: &Sprite, center: Vec2) {
        let width = sprite.width() as f32;
        let height = sprite.height() as f32;
        let top_left = center - Vec2::new(width / 2.0, height / 2.0);
        let blink = self.blink_amount();
        // The accepted orb has no pupils: its two cyan eye-arcs move as
        // complete shapes. Keeping the offset small preserves the original
        // sideways expression instead of turning it into conventional eyes.
        let look = Vec2::new(self.gaze.x * 1.65, self.gaze.y * 0.82);
        let angle = self.tilt();

        let eyes = [
            (Vec2::new(width * 0.19, height * 0.445), (61, 41, 132)),
            (Vec2::new(width * 0.535, height * 0.41), (47, 33, 113)),
        ];
        for (local, background) in eyes {
            let resting_eye = rotate_local(local, width, height, angle) + top_left;
            canvas.fill_ellipse(
                resting_eye.x,
                resting_eye.y,
                width * 0.116,
                height * 0.118,
                premultiplied(background.0, background.1, background.2, 0.96),
            );

            let eye = resting_eye + look;
            let openness = 1.0 - blink;
            if openness <= 0.16 {
                canvas.fill_ellipse(
                    eye.x,
                    eye.y,
                    width * 0.082,
                    0.55,
                    premultiplied(102, 244, 250, 0.94),
                );
                continue;
            }

            // Outer cyan oval minus a smaller, slightly lowered oval gives
            // the same upward eye-arc as the source image. There is
            // deliberately no dark pupil.
            let eye_rx = width * 0.087;
            let eye_ry = 0.58 + height * 0.068 * openness;
            canvas.fill_ellipse(
                eye.x,
                eye.y,
                eye_rx,
                eye_ry,
                premultiplied(105, 244, 250, 0.96),
            );
            canvas.fill_ellipse(
                eye.x,
                eye.y + eye_ry * 0.54,
                (eye_rx * 0.64).max(0.8),
                (eye_ry * 0.62).max(0.6),
                premultiplied(background.0, background.1, background.2, 0.98),
            );
        }
    }
}

fn rotate_local(point: Vec2, width: f32, height: f32, angle: f32) -> Vec2 {
    let pivot = Vec2::new(width / 2.0, height / 2.0);
    let relative = point - pivot;
    let (sin, cos) = angle.sin_cos();
    pivot
        + Vec2::new(
            relative.x * cos - relative.y * sin,
            relative.x * sin + relative.y * cos,
        )
}

fn premultiplied(red: u8, green: u8, blue: u8, alpha: f32) -> [u8; 4] {
    let alpha = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
    let scale = |channel: u8| ((u32::from(channel) * alpha + 127) / 255) as u8;
    [scale(blue), scale(green), scale(red), alpha as u8]
}

/// Free motion around the owner, copied from the accepted desktop-pet actor.
struct OrbMotion {
    angle: f32,
    speed: f32,
    target_speed: f32,
    radius: f32,
    target_radius: f32,
    height: f32,
    target_height: f32,
    retarget_in: f32,
    base_radius: f32,
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
            base_radius: radius,
            rng: Rng::new(seed),
        }
    }

    fn update(&mut self, dt: f32) -> Vec2 {
        let base_radius = self.base_radius;
        self.retarget_in -= dt;
        if self.retarget_in <= 0.0 {
            self.retarget_in = self.rng.range(1.6, 4.5);
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
        Vec2::new(
            self.angle.cos() * self.radius,
            self.angle.sin() * self.radius * 0.42 + self.height * self.radius,
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OrbPhase {
    Whole,
    Gathering,
    Flash,
    Growing,
    SeedingOrbit,
}

/// Periodic burst and reassembly, copied from the accepted desktop-pet actor.
struct OrbLife {
    phase: OrbPhase,
    timer: f32,
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

    fn reset_whole(&mut self) {
        self.phase = OrbPhase::Whole;
        self.timer = 9.0;
        self.solidity = 1.0;
    }

    fn update(&mut self, dt: f32, cloud: &mut Particles, centre: Vec2, radius: f32) {
        match self.phase {
            OrbPhase::Whole => {
                self.timer -= dt;
                self.solidity = (self.solidity + dt * 3.0).min(1.0);
                if self.timer <= 0.0 {
                    self.phase = OrbPhase::Gathering;
                    self.timer = 0.0;
                    self.solidity = 0.0;
                    cloud.burst_with_fragments(
                        centre,
                        radius,
                        radius * self.rng.range(BURST_SPEED_MIN, BURST_SPEED_MAX),
                        BURST_FRAGMENT_COUNT,
                    );
                    // One collision system owns the outward impulse and the
                    // whole return. Only its continuously changing pressure
                    // controls the cycle; there is no mode hand-off.
                    cloud.compress_in(centre, radius * BURST_CHAMBER_RADIUS, 0.0);
                }
            }
            OrbPhase::Gathering => {
                const CLUSTER_DURATION: f32 = 3.50;
                self.timer += dt;
                let cluster = compression_progress(self.timer / CLUSTER_DURATION);
                let chamber_radius =
                    radius * (BURST_CHAMBER_RADIUS + (0.11 - BURST_CHAMBER_RADIUS) * cluster);
                cloud.compress_in(centre, chamber_radius, cluster);
                self.solidity = 0.0;
                if cluster >= 1.0 {
                    self.phase = OrbPhase::Flash;
                    self.timer = 0.0;
                }
            }
            OrbPhase::Flash => {
                const FLASH_DURATION: f32 = 0.28;
                self.timer = (self.timer + dt).min(FLASH_DURATION);
                cloud.set_brightness(1.0 + self.flash_amount());
                if self.timer >= FLASH_DURATION {
                    cloud.clear();
                    self.phase = OrbPhase::Growing;
                    self.timer = 0.0;
                }
            }
            OrbPhase::Growing => {
                const GROW_DURATION: f32 = 1.20;
                self.timer = (self.timer + dt).min(GROW_DURATION);
                self.solidity = smoothstep(0.0, 1.0, self.timer / GROW_DURATION);
                if self.timer >= GROW_DURATION {
                    self.phase = OrbPhase::SeedingOrbit;
                    self.timer = 0.0;
                    self.solidity = 1.0;
                }
            }
            OrbPhase::SeedingOrbit => {
                const SPAWN_INTERVAL: f32 = 0.055;
                self.timer += dt;
                while self.timer >= SPAWN_INTERVAL && cloud.alive() < ORBIT_PARTICLE_COUNT {
                    self.timer -= SPAWN_INTERVAL;
                    cloud.spawn_orbit(centre, radius * 1.05, radius * 1.8);
                }
                if cloud.alive() >= ORBIT_PARTICLE_COUNT {
                    self.phase = OrbPhase::Whole;
                    self.timer = self.rng.range(11.0, 22.0);
                }
            }
        }
    }

    fn opacity(&self) -> f32 {
        match self.phase {
            OrbPhase::Growing => 0.68 + self.appearance() * 0.32,
            OrbPhase::Whole | OrbPhase::SeedingOrbit => 1.0,
            OrbPhase::Gathering | OrbPhase::Flash => 0.0,
        }
    }

    fn appearance(&self) -> f32 {
        match self.phase {
            OrbPhase::Growing => self.solidity.clamp(0.0, 1.0),
            OrbPhase::Whole | OrbPhase::SeedingOrbit => 1.0,
            OrbPhase::Gathering | OrbPhase::Flash => 0.0,
        }
    }

    fn flash_amount(&self) -> f32 {
        if self.phase != OrbPhase::Flash {
            return 0.0;
        }
        const FLASH_DURATION: f32 = 0.28;
        (std::f32::consts::PI * (self.timer / FLASH_DURATION).clamp(0.0, 1.0)).sin()
    }

    const fn free_flight(&self) -> bool {
        matches!(self.phase, OrbPhase::Whole)
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let amount = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    amount * amount * (3.0 - 2.0 * amount)
}

fn compression_progress(value: f32) -> f32 {
    let amount = value.clamp(0.0, 1.0);
    amount * 0.3 + amount * amount * 0.7
}

fn draw_layer(canvas: &mut Canvas, quads: &[Quad], behind: bool) {
    for quad in quads {
        if (quad.depth < 0.0) != behind {
            continue;
        }
        draw_quad(canvas, quad.position, quad);
    }
}

/// A clean pixel fragment. The former doubled halo was visible as a square
/// background while fragments crossed light content.
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
    canvas.fill_rect(
        position.x as i32 - core / 2,
        position.y as i32 - core / 2,
        core,
        core,
        colour_at(core_alpha),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_starts_as_the_same_whole_orb_with_orbit_particles() {
        let actor = OrbActor::new(Vec2::new(100.0, 80.0), 20.0, 30.0, 0.45);
        assert_eq!(actor.alive_particles(), ORBIT_PARTICLE_COUNT);
        assert_eq!(actor.life.opacity(), 1.0);
    }

    #[test]
    fn recording_pose_is_fixed_and_restores_the_home_orbit() {
        let home = Vec2::new(100.0, 80.0);
        let recording = Vec2::new(320.0, 440.0);
        let mut actor = OrbActor::new(home, 20.0, 30.0, 0.45);

        actor.enter_recording_pose(recording);
        actor.update(1.0 / 30.0, home, 1.0);
        assert_eq!(actor.position(), recording);
        assert_eq!(actor.alive_particles(), 0);
        assert!(actor.is_grabbable());

        actor.resume_home_pose(home);
        assert_eq!(actor.position(), home);
        assert_eq!(actor.alive_particles(), ORBIT_PARTICLE_COUNT);
        assert_eq!(actor.life.opacity(), 1.0);
    }

    #[test]
    fn compression_keeps_moving_at_the_collapse_handoff() {
        let final_step = compression_progress(1.0) - compression_progress(0.99);
        let previous_step = compression_progress(0.99) - compression_progress(0.98);

        assert!(final_step > 0.0);
        assert!(final_step >= previous_step);
    }

    #[test]
    fn held_drag_continuously_catches_up_and_can_extend_again() {
        let anchor = Vec2::new(100.0, 80.0);
        let mut actor = OrbActor::new(anchor, 20.0, 30.0, 0.24);
        actor.begin_drag();
        actor.drag_window(Vec2::new(80.0, 0.0));
        assert!(actor.dragging);
        assert!(actor.drag_tail_active);

        actor.update(1.0 / 60.0, anchor, 1.0);
        assert!(
            actor.drag_tail_active,
            "возврат начинается без телепортации"
        );
        for _ in 0..24 {
            actor.update(1.0 / 60.0, anchor, 1.0);
        }
        assert!(actor.dragging, "захват мыши всё ещё активен");
        assert!(
            !actor.drag_tail_active,
            "остановившийся хвост должен догнать сферу"
        );

        actor.drag_window(Vec2::new(24.0, 8.0));
        assert!(
            actor.drag_tail_active,
            "новое движение снова вытягивает хвост"
        );
        actor.end_drag();
        assert!(!actor.dragging);
        assert!(!actor.drag_tail_active);
    }

    #[test]
    fn burst_is_dense_and_reassembly_finishes_without_a_sprite_pop() {
        let anchor = Vec2::new(100.0, 80.0);
        let mut actor = OrbActor::new(anchor + Vec2::new(20.0, 0.0), 20.0, 30.0, 0.16);
        for _ in 0..570 {
            actor.update(1.0 / 60.0, anchor, 1.0);
        }
        assert_eq!(actor.alive_particles(), BURST_FRAGMENT_COUNT);
        assert_eq!(actor.life.opacity(), 0.0);
        let rebuild_position = actor.position();
        for _ in 0..60 {
            actor.update(1.0 / 60.0, anchor, 1.0);
        }
        assert_eq!(actor.position(), rebuild_position);

        let mut saw_partial_opacity = false;
        for _ in 0..600 {
            actor.update(1.0 / 60.0, anchor, 1.0);
            let opacity = actor.life.opacity();
            saw_partial_opacity |= opacity > 0.0 && opacity < 1.0;
            if actor.alive_particles() == ORBIT_PARTICLE_COUNT {
                break;
            }
        }
        assert!(saw_partial_opacity);
        assert_eq!(actor.alive_particles(), ORBIT_PARTICLE_COUNT);
        assert_eq!(actor.life.opacity(), 1.0);
    }
}
