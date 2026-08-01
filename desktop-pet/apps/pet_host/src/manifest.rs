//! Строгое описание геометрии позы.
//!
//! Манифесты генерируются скриптами подготовки ассетов, но всё равно являются
//! внешними файлами установленного приложения. Поэтому они разбираются
//! полноценным JSON-парсером, запрещают неизвестные поля и проходят
//! семантическую проверку до загрузки любого PNG.

use std::{collections::BTreeMap, fs, path::Path};

use pet_face::Visor;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisorRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Откуда исходят частицы: точка в пикселях спрайта и радиус разлёта.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticleAnchor {
    pub x: f32,
    pub y: f32,
    #[serde(rename = "r")]
    pub radius: f32,
}

/// Конечность: спрайт, точка вращения и смещение относительно исходного арта.
#[derive(Debug, Clone)]
pub struct LegManifest {
    pub sprite: String,
    pub pivot: (f32, f32),
    pub origin: (f32, f32),
}

/// Profile arms retain their semantic depth. A flattened source may expose
/// only one complete arm, so positional vectors are not sufficient: omitting
/// the far arm must never make the near arm render behind the torso.
#[derive(Debug, Clone, Default)]
pub struct ArmManifests {
    pub far: Option<LegManifest>,
    pub near: Option<LegManifest>,
}

impl ArmManifests {
    fn iter(&self) -> impl Iterator<Item = &LegManifest> {
        self.far.iter().chain(self.near.iter())
    }
}

/// Continuous leg used by the profile gait. Joint positions are expressed in
/// the common source canvas; the renderer deforms the one texture with a small
/// skinning mesh, so no raster seam can open at the knee or ankle.
#[derive(Debug, Clone)]
pub struct LegChainManifest {
    pub sprite: String,
    pub hip: (f32, f32),
    pub knee: (f32, f32),
    pub ankle: (f32, f32),
}

#[derive(Debug)]
pub struct FaceManifest {
    pub sprite: String,
    pub source_size: (u32, u32),
    pub visor: VisorRect,
    pub left_eye: (f32, f32),
    pub right_eye: (f32, f32),
    pub mouth: (f32, f32),
    pub particle_anchors: Vec<ParticleAnchor>,
    pub legs: Vec<LegManifest>,
    pub leg_chains: Vec<LegChainManifest>,
    pub arms: ArmManifests,
    /// Optional profile tail.  It is articulated independently so wagging it
    /// never stretches the pelvis, jacket, or head through the torso mesh.
    pub tail: Option<LegManifest>,
    pub pockets: Vec<(f32, f32)>,
    pub ground: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    sprite: String,
    source_size: [u32; 2],
    visor: VisorRect,
    anchors: RawFaceAnchors,
    #[serde(default)]
    particle_anchors: BTreeMap<String, ParticleAnchor>,
    #[serde(default)]
    legs: RawLegs,
    #[serde(default)]
    leg_chains: RawLegChains,
    #[serde(default)]
    arms: RawArms,
    #[serde(default)]
    tail: Option<RawLimb>,
    #[serde(default)]
    pockets: RawPockets,
    ground: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFaceAnchors {
    left_eye: [f32; 2],
    right_eye: [f32; 2],
    mouth: [f32; 2],
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLimb {
    sprite: String,
    pivot_x: f32,
    pivot_y: f32,
    origin_x: f32,
    origin_y: f32,
}

impl From<RawLimb> for LegManifest {
    fn from(value: RawLimb) -> Self {
        Self {
            sprite: value.sprite,
            pivot: (value.pivot_x, value.pivot_y),
            origin: (value.origin_x, value.origin_y),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawLegs {
    left: Option<RawLimb>,
    right: Option<RawLimb>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLegChain {
    sprite: String,
    hip: [f32; 2],
    knee: [f32; 2],
    ankle: [f32; 2],
}

impl From<RawLegChain> for LegChainManifest {
    fn from(value: RawLegChain) -> Self {
        Self {
            sprite: value.sprite,
            hip: value.hip.into(),
            knee: value.knee.into(),
            ankle: value.ankle.into(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawLegChains {
    far: Option<RawLegChain>,
    near: Option<RawLegChain>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawArms {
    far: Option<RawLimb>,
    near: Option<RawLimb>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPoint {
    x: f32,
    y: f32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawPockets {
    left: Option<RawPoint>,
    right: Option<RawPoint>,
}

impl FaceManifest {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn parse(text: &str) -> Result<Self, String> {
        let raw: RawManifest =
            serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
        let mut legs = Vec::with_capacity(2);
        legs.extend(raw.legs.left.map(Into::into));
        legs.extend(raw.legs.right.map(Into::into));
        let mut leg_chains = Vec::with_capacity(2);
        leg_chains.extend(raw.leg_chains.far.map(Into::into));
        leg_chains.extend(raw.leg_chains.near.map(Into::into));
        let arms = ArmManifests {
            far: raw.arms.far.map(Into::into),
            near: raw.arms.near.map(Into::into),
        };
        let mut pockets = Vec::with_capacity(2);
        pockets.extend(raw.pockets.left.map(|point| (point.x, point.y)));
        pockets.extend(raw.pockets.right.map(|point| (point.x, point.y)));

        let manifest = Self {
            sprite: raw.sprite,
            source_size: (raw.source_size[0], raw.source_size[1]),
            visor: raw.visor,
            left_eye: raw.anchors.left_eye.into(),
            right_eye: raw.anchors.right_eye.into(),
            mouth: raw.anchors.mouth.into(),
            particle_anchors: raw.particle_anchors.into_values().collect(),
            legs,
            leg_chains,
            arms,
            tail: raw.tail.map(Into::into),
            pockets,
            ground: raw.ground,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), String> {
        let (width, height) = self.source_size;
        if width == 0 || height == 0 {
            return Err("source_size must be non-zero".to_owned());
        }
        validate_asset_name(&self.sprite)?;
        let visor_right = self
            .visor
            .x
            .checked_add(self.visor.w)
            .ok_or("visor x overflow")?;
        let visor_bottom = self
            .visor
            .y
            .checked_add(self.visor.h)
            .ok_or("visor y overflow")?;
        if self.visor.w == 0 || self.visor.h == 0 || visor_right > width || visor_bottom > height {
            return Err("visor must fit inside source_size".to_owned());
        }
        for (name, point) in [
            ("left_eye", self.left_eye),
            ("right_eye", self.right_eye),
            ("mouth", self.mouth),
        ] {
            if !unit_point(point) {
                return Err(format!("anchor {name} must be finite and within 0..=1"));
            }
        }
        for anchor in &self.particle_anchors {
            if !source_point((anchor.x, anchor.y), width, height)
                || !anchor.radius.is_finite()
                || anchor.radius <= 0.0
            {
                return Err(
                    "particle anchor is outside source_size or has invalid radius".to_owned(),
                );
            }
        }
        for pocket in &self.pockets {
            if !source_point(*pocket, width, height) {
                return Err("pocket anchor is outside source_size".to_owned());
            }
        }
        for limb in self
            .legs
            .iter()
            .chain(self.arms.iter())
            .chain(self.tail.iter())
        {
            validate_asset_name(&limb.sprite)?;
            if !source_point(limb.pivot, width, height) || !source_point(limb.origin, width, height)
            {
                return Err(format!("limb {} contains invalid geometry", limb.sprite));
            }
        }
        for leg in &self.leg_chains {
            validate_asset_name(&leg.sprite)?;
            for joint in [leg.hip, leg.knee, leg.ankle] {
                if !source_point(joint, width, height) {
                    return Err("articulated leg contains invalid joint geometry".to_owned());
                }
            }
            if leg.hip == leg.knee || leg.knee == leg.ankle {
                return Err("articulated leg joints must not collapse".to_owned());
            }
        }
        if self
            .ground
            .is_some_and(|ground| !ground.is_finite() || !(0.0..=height as f32).contains(&ground))
        {
            return Err("ground must be finite and inside source_size".to_owned());
        }
        Ok(())
    }

    #[must_use]
    pub fn visor_geometry(&self) -> Visor {
        Visor {
            width: self.visor.w as f32,
            height: self.visor.h as f32,
            left_eye: self.left_eye,
            right_eye: self.right_eye,
            mouth: self.mouth,
        }
    }
}

fn validate_asset_name(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    let mut components = path.components();
    let is_single_file = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if !is_single_file || path.extension().and_then(|value| value.to_str()) != Some("png") {
        return Err(format!("unsafe asset path: {value}"));
    }
    Ok(())
}

fn unit_point((x, y): (f32, f32)) -> bool {
    x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)
}

fn source_point((x, y): (f32, f32), width: u32, height: u32) -> bool {
    x.is_finite()
        && y.is_finite()
        && (0.0..=width as f32).contains(&x)
        && (0.0..=height as f32).contains(&y)
}

#[cfg(test)]
mod tests {
    use super::FaceManifest;

    const SAMPLE: &str = r#"{
      "sprite": "body.png",
      "source_size": [625, 1105],
      "visor": { "x": 128, "y": 178, "w": 272, "h": 140 },
      "anchors": {
        "left_eye": [0.32, 0.39],
        "right_eye": [0.74, 0.44],
        "mouth": [0.52, 0.73]
      },
      "particle_anchors": {
        "tail": { "x": 95, "y": 105, "r": 62 }
      },
      "legs": {
        "left": { "sprite": "leg.png", "pivot_x": 168, "pivot_y": 828,
                  "origin_x": 73, "origin_y": 815 }
      },
      "leg_chains": {
        "far": {
          "sprite": "profile-leg.png",
          "hip": [200, 700], "knee": [210, 850], "ankle": [220, 980]
        }
      },
      "arms": {
        "far": { "sprite": "arm.png", "pivot_x": 200, "pivot_y": 500,
                 "origin_x": 0, "origin_y": 0 }
      },
      "tail": { "sprite": "tail.png", "pivot_x": 190, "pivot_y": 810,
                "origin_x": 0, "origin_y": 0 },
      "pockets": { "left": { "x": 123, "y": 678 } },
      "ground": 1090
    }"#;

    #[test]
    fn reads_all_runtime_geometry_from_its_own_section() {
        let manifest = FaceManifest::parse(SAMPLE).expect("manifest parses");
        assert_eq!(manifest.sprite, "body.png");
        assert_eq!(manifest.source_size, (625, 1105));
        assert_eq!(manifest.legs[0].sprite, "leg.png");
        assert_eq!(manifest.leg_chains[0].sprite, "profile-leg.png");
        assert_eq!(
            manifest.arms.far.as_ref().map(|arm| arm.sprite.as_str()),
            Some("arm.png")
        );
        assert_eq!(
            manifest.tail.as_ref().map(|tail| tail.sprite.as_str()),
            Some("tail.png")
        );
        assert_eq!(manifest.pockets, vec![(123.0, 678.0)]);
        assert_eq!(manifest.particle_anchors[0].radius, 62.0);
    }

    #[test]
    fn rejects_unknown_fields_and_traversal_paths() {
        let unknown = SAMPLE.replace("\"ground\": 1090", "\"ground\": 1090, \"typo\": 1");
        assert!(FaceManifest::parse(&unknown).is_err());
        let traversal = SAMPLE.replace("body.png", "../body.png");
        assert!(FaceManifest::parse(&traversal).is_err());
    }

    #[test]
    fn rejects_out_of_bounds_and_non_finite_geometry() {
        let visor = SAMPLE.replace("\"x\": 128", "\"x\": 600");
        assert!(FaceManifest::parse(&visor).is_err());
        let anchor = SAMPLE.replace("[0.32, 0.39]", "[1.32, 0.39]");
        assert!(FaceManifest::parse(&anchor).is_err());
    }
}
