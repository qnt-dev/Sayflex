//! Режимы активности и темп кадров.
//!
//! Питомец живёт в фоне весь день, поэтому частота перерисовки — это не
//! настройка качества, а статья расхода батареи. Спящий персонаж не должен
//! перерисовываться вообще.

use std::time::Duration;

/// Что персонаж делает прямо сейчас. От этого зависит темп кадров.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// Полная неподвижность: кадры не выдаются.
    Asleep,
    /// Спокойное дыхание и медленный дрейф частиц.
    Idle,
    /// Слушает речь, реагирует на курсор.
    Attentive,
    /// Перемещение, бросок, перекат, взрыв.
    Active,
}

impl Activity {
    /// Порядок бодрости: чем выше, тем чаще кадры.
    const fn rank(self) -> u8 {
        match self {
            Self::Asleep => 0,
            Self::Idle => 1,
            Self::Attentive => 2,
            Self::Active => 3,
        }
    }

    /// Более бодрое из двух состояний. Нужно, когда пользователь печатает:
    /// персонаж стоит на месте, но спать ему нельзя.
    #[must_use]
    pub const fn max(self, other: Self) -> Self {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }

    /// Целевой интервал между кадрами. `None` — перерисовка не нужна.
    #[must_use]
    pub const fn frame_interval(self) -> Option<Duration> {
        match self {
            Self::Asleep => None,
            Self::Idle => Some(Duration::from_millis(66)), // ~15 fps
            Self::Attentive => Some(Duration::from_millis(33)), // ~30 fps
            Self::Active => Some(Duration::from_millis(16)), // ~60 fps
        }
    }
}

/// Скользящая статистика кадра для замеров этапа 0.
#[derive(Default)]
pub struct FrameStats {
    frames: u32,
    total_us: u64,
    peak_us: u64,
}

impl FrameStats {
    pub fn record(&mut self, elapsed: Duration) {
        let micros = elapsed.as_micros() as u64;
        self.frames += 1;
        self.total_us += micros;
        self.peak_us = self.peak_us.max(micros);
    }

    #[must_use]
    pub fn average_ms(&self) -> f32 {
        if self.frames == 0 {
            return 0.0;
        }
        self.total_us as f32 / self.frames as f32 / 1000.0
    }

    #[must_use]
    pub fn peak_ms(&self) -> f32 {
        self.peak_us as f32 / 1000.0
    }

    #[must_use]
    pub const fn frames(&self) -> u32 {
        self.frames
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::{Activity, FrameStats};
    use std::time::Duration;

    #[test]
    fn sleeping_never_asks_for_a_frame() {
        assert!(Activity::Asleep.frame_interval().is_none());
    }

    #[test]
    fn busier_states_ask_for_shorter_intervals() {
        let idle = Activity::Idle.frame_interval().unwrap();
        let attentive = Activity::Attentive.frame_interval().unwrap();
        let active = Activity::Active.frame_interval().unwrap();
        assert!(idle > attentive && attentive > active);
    }

    #[test]
    fn typing_keeps_the_pet_awake() {
        assert_eq!(
            Activity::Asleep.max(Activity::Attentive),
            Activity::Attentive
        );
        assert_eq!(Activity::Active.max(Activity::Idle), Activity::Active);
        assert_eq!(Activity::Idle.max(Activity::Idle), Activity::Idle);
    }

    #[test]
    fn stats_track_average_and_peak() {
        let mut stats = FrameStats::default();
        stats.record(Duration::from_micros(1000));
        stats.record(Duration::from_micros(3000));
        assert_eq!(stats.frames(), 2);
        assert!((stats.average_ms() - 2.0).abs() < 0.01);
        assert!((stats.peak_ms() - 3.0).abs() < 0.01);
    }
}
