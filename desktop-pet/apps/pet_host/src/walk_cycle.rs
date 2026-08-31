//! Authored profile walk-cycle assets.
//!
//! Walking is intentionally represented by complete painted frames.  A flat
//! limb rig cannot reproduce the changing silhouette, overlap and
//! foreshortening present in the supplied contact/down/passing/up references.

use std::{fs, path::Path};

use pet_face::Visor;
use pet_render::{Sprite, SpriteSource};
use serde::Deserialize;

pub const FRAME_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pack {
    A,
    B,
    C,
    #[default]
    D,
    /// Diagnostic fallback to the previous articulated renderer.
    Procedural,
}

impl Pack {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "a" | "generated-a" => Some(Self::A),
            "b" | "generated-b" => Some(Self::B),
            "c" | "generated-c" => Some(Self::C),
            "d" | "generated-d" | "authored" | "default" => Some(Self::D),
            "rig" | "procedural" | "legacy" => Some(Self::Procedural),
            _ => None,
        }
    }

    pub const fn manifest_name(self) -> Option<&'static str> {
        match self {
            Self::A => Some("walk_cycle_a.json"),
            Self::B => Some("walk_cycle_b.json"),
            Self::C => Some("walk_cycle_c.json"),
            Self::D => Some("walk_cycle_d.json"),
            Self::Procedural => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    source_size: [u32; 2],
    ground: f32,
    cycle_travel: f32,
    face_anchors: RawFaceAnchors,
    frames: Vec<RawFrame>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFaceAnchors {
    eye: [f32; 2],
    mouth: [f32; 2],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrame {
    phase: String,
    source_index: usize,
    right: String,
    left: String,
    right_visor: RawRect,
    left_visor: RawRect,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

pub struct Assets {
    source_size: (u32, u32),
    ground: f32,
    cycle_travel: f32,
    face_anchors: RawFaceAnchors,
    frames: Vec<SourceFrame>,
}

struct SourceFrame {
    right: SpriteSource,
    left: SpriteSource,
    right_visor: RawRect,
    left_visor: RawRect,
}

impl Assets {
    pub fn load(manifest_path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(manifest_path)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        let raw: RawManifest = serde_json::from_str(&text)
            .map_err(|error| format!("{}: invalid JSON: {error}", manifest_path.display()))?;
        validate_manifest(&raw)?;
        let root = manifest_path
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", manifest_path.display()))?;
        let mut frames = Vec::with_capacity(FRAME_COUNT);
        for frame in raw.frames {
            let right = load_sprite(root, &frame.right)?;
            let left = load_sprite(root, &frame.left)?;
            frames.push(SourceFrame {
                right,
                left,
                right_visor: frame.right_visor,
                left_visor: frame.left_visor,
            });
        }
        Ok(Self {
            source_size: raw.source_size.into(),
            ground: raw.ground,
            cycle_travel: raw.cycle_travel,
            face_anchors: raw.face_anchors,
            frames,
        })
    }

    pub fn build(&self, scale: f32, canvas_width: u32, scene_ground: f32) -> Cycle {
        let source_left = (canvas_width as f32 - self.source_size.0 as f32 * scale) * 0.5;
        let source_top = scene_ground - self.ground * scale;
        let visor = |rect: RawRect| Visor {
            width: rect.w as f32,
            height: rect.h as f32,
            left_eye: self.face_anchors.eye.into(),
            right_eye: self.face_anchors.eye.into(),
            mouth: self.face_anchors.mouth.into(),
        };
        let build_side = |source: &SpriteSource, rect: RawRect| {
            let (crop_x, crop_y) = source.offset();
            BuiltFrame {
                sprite: source.scaled(scale),
                origin: (
                    source_left + crop_x as f32 * scale,
                    source_top + crop_y as f32 * scale,
                ),
                visor_origin: (
                    source_left + rect.x as f32 * scale,
                    source_top + rect.y as f32 * scale,
                ),
                visor: visor(rect),
            }
        };
        let frames = self
            .frames
            .iter()
            .map(|frame| BuiltPair {
                right: build_side(&frame.right, frame.right_visor),
                left: build_side(&frame.left, frame.left_visor),
            })
            .collect();
        Cycle {
            frames,
            travel_distance: self.cycle_travel * scale,
        }
    }
}

fn load_sprite(root: &Path, name: &str) -> Result<SpriteSource, String> {
    let relative = Path::new(name);
    if relative.components().count() != 1
        || relative.extension().and_then(|value| value.to_str()) != Some("png")
    {
        return Err(format!("unsafe walk-cycle sprite path: {name}"));
    }
    SpriteSource::load(&root.join(relative), 10).map_err(|error| format!("{name}: {error}"))
}

fn validate_manifest(raw: &RawManifest) -> Result<(), String> {
    if raw.source_size[0] == 0 || raw.source_size[1] == 0 {
        return Err("walk-cycle source_size must be non-zero".to_owned());
    }
    if !(0.0..=raw.source_size[1] as f32).contains(&raw.ground) {
        return Err("walk-cycle ground is outside source_size".to_owned());
    }
    if !raw.cycle_travel.is_finite() || raw.cycle_travel <= 0.0 {
        return Err("walk-cycle cycle_travel must be finite and positive".to_owned());
    }
    if raw.frames.len() != FRAME_COUNT {
        return Err(format!(
            "walk cycle must contain exactly {FRAME_COUNT} frames, got {}",
            raw.frames.len()
        ));
    }
    const PHASES: [&str; FRAME_COUNT] = [
        "contact_a",
        "down_a",
        "passing_a",
        "up_a",
        "contact_b",
        "down_b",
        "passing_b",
        "up_b",
    ];
    for (index, (frame, phase)) in raw.frames.iter().zip(PHASES).enumerate() {
        if frame.phase != phase {
            return Err(format!(
                "walk frame {index} must be phase {phase}, got {}",
                frame.phase
            ));
        }
        if frame.source_index > 64 {
            return Err(format!("walk frame {index} has implausible source_index"));
        }
        for rect in [frame.right_visor, frame.left_visor] {
            if rect.w == 0
                || rect.h == 0
                || rect.x.saturating_add(rect.w) > raw.source_size[0]
                || rect.y.saturating_add(rect.h) > raw.source_size[1]
            {
                return Err(format!("walk frame {index} has invalid visor geometry"));
            }
        }
    }
    for value in [
        raw.face_anchors.eye[0],
        raw.face_anchors.eye[1],
        raw.face_anchors.mouth[0],
        raw.face_anchors.mouth[1],
    ] {
        if !(0.0..=1.0).contains(&value) {
            return Err("walk-cycle face anchor must be normalized".to_owned());
        }
    }
    Ok(())
}

pub struct Cycle {
    frames: Vec<BuiltPair>,
    travel_distance: f32,
}

impl Cycle {
    pub fn travel_distance(&self) -> f32 {
        self.travel_distance
    }

    pub fn frame(&self, index: usize, facing_right: bool) -> &BuiltFrame {
        let pair = &self.frames[index % FRAME_COUNT];
        if facing_right {
            &pair.right
        } else {
            &pair.left
        }
    }
}

struct BuiltPair {
    right: BuiltFrame,
    left: BuiltFrame,
}

pub struct BuiltFrame {
    pub sprite: Sprite,
    pub origin: (f32, f32),
    pub visor_origin: (f32, f32),
    pub visor: Visor,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_parser_keeps_the_procedural_fallback_explicit() {
        assert_eq!(Pack::parse("a"), Some(Pack::A));
        assert_eq!(Pack::parse("generated-b"), Some(Pack::B));
        assert_eq!(Pack::parse("default"), Some(Pack::D));
        assert_eq!(Pack::parse("legacy"), Some(Pack::Procedural));
        assert_eq!(Pack::parse("unknown"), None);
    }
}
