//! Live process-resource projection and explicit release of resident models.
//!
//! This page is intentionally observational until the user presses Unload.
//! Sampling starts only while Resources is visible, and releasing memory never
//! edits the selected model or removes anything from disk.

use std::{cell::RefCell, rc::Rc, time::Duration};

use rust_platform_windows::{ProcessResourceSampler, ProcessResourceUsage};
use rust_protocol::{SpeechWorkerCommand, TranslationWorkerCommand};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::{
    AppState, ResourceRow, SettingsWindow, settings_projection::human_size,
    settings_view::set_status_problem, worker_supervisor::Workers,
};

const RESOURCES_PAGE: i32 = 8;

#[derive(Clone, Copy)]
enum ResourceFeature {
    Speech,
    Translation,
    Ocr,
}

pub(crate) fn wire_resources(
    settings: &SettingsWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) -> Timer {
    let sampler = Rc::new(RefCell::new(ProcessResourceSampler::new()));
    sync(settings, state, workers, &sampler, false);

    {
        let settings = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
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
                    unload(feature, &settings, &state, &workers);
                }
                if let Some(settings) = settings.upgrade() {
                    sync(&settings, &state, &workers, &sampler, true);
                }
            });
    }

    {
        let settings = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
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
                    unload(feature, &settings, &state, &workers);
                }
                if let Some(settings) = settings.upgrade() {
                    sync(&settings, &state, &workers, &sampler, true);
                }
            });
    }

    let timer = Timer::default();
    {
        let settings = settings.as_weak();
        let state = Rc::clone(state);
        let workers = Rc::clone(workers);
        timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            let Some(settings) = settings.upgrade() else {
                return;
            };
            if settings.window().is_visible() && settings.get_active_page() == RESOURCES_PAGE {
                sync(&settings, &state, &workers, &sampler, true);
            }
        });
    }
    timer
}

fn unload(
    feature: ResourceFeature,
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) {
    let result = match feature {
        ResourceFeature::Speech => {
            let busy = state.borrow().indicator_session.is_some();
            if busy {
                return;
            }
            let loaded = state.borrow().speech_loaded_model_id.is_some();
            if !loaded {
                return;
            }
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
            let state_ref = state.borrow();
            let busy = !state_ref.pending_translations.is_empty()
                || state_ref.queued_model_translation.is_some()
                || state_ref.queued_selection_translation.is_some()
                || {
                    #[cfg(feature = "screen-translation")]
                    {
                        state_ref.queued_screen_translation.is_some()
                    }
                    #[cfg(not(feature = "screen-translation"))]
                    {
                        false
                    }
                };
            let loaded = state_ref.translation_loaded_model_id.is_some();
            drop(state_ref);
            if busy || !loaded {
                return;
            }
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
        ResourceFeature::Ocr => workers.unload_ocr().map(|_| ()),
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

fn sync(
    settings: &SettingsWindow,
    state: &Rc<RefCell<AppState>>,
    workers: &Workers,
    sampler: &Rc<RefCell<ProcessResourceSampler>>,
    sample: bool,
) {
    let mut roots = vec![workers.speech_pid(), workers.translation_pid()];
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
    let speech_busy = state.indicator_session.is_some();
    let translation_busy =
        !state.pending_translations.is_empty() || state.queued_selection_translation.is_some() || {
            #[cfg(feature = "screen-translation")]
            {
                state.queued_screen_translation.is_some()
            }
            #[cfg(not(feature = "screen-translation"))]
            {
                false
            }
        };
    let ocr_busy = workers.ocr_busy();

    let speech_loaded = state.speech_loaded_model_id.is_some();
    let translation_loaded = state.translation_loaded_model_id.is_some();
    let ocr_loaded = ocr_index.is_some();
    let rows = vec![
        row(
            "speech",
            samples.first().copied(),
            speech_loaded,
            speech_busy,
            state.speech_unloading,
        ),
        row(
            "translation",
            samples.get(1).copied(),
            translation_loaded,
            translation_busy,
            state.translation_unloading,
        ),
        row(
            "ocr",
            ocr_index.and_then(|index| samples.get(index).copied()),
            ocr_loaded,
            ocr_busy,
            false,
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
        can_unload: loaded && !busy && !releasing,
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
    use super::{metric_bytes, metric_percent};

    #[test]
    fn unavailable_metrics_are_not_presented_as_real_zeroes() {
        assert_eq!(metric_percent(None), "—");
        assert_eq!(metric_bytes(None), "—");
    }
}
