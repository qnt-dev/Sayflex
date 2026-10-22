//! Activating a newly installed accelerator uses the normal worker lifecycle.
use std::{cell::RefCell, rc::Rc};

use rust_protocol::SpeechWorkerCommand;

use crate::{
    AppState, SettingsWindow, active_locale,
    localization::UiLocale,
    settings_document::save_settings,
    settings_view::sync_action_settings,
    speech_backend::{SpeechBackend, SpeechBackendPreference},
    worker_supervisor::Workers,
};

pub(crate) fn activate(
    settings: &slint::Weak<SettingsWindow>,
    state: &Rc<RefCell<AppState>>,
    workers: &Rc<Workers>,
) -> Result<(), String> {
    {
        let app = state.borrow();
        if app.registry.has_pending_work()
            || app.indicator_session.is_some()
            || !app.downloads.is_empty()
            || app.speech_unloading
            || !app.speech_worker_ready
        {
            return Err(if active_locale(app.ui_locale) == UiLocale::Russian {
                "Дождитесь завершения записи, обработки и загрузки модели, затем нажмите «Включить CUDA» ещё раз."
            } else {
                "Wait for recording, processing and model downloads to finish, then activate CUDA again."
            }.to_owned());
        }
    }
    let active = workers.switch_speech_backend(SpeechBackendPreference::Cuda)?;
    let (initialization, providers) = {
        let mut app = state.borrow_mut();
        app.speech_backend_preference = SpeechBackendPreference::Cuda;
        app.speech_backend_active = Some(active);
        app.speech_worker_ready = false;
        app.speech_ready = false;
        app.speech_reload_on_next_use = false;
        app.speech_loaded_model_id = None;
        (
            SpeechWorkerCommand::Initialize {
                selected_model_id: app.selected_model_id.clone(),
                output_mode: app.output_mode,
                dictation_language: app.dictation_language,
                history_enabled: app.history_enabled,
                fast_release_previews: app.fast_release_previews,
                prepare_local_model: app.local_speech_enabled(),
            },
            app.transcription_chain.clone(),
        )
    };
    sync_action_settings(settings, state);
    workers
        .speech
        .send(&initialization)
        .map_err(|error| error.to_string())?;
    if !providers.is_empty() {
        workers
            .speech
            .send(&SpeechWorkerCommand::SetTranscriptionProviders { providers })
            .map_err(|error| error.to_string())?;
    }
    workers
        .speech
        .send(&SpeechWorkerCommand::GetTranscriptionProviders)
        .map_err(|error| error.to_string())?;
    save_settings(&state.borrow()).map_err(|error| error.to_string())?;
    if active != SpeechBackend::Cuda {
        return Err(if active_locale(state.borrow().ui_locale) == UiLocale::Russian {
            "CUDA не запустилась. SayFlex продолжает работать через Vulkan или CPU. Проверьте совместимость видеокарты и драйвер NVIDIA."
        } else {
            "CUDA could not start. SayFlex continues using Vulkan or CPU. Check your GPU compatibility and NVIDIA driver."
        }.to_owned());
    }
    Ok(())
}
