use rust_protocol::{CompanionRendererEvent, InputHostEvent, TranslationWorkerEvent};

use super::{RuntimeEvent, WorkerKind};

#[test]
fn speech_install_failure_and_cancel_replace_stale_preparing_footer() {
    use std::{cell::RefCell, rc::Rc};

    use rust_protocol::{ModelSummary, SpeechOutputMode, SpeechWorkerEvent};
    use slint::ComponentHandle;

    use super::{
        AppState, SavedSettings, SettingsWindow, SpeechBackend, localization::UiLocale,
        settings_projection::Download,
    };

    i_slint_backend_testing::init_no_event_loop();
    let settings = SettingsWindow::new().unwrap();
    slint::select_bundled_translation("en").unwrap();
    let state = Rc::new(RefCell::new(AppState::new(
        "missing-selected".into(),
        SpeechOutputMode::OriginalLanguage,
        SpeechBackend::Cpu,
        &SavedSettings::default(),
    )));
    state.borrow_mut().ui_locale = UiLocale::English;
    state.borrow_mut().models.push(ModelSummary {
        catalog_id: "missing-selected".into(),
        display_name: "Whisper".into(),
        description: String::new(),
        size_bytes: 100,
        installed: false,
        selected: true,
        recommended: true,
        supports_speech_translation: true,
    });

    for code in ["model_install", "model_install_cancelled"] {
        if code == "model_install_cancelled" {
            state.borrow_mut().downloads.insert(
                "missing-selected".into(),
                Download {
                    downloaded_bytes: 35,
                    total_bytes: 100,
                },
            );
        }
        // A connection refusal can terminate before the first progress event;
        // cancellation here exercises the same footer after real progress.
        settings.set_runtime_status("Preparing model".into());
        settings.set_indicator_status("Installing model".into());
        settings.set_status_is_a_problem(false);
        assert!(super::handle_speech_install_terminal_event(
            &SpeechWorkerEvent::Failed {
                session_id: None,
                code: code.into(),
                message: "connection refused".into(),
                retryable: true,
            },
            &settings.as_weak(),
            &state,
        ));
        assert_eq!(settings.get_runtime_status(), "Not ready");
        assert_eq!(settings.get_status_is_a_problem(), code == "model_install");
        assert_eq!(
            settings.get_indicator_status(),
            if code == "model_install" {
                "model_install: connection refused"
            } else {
                "Install cancelled. The partial file is kept."
            }
        );
        assert_eq!(
            settings.get_overview_statement(),
            "The selected speech model is not installed."
        );
        assert_eq!(settings.get_overview_note_action(), "Install");
        assert!(!state.borrow().speech_ready);
        assert!(state.borrow().downloads.is_empty());
    }

    // Failing an optional install must not relabel an already loaded route or
    // an installed selected model that is still genuinely warming up.
    state.borrow_mut().models[0].installed = true;
    for (ready, expected) in [(false, "Preparing model"), (true, "Ready")] {
        state.borrow_mut().speech_ready = ready;
        settings.set_runtime_status(expected.into());
        assert!(super::handle_speech_install_terminal_event(
            &SpeechWorkerEvent::Failed {
                session_id: None,
                code: "model_install".into(),
                message: "optional install failed".into(),
                retryable: true,
            },
            &settings.as_weak(),
            &state,
        ));
        assert_eq!(settings.get_runtime_status(), expected);
    }

    {
        let mut app = state.borrow_mut();
        app.models[0].installed = false;
        app.speech_ready = false;
        app.speech_worker_ready = true;
        app.transcription_providers
            .push(rust_protocol::TranscriptionProviderStatus {
                id: "groq".into(),
                position: 0,
                enabled: true,
                available: true,
                detail: String::new(),
                has_credential: true,
                credential_hint: None,
            });
    }
    settings.set_runtime_status("Ready".into());
    assert!(super::handle_speech_install_terminal_event(
        &SpeechWorkerEvent::Failed {
            session_id: None,
            code: "model_install".into(),
            message: "optional local install failed".into(),
            retryable: true,
        },
        &settings.as_weak(),
        &state,
    ));
    assert_eq!(settings.get_runtime_status(), "Ready");
    assert_eq!(settings.get_overview_statement(), "Ready");
}

#[test]
fn speech_install_terminal_events_clear_speech_progress_and_refresh_overview_only() {
    use std::{cell::RefCell, rc::Rc};

    use rust_protocol::{ModelSummary, SpeechOutputMode, SpeechWorkerEvent};
    use slint::ComponentHandle;

    use super::{
        AppState, SavedSettings, SettingsWindow, SpeechBackend, localization::UiLocale,
        settings_projection::Download,
    };

    i_slint_backend_testing::init_no_event_loop();
    let settings = SettingsWindow::new().unwrap();
    slint::select_bundled_translation("en").unwrap();
    let state = Rc::new(RefCell::new(AppState::new(
        "queued-next".into(),
        SpeechOutputMode::OriginalLanguage,
        SpeechBackend::Cpu,
        &SavedSettings::default(),
    )));
    state.borrow_mut().ui_locale = UiLocale::English;
    for id in ["previous-active", "queued-next"] {
        state.borrow_mut().models.push(ModelSummary {
            catalog_id: id.into(),
            display_name: id.into(),
            description: String::new(),
            size_bytes: 100,
            installed: false,
            selected: id == "queued-next",
            recommended: false,
            supports_speech_translation: true,
        });
    }
    let progress = Download {
        downloaded_bytes: 35,
        total_bytes: 100,
    };
    for code in ["model_install", "model_install_cancelled"] {
        // Selection can already point at a queued model. Only progress emitted
        // by the serial speech installer belongs to the terminating operation.
        for id in [
            "previous-active",
            "translation-download",
            "unknown-download",
        ] {
            state.borrow_mut().downloads.insert(id.into(), progress);
        }
        settings.set_overview_statement("stale progress".into());
        assert!(super::handle_speech_install_terminal_event(
            &SpeechWorkerEvent::Failed {
                session_id: None,
                code: code.into(),
                message: "fixture".into(),
                retryable: true
            },
            &settings.as_weak(),
            &state,
        ));
        assert!(!state.borrow().downloads.contains_key("previous-active"));
        assert!(
            state
                .borrow()
                .downloads
                .contains_key("translation-download")
        );
        assert!(state.borrow().downloads.contains_key("unknown-download"));
        assert_eq!(state.borrow().selected_model_id, "queued-next");
        assert_eq!(
            settings.get_overview_statement(),
            "The selected speech model is not installed."
        );

        // The next queued install emits progress only after the terminal event.
        state
            .borrow_mut()
            .downloads
            .insert("queued-next".into(), progress);
        super::sync_models(&settings.as_weak(), &state);
        assert_eq!(
            settings.get_overview_statement(),
            "Downloading speech model"
        );
        state.borrow_mut().downloads.remove("queued-next");
    }
    for code in [
        "model_prepare",
        "model_busy",
        "model_remove",
        "transcription",
    ] {
        state
            .borrow_mut()
            .downloads
            .insert("previous-active".into(), progress);
        assert!(!super::handle_speech_install_terminal_event(
            &SpeechWorkerEvent::Failed {
                session_id: None,
                code: code.into(),
                message: "fixture".into(),
                retryable: true
            },
            &settings.as_weak(),
            &state,
        ));
        assert!(state.borrow().downloads.contains_key("previous-active"));
    }
    assert!(!super::handle_speech_install_terminal_event(
        &SpeechWorkerEvent::Failed {
            session_id: Some(rust_protocol::SessionId::new()),
            code: "model_install".into(),
            message: "fixture".into(),
            retryable: true
        },
        &settings.as_weak(),
        &state,
    ));
    assert!(state.borrow().downloads.contains_key("previous-active"));
}

fn input_ready(instance_id: u64) -> RuntimeEvent {
    RuntimeEvent::input(
        instance_id,
        InputHostEvent::Ready {
            protocol_version: 1,
        },
    )
}

fn translation_ready(instance_id: u64) -> RuntimeEvent {
    RuntimeEvent::translation(
        instance_id,
        TranslationWorkerEvent::Ready {
            protocol_version: 1,
            selected_catalog_id: None,
            selected_tier: None,
            models: Vec::new(),
        },
    )
}

fn translation_history(instance_id: u64) -> RuntimeEvent {
    RuntimeEvent::translation(
        instance_id,
        TranslationWorkerEvent::History {
            entries: Vec::new(),
        },
    )
}

fn companion_ready(instance_id: u64) -> RuntimeEvent {
    RuntimeEvent::companion(
        instance_id,
        CompanionRendererEvent::Ready {
            protocol_version: 1,
        },
    )
}

fn companion_home(instance_id: u64) -> RuntimeEvent {
    RuntimeEvent::companion(
        instance_id,
        CompanionRendererEvent::HomePositionChanged { x: 25, y: 50 },
    )
}

#[test]
fn buffered_generic_worker_events_cannot_reach_replacement_handlers() {
    type EventFixture = (WorkerKind, fn(u64) -> RuntimeEvent);
    let fixtures: [EventFixture; 5] = [
        (WorkerKind::Input, input_ready),
        (WorkerKind::Translation, translation_ready),
        (WorkerKind::Translation, translation_history),
        (WorkerKind::Companion, companion_ready),
        (WorkerKind::Companion, companion_home),
    ];
    for (expected_kind, event) in fixtures {
        assert!(
            event(41)
                .filter_current_worker(|kind, instance_id| {
                    assert_eq!(kind, expected_kind);
                    instance_id == 42
                })
                .is_none(),
            "stale {expected_kind:?} event reached its replacement"
        );
        let mut checked = false;
        assert!(
            event(42)
                .filter_current_worker(|kind, instance_id| {
                    checked = true;
                    assert_eq!(kind, expected_kind);
                    instance_id == 42
                })
                .is_some()
        );
        assert!(
            checked,
            "matching {expected_kind:?} must also retain its envelope"
        );
    }
}

#[test]
fn exit_events_retain_the_existing_restart_policy_owner() {
    let event = RuntimeEvent::WorkerExited {
        kind: WorkerKind::Input,
        instance_id: 41,
        exit: rust_platform_windows::ProcessExit {
            exit_code: Some(1),
            uptime: std::time::Duration::from_secs(10),
        },
    };
    assert!(
        event
            .filter_current_worker(|_, _| panic!("exit policy is separate"))
            .is_some()
    );
}

#[test]
fn speech_and_ocr_keep_their_instance_gate() {
    for instance_id in [41, 42] {
        let event = RuntimeEvent::Speech {
            instance_id,
            event: rust_protocol::SpeechWorkerEvent::History {
                entries: Vec::new(),
            },
        };
        assert_eq!(
            event
                .filter_current_worker(|kind, id| {
                    assert_eq!(kind, WorkerKind::Speech);
                    id == 42
                })
                .is_some(),
            instance_id == 42
        );
        #[cfg(feature = "screen-translation")]
        {
            let event = RuntimeEvent::Ocr {
                instance_id,
                event: rust_protocol::OcrWorkerEvent::Ready {
                    protocol_version: 1,
                    engine: "fixture".into(),
                },
            };
            assert_eq!(
                event
                    .filter_current_worker(|kind, id| {
                        assert_eq!(kind, WorkerKind::Ocr);
                        id == 42
                    })
                    .is_some(),
                instance_id == 42
            );
        }
    }
}
