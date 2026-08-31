use std::collections::HashSet;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechBackendPreference {
    #[default]
    Auto,
    Cuda,
    Vulkan,
    Cpu,
}

impl SpeechBackendPreference {
    pub const fn index(self) -> i32 {
        match self {
            Self::Auto => 0,
            Self::Cuda => 1,
            Self::Vulkan => 2,
            Self::Cpu => 3,
        }
    }

    pub const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Auto),
            1 => Some(Self::Cuda),
            2 => Some(Self::Vulkan),
            3 => Some(Self::Cpu),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Cuda => "CUDA",
            Self::Vulkan => "Vulkan",
            Self::Cpu => "CPU",
        }
    }

    fn candidates(self) -> &'static [SpeechBackend] {
        match self {
            Self::Auto | Self::Cuda => &[
                SpeechBackend::Cuda,
                SpeechBackend::Vulkan,
                SpeechBackend::Cpu,
            ],
            Self::Vulkan => &[SpeechBackend::Vulkan, SpeechBackend::Cpu],
            Self::Cpu => &[SpeechBackend::Cpu],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpeechBackend {
    Cuda,
    Vulkan,
    Cpu,
}

impl SpeechBackend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cuda => "cuda",
            Self::Vulkan => "vulkan",
            Self::Cpu => "cpu",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Cuda => "CUDA",
            Self::Vulkan => "Vulkan",
            Self::Cpu => "CPU",
        }
    }

    pub fn from_reported_name(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("cuda") {
            Some(Self::Cuda)
        } else if name.eq_ignore_ascii_case("vulkan") {
            Some(Self::Vulkan)
        } else if name.eq_ignore_ascii_case("cpu") {
            Some(Self::Cpu)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpeechBackendPlan {
    preference: SpeechBackendPreference,
    candidates: Vec<SpeechBackend>,
    rejected: HashSet<SpeechBackend>,
    active: SpeechBackend,
    verified: bool,
}

impl SpeechBackendPlan {
    pub fn new(
        preference: SpeechBackendPreference,
        available: impl IntoIterator<Item = SpeechBackend>,
    ) -> Result<Self, String> {
        let available = available.into_iter().collect::<HashSet<_>>();
        let candidates = preference
            .candidates()
            .iter()
            .copied()
            .filter(|backend| available.contains(backend))
            .collect::<Vec<_>>();
        let Some(active) = candidates.first().copied() else {
            return Err(format!(
                "no installed speech backend can satisfy the {} preference",
                preference.label()
            ));
        };
        Ok(Self {
            preference,
            candidates,
            rejected: HashSet::new(),
            active,
            verified: false,
        })
    }

    pub const fn preference(&self) -> SpeechBackendPreference {
        self.preference
    }

    pub const fn active(&self) -> SpeechBackend {
        self.active
    }

    #[cfg(test)]
    pub const fn is_verified(&self) -> bool {
        self.verified
    }

    pub fn mark_verified(&mut self, reported: &str) -> Result<SpeechBackend, String> {
        let reported = SpeechBackend::from_reported_name(reported)
            .ok_or_else(|| format!("speech worker reported unknown backend {reported}"))?;
        if reported != self.active {
            return Err(format!(
                "speech worker reported {} while {} is active",
                reported.display_name(),
                self.active.display_name()
            ));
        }
        self.verified = true;
        Ok(reported)
    }

    /// Quarantines the active implementation for this application run. A
    /// backend that failed its real model probe must not be selected again by
    /// a generic worker restart loop.
    pub fn reject_active_and_advance(&mut self) -> Option<SpeechBackend> {
        self.rejected.insert(self.active);
        self.verified = false;
        let next = self
            .candidates
            .iter()
            .copied()
            .find(|candidate| !self.rejected.contains(candidate))?;
        self.active = next;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::{SpeechBackend, SpeechBackendPlan, SpeechBackendPreference};

    #[test]
    fn auto_falls_back_once_per_backend() {
        let mut plan = SpeechBackendPlan::new(
            SpeechBackendPreference::Auto,
            [
                SpeechBackend::Cuda,
                SpeechBackend::Vulkan,
                SpeechBackend::Cpu,
            ],
        )
        .unwrap();
        assert_eq!(plan.active(), SpeechBackend::Cuda);
        assert_eq!(
            plan.reject_active_and_advance(),
            Some(SpeechBackend::Vulkan)
        );
        assert_eq!(plan.reject_active_and_advance(), Some(SpeechBackend::Cpu));
        assert_eq!(plan.reject_active_and_advance(), None);
    }

    #[test]
    fn explicit_vulkan_never_silently_returns_to_cuda() {
        let mut plan = SpeechBackendPlan::new(
            SpeechBackendPreference::Vulkan,
            [
                SpeechBackend::Cuda,
                SpeechBackend::Vulkan,
                SpeechBackend::Cpu,
            ],
        )
        .unwrap();
        assert_eq!(plan.active(), SpeechBackend::Vulkan);
        assert_eq!(plan.reject_active_and_advance(), Some(SpeechBackend::Cpu));
    }

    #[test]
    fn reported_backend_must_match_the_launched_binary() {
        let mut plan = SpeechBackendPlan::new(
            SpeechBackendPreference::Auto,
            [SpeechBackend::Cuda, SpeechBackend::Cpu],
        )
        .unwrap();
        assert!(plan.mark_verified("vulkan").is_err());
        assert!(!plan.is_verified());
        assert_eq!(plan.mark_verified("cuda"), Ok(SpeechBackend::Cuda));
        assert!(plan.is_verified());
    }
}
