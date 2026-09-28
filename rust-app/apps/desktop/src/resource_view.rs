//! Live process-resource projection and release of resident models.
//!
//! Sampling starts only while Resources is visible. Manual and conservative
//! idle release never edit the selected model or remove anything from disk.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use rust_platform_windows::{ProcessResourceSampler, ProcessResourceUsage};
use rust_protocol::{SpeechWorkerCommand, TranslationWorkerCommand};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::{
    AppState, ResourceRow, SettingsWindow, screen_translation::ScreenTranslationCoordinator,
    settings_projection::human_size, settings_view::set_status_problem, worker_supervisor::Workers,
};

const RESOURCES_PAGE: i32 = 9;
const SPEECH_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const TRANSLATION_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const OCR_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResourceFeature {
    Speech,
    Translation,
    Ocr,
}

#[derive(Clone, Copy, Debug)]
enum ReleaseCause {
    Manual,
    Idle,
}

impl ReleaseCause {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Idle => "idle",
        }
    }
}

impl ResourceFeature {
    const fn index(self) -> usize {
        match self {
            Self::Speech => 0,
            Self::Translation => 1,
            Self::Ocr => 2,
        }
    }

    const fn idle_timeout(self) -> Duration {
        match self {
            Self::Speech => SPEECH_IDLE_TIMEOUT,
            Self::Translation => TRANSLATION_IDLE_TIMEOUT,
            Self::Ocr => OCR_IDLE_TIMEOUT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResourceResidency {
    loaded: bool,
    busy: bool,
    releasing: bool,
    /// A user-facing keep-warm choice. Manual unload remains available.
    protected: bool,
}

#[derive(Debug, Default)]
struct IdleResidencyPolicy {
    idle_since: [Option<Duration>; 3],
}

impl IdleResidencyPolicy {
    fn due(
        &mut self,
        now: Duration,
        resources: impl IntoIterator<Item = (ResourceFeature, ResourceResidency)>,
    ) -> Vec<ResourceFeature> {
        let mut due = Vec::new();
        for (feature, residency) in resources {
            let idle_since = &mut self.idle_since[feature.index()];
            if !residency.loaded {
                *idle_since = None;
                continue;
            }
            if residency.busy {
                *idle_since = Some(now);
                continue;
            }
            if residency.releasing || residency.protected {
                *idle_since = None;
                continue;
            }

            let since = idle_since.get_or_insert(now);
            if now.saturating_sub(*since) >= feature.idle_timeout() {
                due.push(feature);
                // Avoid retrying every second if the worker rejects a release.
                *since = now;
            }
        }
        due
    }
}

pub(crate) fn wire_resources(
    settings: &SettingsWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    screen_translation: &Rc<RefCell<ScreenTranslationCoordinator>>,
) -> Timer {
    let sampler = Rc::new(RefCell::new(ProcessResourceSampler::new()));
    let residency_policy = Rc::new(RefCell::new(IdleResidencyPolicy::default()));
    let residency_clock = Instant::now();
    sync(
        settings,
        state,
        workers,
        screen_translation,
        &sampler,
        false,
    );

    {
        let settings = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let screen_translation = Rc::clone(screen_translation);
        let sampler = Rc::clone(&sampler);
        settings
            .upgrade()
            .expect("settings is alive while callbacks are wired")
            .on_unload_resource(move |id| {
                let feature = match id.as_str() {
                    "speech" => Some(ResourceFeature::Speech),
                    "translation" => Some(ResourceFeature::Translation),
                    "ocr" => Some(ResourceFeature::Ocr),
                    _ => None,
                };
                if let Some(feature) = feature {
                    unload(
                        feature,
                        ReleaseCause::Manual,
                        &settings,
                        &state,
                        &workers,
                        screen_translation.borrow().has_active_work(),
                    );
                }
                if let Some(settings) = settings.upgrade() {
                    sync(
                        &settings,
                        &state,
                        &workers,
                        &screen_translation,
                        &sampler,
                        true,
                    );
                }
            });
    }

    {
        let settings = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let screen_translation = Rc::clone(screen_translation);
        let sampler = Rc::clone(&sampler);
        settings
            .upgrade()
            .expect("settings is alive while callbacks are wired")
            .on_unload_all_resources(move || {
                for feature in [
                    ResourceFeature::Speech,
                    ResourceFeature::Translation,
                    ResourceFeature::Ocr,
                ] {
                    unload(
                        feature,
                        ReleaseCause::Manual,
                        &settings,
                        &state,
                        &workers,
                        screen_translation.borrow().has_active_work(),
                    );
                }
                if let Some(settings) = settings.upgrade() {
                    sync(
                        &settings,
                        &state,
                        &workers,
                        &screen_translation,
                        &sampler,
                        true,
                    );
                }
            });
    }

    let timer = Timer::default();
    {
        let settings = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        let screen_translation = Rc::clone(screen_translation);
        let residency_policy = Rc::clone(&residency_policy);
        timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            let Some(settings) = settings.upgrade() else {
                return;
            };
            let resources = {
                let state = state.borrow();
                resource_residencies(
                    &state,
                    &workers,
                    screen_translation.borrow().has_active_work(),
                )
            };
            for feature in residency_policy
                .borrow_mut()
                .due(residency_clock.elapsed(), resources)
            {
                unload(
                    feature,
                    ReleaseCause::Idle,
                    &settings.as_weak(),
                    &state,
                    &workers,
                    screen_translation.borrow().has_active_work(),
                );
            }
            if settings.window().is_visible() && settings.get_active_page() == RESOURCES_PAGE {
                sync(
                    &settings,
                    &state,
                    &workers,
                    &screen_translation,
                    &sampler,
                    true,
                );
            }
        });
    }
    timer
}

fn unload(
    feature: ResourceFeature,
    cause: ReleaseCause,
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
    screen_translation_active: bool,
) {
    let residency = {
        let state = state.borrow();
        resource_residencies(&state, workers, screen_translation_active)
            .into_iter()
            .find_map(|(candidate, residency)| (candidate == feature).then_some(residency))
            .expect("all resource features have residency state")
    };
    if !residency.loaded || residency.busy || residency.releasing {
        return;
    }
    rust_app_core::diagnostics::log_line(&format!(
        "resource_release feature={feature:?} cause={}",
        cause.as_str()
    ));

    let result = match feature {
        ResourceFeature::Speech => {
            {
                let mut state = state.borrow_mut();
                state.speech_reload_on_next_use = true;
                state.speech_unloading = true;
                state.speech_ready = false;
            }
            workers
                .speech
                .send(&SpeechWorkerCommand::UnloadLocalModel)
                .map_err(|error| error.to_string())
        }
        ResourceFeature::Translation => {
            {
                let mut state = state.borrow_mut();
                state.translation_reload_on_next_use = true;
                state.translation_unloading = true;
                state.translation_ready = false;
            }
            workers
                .translation
                .send(&TranslationWorkerCommand::UnloadLocalModel)
                .map_err(|error| error.to_string())
        }
        ResourceFeature::Ocr => workers.unload_ocr(cause.as_str()).map(|_| ()),
    };

    if let Err(error) = result {
        match feature {
            ResourceFeature::Speech => {
                let mut state = state.borrow_mut();
                state.speech_unloading = false;
                state.speech_reload_on_next_use = false;
                state.speech_ready = state.speech_loaded_model_id.is_some();
            }
            ResourceFeature::Translation => {
                let mut state = state.borrow_mut();
                state.translation_unloading = false;
                state.translation_reload_on_next_use = false;
                state.translation_ready = state.translation_loaded_model_id.is_some();
            }
            ResourceFeature::Ocr => {}
        }
        set_status_problem(settings, &error);
    }
}

fn resource_residencies(
    state: &AppState,
    workers: &Workers,
    screen_translation_active: bool,
) -> [(ResourceFeature, ResourceResidency); 3] {
    let speech_installing = state.downloads.keys().any(|catalog_id| {
        state
            .models
            .iter()
            .any(|model| model.catalog_id == *catalog_id)
    });
    let translation_installing = state.downloads.keys().any(|catalog_id| {
        state
            .translation_models
            .iter()
            .any(|model| model.catalog_id == *catalog_id)
    });
    let speech_busy = state.indicator_session.is_some() || speech_installing;
    let translation_busy = !state.pending_translations.is_empty()
        || state.pending_text_action.is_some()
        || !state.pending_selection_captures.is_empty()
        || !state.selection_release_gates.is_empty()
        || state.queued_model_translation.is_some()
        || state.queued_selection_translation.is_some()
        || translation_installing
        || screen_translation_active
        || {
            #[cfg(feature = "screen-translation")]
            {
                state.queued_screen_translation.is_some()
            }
            #[cfg(not(feature = "screen-translation"))]
            {
                false
            }
        };

    [
        (
            ResourceFeature::Speech,
            ResourceResidency {
                loaded: state.speech_loaded_model_id.is_some(),
                busy: speech_busy,
                releasing: state.speech_unloading,
                protected: false,
            },
        ),
        (
            ResourceFeature::Translation,
            ResourceResidency {
                loaded: state.translation_loaded_model_id.is_some(),
                busy: translation_busy,
                releasing: state.translation_unloading,
                protected: state.translation_keep_warm,
            },
        ),
        (
            ResourceFeature::Ocr,
            ResourceResidency {
                loaded: workers.ocr_pid().is_some(),
                busy: workers.ocr_busy() || screen_translation_active,
                releasing: false,
                protected: false,
            },
        ),
    ]
}

fn sync(
    settings: &SettingsWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Workers,
    screen_translation: &Rc<RefCell<ScreenTranslationCoordinator>>,
    sampler: &Rc<RefCell<ProcessResourceSampler>>,
    sample: bool,
) {
    let mut roots = vec![
        std::process::id(),
        workers.speech_pid(),
        workers.translation_pid(),
    ];
    let ocr_index = workers.ocr_pid().map(|pid| {
        let index = roots.len();
        roots.push(pid);
        index
    });
    let samples = if sample {
        sampler
            .borrow_mut()
            .sample_groups(&roots)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let state = state.borrow();
    let resources = resource_residencies(
        &state,
        workers,
        screen_translation.borrow().has_active_work(),
    );
    let speech = resources[ResourceFeature::Speech.index()].1;
    let translation = resources[ResourceFeature::Translation.index()].1;
    let ocr = resources[ResourceFeature::Ocr.index()].1;
    let aggregate_busy = resources.iter().any(|(_, resource)| resource.busy);
    let rows = vec![
        row(
            "total",
            samples.first().copied(),
            true,
            aggregate_busy,
            false,
            false,
        ),
        row(
            "speech",
            samples.get(1).copied(),
            speech.loaded,
            speech.busy,
            speech.releasing,
            true,
        ),
        row(
            "translation",
            samples.get(2).copied(),
            translation.loaded,
            translation.busy,
            translation.releasing,
            true,
        ),
        row(
            "ocr",
            ocr_index.and_then(|index| samples.get(index).copied()),
            ocr.loaded,
            ocr.busy,
            ocr.releasing,
            true,
        ),
    ];
    settings.set_resources_can_unload(rows.iter().any(|row| row.can_unload));
    settings.set_resource_rows(ModelRc::new(VecModel::from(rows)));
}

fn row(
    id: &str,
    usage: Option<ProcessResourceUsage>,
    loaded: bool,
    busy: bool,
    releasing: bool,
    allow_unload: bool,
) -> ResourceRow {
    let state = if releasing {
        "releasing"
    } else if busy {
        "working"
    } else if loaded {
        "ready"
    } else {
        "not-loaded"
    };
    let usage = usage.filter(|_| loaded || busy || releasing);
    ResourceRow {
        id: id.into(),
        state: state.into(),
        tone: if releasing {
            "releasing"
        } else if busy {
            "working"
        } else if loaded {
            "resting"
        } else {
            "off"
        }
        .into(),
        cpu: metric_percent(usage.and_then(|value| value.cpu_percent)),
        ram: metric_bytes(usage.map(|value| value.ram_bytes)),
        gpu: metric_percent(usage.and_then(|value| value.gpu_percent)),
        vram: metric_bytes(usage.and_then(|value| value.vram_bytes)),
        can_unload: allow_unload && loaded && !busy && !releasing,
    }
}

fn metric_percent(value: Option<f32>) -> SharedString {
    value
        .map(|value| format!("{value:.1} %"))
        .unwrap_or_else(|| "—".to_owned())
        .into()
}

fn metric_bytes(value: Option<u64>) -> SharedString {
    value
        .map(human_size)
        .unwrap_or_else(|| "—".to_owned())
        .into()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        IdleResidencyPolicy, ResourceFeature, ResourceResidency, metric_bytes, metric_percent, row,
    };

    #[test]
    fn unavailable_metrics_are_not_presented_as_real_zeroes() {
        assert_eq!(metric_percent(None), "—");
        assert_eq!(metric_bytes(None), "—");
    }

    #[test]
    fn aggregate_row_is_observational_and_can_never_be_unloaded() {
        let aggregate = row("total", None, true, false, false, false);

        assert_eq!(aggregate.id, "total");
        assert!(!aggregate.can_unload);
    }

    #[test]
    fn active_work_is_never_selected_for_idle_eviction() {
        let mut policy = IdleResidencyPolicy::default();
        let active = ResourceResidency {
            loaded: true,
            busy: true,
            releasing: false,
            protected: false,
        };

        assert!(
            policy
                .due(Duration::ZERO, [(ResourceFeature::Speech, active)])
                .is_empty()
        );
        assert!(
            policy
                .due(
                    Duration::from_secs(3_600),
                    [(ResourceFeature::Speech, active)]
                )
                .is_empty()
        );
    }

    #[test]
    fn idle_loaded_engines_are_evicted_only_after_their_deadline() {
        let mut policy = IdleResidencyPolicy::default();
        let idle = ResourceResidency {
            loaded: true,
            busy: false,
            releasing: false,
            protected: false,
        };

        assert!(
            policy
                .due(Duration::ZERO, [(ResourceFeature::Ocr, idle)])
                .is_empty()
        );
        assert!(
            policy
                .due(Duration::from_secs(299), [(ResourceFeature::Ocr, idle)])
                .is_empty()
        );
        assert_eq!(
            policy.due(Duration::from_secs(300), [(ResourceFeature::Ocr, idle)]),
            vec![ResourceFeature::Ocr]
        );
    }

    #[test]
    fn fresh_activity_resets_the_idle_deadline() {
        let mut policy = IdleResidencyPolicy::default();
        let idle = ResourceResidency {
            loaded: true,
            busy: false,
            releasing: false,
            protected: false,
        };
        let active = ResourceResidency { busy: true, ..idle };

        assert!(
            policy
                .due(Duration::ZERO, [(ResourceFeature::Ocr, idle)])
                .is_empty()
        );
        assert!(
            policy
                .due(Duration::from_secs(250), [(ResourceFeature::Ocr, active)])
                .is_empty()
        );
        assert!(
            policy
                .due(Duration::from_secs(549), [(ResourceFeature::Ocr, idle)])
                .is_empty()
        );
        assert_eq!(
            policy.due(Duration::from_secs(550), [(ResourceFeature::Ocr, idle)]),
            vec![ResourceFeature::Ocr]
        );
    }

    #[test]
    fn keep_warm_protects_translation_from_automatic_eviction() {
        let mut policy = IdleResidencyPolicy::default();
        let protected = ResourceResidency {
            loaded: true,
            busy: false,
            releasing: false,
            protected: true,
        };

        assert!(
            policy
                .due(Duration::ZERO, [(ResourceFeature::Translation, protected)])
                .is_empty()
        );
        assert!(
            policy
                .due(
                    Duration::from_secs(3_600),
                    [(ResourceFeature::Translation, protected)]
                )
                .is_empty()
        );
    }
}
