//! Planning primitives for premium character traversal.
//!
//! This crate deliberately owns *decisions*, not rendering. The desktop world
//! reports reachable ledges and clearances, the planner chooses a mechanic,
//! and the animation graph consumes the fixed phase timeline. Keeping these
//! layers separate prevents each new jump or climb from becoming another
//! collection of mutually conflicting timers in the window loop.

/// A traversal mechanic selected for one obstacle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanic {
    /// A low obstacle that can be crossed without leaving the ground for long.
    Vault,
    /// A conventional jump onto a reachable top surface.
    Jump,
    /// A jump with a full-body flip. Selected only when clearance permits it.
    FlipJump,
    /// Jump, catch the ledge with both hands, then pull the body over it.
    LedgeGrab,
    /// Hold the companion orb while it lifts the character to the ledge.
    OrbLift,
}

/// Semantic animation phases. Render code maps these to poses and curves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Anticipation,
    Launch,
    Flight,
    Tuck,
    LedgeContact,
    PullUp,
    OrbCall,
    OrbGrip,
    OrbAscent,
    OrbRelease,
    Landing,
    Recovery,
}

/// What is physically constrained during a phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attachment {
    Free,
    BothHandsToLedge,
    BothHandsToOrb,
    FeetOnSurface,
}

/// One immutable entry in a traversal timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaseSpec {
    pub phase: Phase,
    pub duration_seconds: f32,
    pub attachment: Attachment,
}

impl PhaseSpec {
    const EMPTY: Self = Self {
        phase: Phase::Recovery,
        duration_seconds: 0.0,
        attachment: Attachment::Free,
    };

    const fn new(phase: Phase, duration_seconds: f32, attachment: Attachment) -> Self {
        Self {
            phase,
            duration_seconds,
            attachment,
        }
    }
}

const MAX_PHASES: usize = 8;

/// Fixed-capacity phase sequence: traversal never allocates in the frame loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    phases: [PhaseSpec; MAX_PHASES],
    len: u8,
}

impl Timeline {
    const fn from_slice(phases: &[PhaseSpec]) -> Self {
        let mut result = [PhaseSpec::EMPTY; MAX_PHASES];
        let mut index = 0;
        while index < phases.len() {
            result[index] = phases[index];
            index += 1;
        }
        Self {
            phases: result,
            len: phases.len() as u8,
        }
    }

    #[must_use]
    pub fn phases(&self) -> &[PhaseSpec] {
        &self.phases[..usize::from(self.len)]
    }

    #[must_use]
    pub fn duration_seconds(&self) -> f32 {
        self.phases()
            .iter()
            .map(|phase| phase.duration_seconds)
            .sum()
    }

    /// Resolves elapsed time to a phase without allocating or mutating state.
    #[must_use]
    pub fn sample(&self, elapsed_seconds: f32) -> TimelineSample {
        let elapsed_seconds = elapsed_seconds.max(0.0);
        let mut phase_start = 0.0;
        for (index, phase) in self.phases().iter().enumerate() {
            let phase_end = phase_start + phase.duration_seconds;
            if elapsed_seconds < phase_end || index + 1 == self.phases().len() {
                let progress = if phase.duration_seconds > 0.0 {
                    ((elapsed_seconds - phase_start) / phase.duration_seconds).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                return TimelineSample {
                    index,
                    phase: phase.phase,
                    attachment: phase.attachment,
                    progress,
                    complete: elapsed_seconds >= self.duration_seconds(),
                };
            }
            phase_start = phase_end;
        }
        TimelineSample {
            index: 0,
            phase: Phase::Recovery,
            attachment: Attachment::Free,
            progress: 1.0,
            complete: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineSample {
    pub index: usize,
    pub phase: Phase,
    pub attachment: Attachment,
    pub progress: f32,
    pub complete: bool,
}

/// A single active traversal owned by the animation graph.
///
/// The graph advances from one immutable plan. Render code can sample it from
/// multiple layers (body, limbs, orb and effects) without giving each layer an
/// independent timer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveTraversal {
    plan: Plan,
    elapsed_seconds: f32,
}

impl ActiveTraversal {
    #[must_use]
    pub const fn new(plan: Plan) -> Self {
        Self {
            plan,
            elapsed_seconds: 0.0,
        }
    }

    #[must_use]
    pub const fn plan(&self) -> Plan {
        self.plan
    }

    #[must_use]
    pub fn sample(&self) -> TimelineSample {
        self.plan.timeline.sample(self.elapsed_seconds)
    }

    /// Advances one clock shared by the complete character action.
    pub fn advance(&mut self, dt_seconds: f32) -> TimelineSample {
        if dt_seconds.is_finite() && dt_seconds > 0.0 {
            self.elapsed_seconds =
                (self.elapsed_seconds + dt_seconds).min(self.plan.timeline.duration_seconds());
        }
        self.sample()
    }
}

/// Minimal action graph boundary used by the future behavior director.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionGraph {
    active: Option<ActiveTraversal>,
}

impl MotionGraph {
    #[must_use]
    pub const fn is_traversing(&self) -> bool {
        self.active.is_some()
    }

    #[must_use]
    pub const fn active(&self) -> Option<ActiveTraversal> {
        self.active
    }

    /// Traversals are atomic across their contact phases. The behavior layer
    /// can queue another request, but cannot silently replace a physical grip.
    pub fn begin(&mut self, plan: Plan) -> Result<(), GraphError> {
        if self.active.is_some() {
            return Err(GraphError::Busy);
        }
        self.active = Some(ActiveTraversal::new(plan));
        Ok(())
    }

    /// Returns the current sample. Completion clears the graph only after the
    /// recovery pose has been observable for its full duration.
    pub fn advance(&mut self, dt_seconds: f32) -> Option<TimelineSample> {
        let active = self.active.as_mut()?;
        let sample = active.advance(dt_seconds);
        if sample.complete {
            self.active = None;
        }
        Some(sample)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphError {
    Busy,
}

/// Character-relative traversal limits, expressed as multiples of body height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capabilities {
    pub vault_height: f32,
    pub jump_height: f32,
    pub ledge_grab_height: f32,
    pub orb_lift_height: f32,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            vault_height: 0.38,
            jump_height: 0.82,
            ledge_grab_height: 1.28,
            orb_lift_height: 2.75,
        }
    }
}

/// Per-obstacle inputs supplied by the future desktop world model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    /// Ledge height above the character's current support, in body heights.
    pub ledge_height: f32,
    /// Horizontal gap to the contact point, in body heights.
    pub horizontal_gap: f32,
    /// Clear vertical space above the character, in body heights.
    pub overhead_clearance: f32,
    /// Current horizontal speed, in body heights per second.
    pub approach_speed: f32,
    pub orb_available: bool,
    /// 0 disables acrobatics, 1 strongly favours them when safe.
    pub acrobatics: f32,
    /// Stable 0..1 variation value chosen by the behavior director.
    pub variation: f32,
}

/// A selected mechanic and its animation contract.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub mechanic: Mechanic,
    pub timeline: Timeline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    InvalidGeometry,
    Unreachable,
}

#[derive(Clone, Copy, Debug)]
pub struct Planner {
    capabilities: Capabilities,
}

impl Planner {
    #[must_use]
    pub const fn new(capabilities: Capabilities) -> Self {
        Self { capabilities }
    }

    /// Selects a traversal mechanic from geometry and behavior preferences.
    ///
    /// Safety dominates variety: a flip is never selected without sufficient
    /// head clearance, and orb assistance is reserved for heights that normal
    /// body mechanics cannot reach.
    pub fn plan(&self, request: Request) -> Result<Plan, PlanError> {
        if !request.ledge_height.is_finite()
            || !request.horizontal_gap.is_finite()
            || !request.overhead_clearance.is_finite()
            || !request.approach_speed.is_finite()
            || request.ledge_height < 0.0
            || request.horizontal_gap < 0.0
            || request.overhead_clearance < 0.0
        {
            return Err(PlanError::InvalidGeometry);
        }

        let caps = self.capabilities;
        let mechanic =
            if request.ledge_height <= caps.vault_height && request.approach_speed >= 0.65 {
                Mechanic::Vault
            } else if request.ledge_height <= caps.jump_height {
                let flip_is_safe = request.overhead_clearance >= 1.45
                    && request.horizontal_gap >= 0.45
                    && request.horizontal_gap <= 1.65;
                let flip_is_wanted = request.acrobatics.clamp(0.0, 1.0) >= 0.62
                    && request.variation.clamp(0.0, 1.0) >= 0.42;
                if flip_is_safe && flip_is_wanted {
                    Mechanic::FlipJump
                } else {
                    Mechanic::Jump
                }
            } else if request.ledge_height <= caps.ledge_grab_height {
                Mechanic::LedgeGrab
            } else if request.orb_available && request.ledge_height <= caps.orb_lift_height {
                Mechanic::OrbLift
            } else {
                return Err(PlanError::Unreachable);
            };

        Ok(Plan {
            mechanic,
            timeline: timeline_for(mechanic),
        })
    }
}

impl Default for Planner {
    fn default() -> Self {
        Self::new(Capabilities::default())
    }
}

#[must_use]
pub const fn timeline_for(mechanic: Mechanic) -> Timeline {
    use Attachment::{BothHandsToLedge, BothHandsToOrb, FeetOnSurface, Free};
    use Phase::{
        Anticipation, Flight, Landing, Launch, LedgeContact, OrbAscent, OrbCall, OrbGrip,
        OrbRelease, PullUp, Recovery, Tuck,
    };

    match mechanic {
        Mechanic::Vault => Timeline::from_slice(&[
            PhaseSpec::new(Anticipation, 0.10, FeetOnSurface),
            PhaseSpec::new(Launch, 0.10, Free),
            PhaseSpec::new(Flight, 0.18, Free),
            PhaseSpec::new(Landing, 0.09, FeetOnSurface),
            PhaseSpec::new(Recovery, 0.12, FeetOnSurface),
        ]),
        Mechanic::Jump => Timeline::from_slice(&[
            PhaseSpec::new(Anticipation, 0.14, FeetOnSurface),
            PhaseSpec::new(Launch, 0.10, Free),
            PhaseSpec::new(Flight, 0.31, Free),
            PhaseSpec::new(Landing, 0.11, FeetOnSurface),
            PhaseSpec::new(Recovery, 0.15, FeetOnSurface),
        ]),
        Mechanic::FlipJump => Timeline::from_slice(&[
            PhaseSpec::new(Anticipation, 0.17, FeetOnSurface),
            PhaseSpec::new(Launch, 0.09, Free),
            PhaseSpec::new(Tuck, 0.24, Free),
            PhaseSpec::new(Flight, 0.18, Free),
            PhaseSpec::new(Landing, 0.12, FeetOnSurface),
            PhaseSpec::new(Recovery, 0.18, FeetOnSurface),
        ]),
        Mechanic::LedgeGrab => Timeline::from_slice(&[
            PhaseSpec::new(Anticipation, 0.16, FeetOnSurface),
            PhaseSpec::new(Launch, 0.11, Free),
            PhaseSpec::new(Flight, 0.24, Free),
            PhaseSpec::new(LedgeContact, 0.10, BothHandsToLedge),
            PhaseSpec::new(PullUp, 0.42, BothHandsToLedge),
            PhaseSpec::new(Landing, 0.10, FeetOnSurface),
            PhaseSpec::new(Recovery, 0.16, FeetOnSurface),
        ]),
        Mechanic::OrbLift => Timeline::from_slice(&[
            PhaseSpec::new(OrbCall, 0.16, FeetOnSurface),
            PhaseSpec::new(OrbGrip, 0.13, BothHandsToOrb),
            PhaseSpec::new(OrbAscent, 0.58, BothHandsToOrb),
            PhaseSpec::new(OrbRelease, 0.12, BothHandsToLedge),
            PhaseSpec::new(PullUp, 0.40, BothHandsToLedge),
            PhaseSpec::new(Landing, 0.10, FeetOnSurface),
            PhaseSpec::new(Recovery, 0.18, FeetOnSurface),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Attachment, GraphError, Mechanic, MotionGraph, Phase, PlanError, Planner, Request,
    };

    fn request(height: f32) -> Request {
        Request {
            ledge_height: height,
            horizontal_gap: 0.8,
            overhead_clearance: 2.0,
            approach_speed: 0.2,
            orb_available: true,
            acrobatics: 0.0,
            variation: 0.0,
        }
    }

    #[test]
    fn geometry_selects_increasingly_capable_mechanics() {
        let planner = Planner::default();
        let mut low = request(0.25);
        low.approach_speed = 1.0;
        assert_eq!(planner.plan(low).unwrap().mechanic, Mechanic::Vault);
        assert_eq!(
            planner.plan(request(0.70)).unwrap().mechanic,
            Mechanic::Jump
        );
        assert_eq!(
            planner.plan(request(1.10)).unwrap().mechanic,
            Mechanic::LedgeGrab
        );
        assert_eq!(
            planner.plan(request(1.90)).unwrap().mechanic,
            Mechanic::OrbLift
        );
    }

    #[test]
    fn orb_lift_has_a_real_grip_and_transfer_contract() {
        let plan = Planner::default().plan(request(2.0)).unwrap();
        let phases = plan.timeline.phases();
        assert!(phases.iter().any(|phase| {
            phase.phase == Phase::OrbAscent && phase.attachment == Attachment::BothHandsToOrb
        }));
        assert!(phases.iter().any(|phase| {
            phase.phase == Phase::OrbRelease && phase.attachment == Attachment::BothHandsToLedge
        }));
        assert!(plan.timeline.duration_seconds() > 1.0);
    }

    #[test]
    fn flip_requires_clearance_and_behavior_permission() {
        let planner = Planner::default();
        let mut acrobatic = request(0.7);
        acrobatic.acrobatics = 0.9;
        acrobatic.variation = 0.8;
        assert_eq!(
            planner.plan(acrobatic).unwrap().mechanic,
            Mechanic::FlipJump
        );
        acrobatic.overhead_clearance = 1.0;
        assert_eq!(planner.plan(acrobatic).unwrap().mechanic, Mechanic::Jump);
    }

    #[test]
    fn unreachable_height_does_not_fall_back_to_fake_flight() {
        let planner = Planner::default();
        let mut too_high = request(3.2);
        assert_eq!(planner.plan(too_high), Err(PlanError::Unreachable));
        too_high.ledge_height = 2.0;
        too_high.orb_available = false;
        assert_eq!(planner.plan(too_high), Err(PlanError::Unreachable));
    }

    #[test]
    fn sampling_preserves_attachment_until_the_phase_changes() {
        let plan = Planner::default().plan(request(2.0)).unwrap();
        let grip = plan.timeline.sample(0.20);
        assert_eq!(grip.phase, Phase::OrbGrip);
        assert_eq!(grip.attachment, Attachment::BothHandsToOrb);
        assert!((0.0..=1.0).contains(&grip.progress));
        let done = plan.timeline.sample(99.0);
        assert!(done.complete);
        assert_eq!(done.phase, Phase::Recovery);
    }

    #[test]
    fn rejects_non_finite_or_negative_geometry() {
        let planner = Planner::default();
        assert_eq!(
            planner.plan(Request {
                ledge_height: f32::NAN,
                ..request(1.0)
            }),
            Err(PlanError::InvalidGeometry)
        );
        assert_eq!(
            planner.plan(Request {
                horizontal_gap: -1.0,
                ..request(1.0)
            }),
            Err(PlanError::InvalidGeometry)
        );
    }

    #[test]
    fn graph_uses_one_clock_and_does_not_replace_an_active_grip() {
        let planner = Planner::default();
        let orb = planner.plan(request(2.0)).unwrap();
        let jump = planner.plan(request(0.7)).unwrap();
        let mut graph = MotionGraph::default();
        graph.begin(orb).unwrap();
        assert_eq!(graph.begin(jump), Err(GraphError::Busy));

        let mut saw_grip = false;
        let mut saw_ledge_transfer = false;
        for _ in 0..240 {
            let Some(sample) = graph.advance(1.0 / 120.0) else {
                break;
            };
            saw_grip |= sample.attachment == Attachment::BothHandsToOrb;
            saw_ledge_transfer |= sample.attachment == Attachment::BothHandsToLedge;
        }
        assert!(saw_grip && saw_ledge_transfer);
        assert!(!graph.is_traversing());
        assert!(graph.begin(jump).is_ok());
    }
}
