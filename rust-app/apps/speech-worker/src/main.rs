mod audio;
#[cfg(test)]
#[path = "../build_metadata.rs"]
mod build_metadata;
mod engine;
mod history;
mod history_actor;
mod inference;
mod model_actor;
mod speculation;
mod transcription;
mod transcription_mailbox;

use std::{
    env, fs,
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Sender},
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use audio::AudioRecorder;
use engine::WhisperEngine;
use history_actor::HistoryCommand;
use inference::InferenceHandle;
use model_actor::{ModelCommand, model_summaries};
use rust_model_manager::{BuiltInCatalog, HttpFetcher, ModelCatalog, ModelInstaller};
use rust_protocol::{
    PROTOCOL_VERSION, SessionId, SpeechOutputMode, SpeechWorkerCommand, SpeechWorkerEvent,
    Transcript, TranscriptionLatency,
};
use speculation::SpeculationScheduler;
use transcription::TranscriptionCommand;

/// Whether this build can run a speculative pass without the final one paying
/// for it.
///
/// There is one inference actor. On a GPU a speculative pass over the
/// uncommitted tail finishes in a fraction of a second, so the final job that
/// queues behind it waits for almost nothing and the preview is free. On the
/// CPU build the same pass takes seconds, and the final job — the one whose
/// text actually gets typed — waits for every one of them: a dictation that
/// should have taken a hundred seconds took a hundred and ninety-five, and the
/// difference was entirely previews nobody read.
///
/// So the CPU build does not speculate, whatever the setting asks for. The
/// setting still governs the accelerated builds, where it costs what it says
/// it costs.
const SPECULATION_IS_AFFORDABLE: bool = cfg!(any(feature = "cuda", feature = "vulkan"));

/// Whether previews may run, given what the setting asks for.
///
/// Every path that changes the setting goes through here. Initialize did, and
/// `SetFastReleasePreviews` did not, so the CPU build honoured the limit until
/// the moment the user touched the switch in Settings and then ran the
/// speculation anyway — the exact case the limit exists for.
const fn previews_allowed(requested: bool) -> bool {
    requested && SPECULATION_IS_AFFORDABLE
}

/// The router owns every cloud-first request and every request for which the
/// local route is disabled. The second clause makes a freshly changed
/// cloud-only configuration safe even before the router actor has observed
/// its own copy of the provider list.
const fn use_routing_actor(local_model_enabled: bool, router_prefers_cloud: bool) -> bool {
    !local_model_enabled || router_prefers_cloud
}

fn main() -> Result<()> {
    rust_platform_windows::configure_safe_dll_search()?;
    // Asked by the release check, which otherwise has no way to find out what a
    // packaged worker speaks short of driving a whole handshake.
    if env::args().any(|argument| argument == "--build-id") {
        println!("{}", rust_protocol::BUILD_COMMIT);
        return Ok(());
    }
    if env::args().any(|argument| argument == "--protocol-version") {
        println!("{PROTOCOL_VERSION}");
        return Ok(());
    }
    if env::args().nth(1).as_deref() == Some("--transcribe-wav") {
        return transcribe_wav_diagnostic();
    }
    let protocol_smoke = env::args().any(|argument| argument == "--protocol-smoke");
    if protocol_smoke {
        require_smoke_opt_in("SAYFLEX_RUN_PROTOCOL_SMOKE")?;
    }

    let (event_sender, event_receiver) = mpsc::channel::<SpeechWorkerEvent>();
    thread::Builder::new()
        .name("speech-worker-json-output".to_owned())
        .spawn(move || {
            let stdout = io::stdout();
            let mut writer = BufWriter::new(stdout.lock());
            for event in event_receiver {
                if serde_json::to_writer(&mut writer, &event).is_err()
                    || writer.write_all(b"\n").is_err()
                    || writer.flush().is_err()
                {
                    break;
                }
            }
        })?;

    if let Err(error) = run(event_sender.clone(), protocol_smoke) {
        let _ = event_sender.send(SpeechWorkerEvent::Failed {
            session_id: None,
            code: "worker_fatal".to_owned(),
            message: format!("{error:#}"),
            retryable: false,
        });
        return Err(error);
    }
    Ok(())
}

/// Refuses a test-only mode unless the caller asked for it explicitly.
///
/// These modes are reachable in a release build, so the opt-in is what keeps
/// them out of the ordinary application lifecycle.
fn require_smoke_opt_in(variable: &str) -> Result<()> {
    if env::var(variable).is_ok_and(|value| value == "1") {
        return Ok(());
    }
    anyhow::bail!("{variable}=1 is required for this diagnostic mode")
}

fn transcribe_wav_diagnostic() -> Result<()> {
    let mut arguments = env::args().skip(2);
    let wav_path = arguments
        .next()
        .context("--transcribe-wav requires a WAV path")?;
    let model_path = arguments
        .next()
        .context("--transcribe-wav requires a model path")?;
    let output_mode = match arguments.next().as_deref() {
        None | Some("--original-language") => SpeechOutputMode::OriginalLanguage,
        Some("--translate-to-english") => SpeechOutputMode::TranslateToEnglish,
        Some(argument) => {
            anyhow::bail!(
                "unsupported --transcribe-wav output mode {argument}; use \
                 --original-language or --translate-to-english"
            );
        }
    };
    let mut reader = hound::WavReader::open(&wav_path)?;
    let specification = reader.spec();
    let channels = usize::from(specification.channels.max(1));
    let interleaved = match specification.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|sample| sample.map(|sample| f32::from(sample) / f32::from(i16::MAX)))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?,
    };
    let samples = interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect::<Vec<_>>();
    let duration_ms = samples.len() as u64 * 1000 / u64::from(specification.sample_rate.max(1));
    let recording = audio::RecordedAudio {
        session_id: SessionId::new(),
        source_start_sample: 0,
        source_total_samples: samples.len(),
        samples,
        sample_rate_hz: specification.sample_rate,
        duration_ms,
    };
    let mut engine = WhisperEngine::new();
    engine.prepare(Path::new(&model_path))?;
    let started = Instant::now();
    let result = engine.transcribe(&recording, output_mode, None)?;
    let transcript = Transcript {
        session_id: recording.session_id,
        text: result.text,
        language: result.language,
        audio_duration_ms: result.audio_duration_ms,
        processing_duration_ms: result.inference_ms,
        provider: None,
        latency: TranscriptionLatency {
            preprocessing_ms: result.preprocessing_ms,
            inference_ms: result.inference_ms,
            after_release_ms: elapsed_ms(started),
            ..Default::default()
        },
    };
    println!("{}", serde_json::to_string(&transcript)?);
    Ok(())
}

/// Runs the worker.
///
/// `protocol_smoke` keeps everything that speaks the protocol and leaves out
/// everything that needs hardware: no microphone is opened and no model is
/// loaded. It exists so a test can start this exact binary and check the
/// handshake, the version and the shape of the replies — the contract the
/// desktop depends on — on a machine with no sound card and no gigabytes to
/// download. Recording commands answer with a failure rather than pretending.
fn run(events: Sender<SpeechWorkerEvent>, protocol_smoke: bool) -> Result<()> {
    let data_root = application_data_root();
    fs::create_dir_all(&data_root)?;
    let models_root = data_root.join("models");
    let history_path = data_root.join("history.json");
    let catalog = BuiltInCatalog::load()?;
    let installer = ModelInstaller::new(models_root);
    let fetcher = HttpFetcher::new("model-manager")?;
    let history = history_actor::start(history_path, events.clone())?;
    if protocol_smoke {
        return run_protocol_smoke(&events, &catalog, &installer);
    }
    let mut recorder = AudioRecorder::open(events.clone())?;
    let inference = InferenceHandle::start(events.clone(), history.clone())?;
    let models = model_actor::start(
        catalog.clone(),
        installer.clone(),
        fetcher,
        events.clone(),
        inference.clone(),
    )?;
    let speculation = SpeculationScheduler::start(recorder.snapshot_source(), inference.clone())?;
    let transcription = transcription::start(
        data_root.join("credentials.dat"),
        events.clone(),
        history.clone(),
        inference.clone(),
    )?;
    let mut output_mode = SpeechOutputMode::OriginalLanguage;
    let mut fast_release_previews = previews_allowed(true);
    let mut selected_model_id = catalog.default_model.clone();
    let mut local_model_enabled = true;

    let stdin = io::stdin();
    for line in BufReader::new(stdin.lock()).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let command = match serde_json::from_str::<SpeechWorkerCommand>(&line) {
            Ok(command) => command,
            Err(error) => {
                let _ = events.send(SpeechWorkerEvent::Failed {
                    session_id: None,
                    code: "invalid_command".to_owned(),
                    message: error.to_string(),
                    retryable: false,
                });
                continue;
            }
        };

        match command {
            SpeechWorkerCommand::Initialize {
                selected_model_id: requested,
                output_mode: requested_output_mode,
                dictation_language: requested_dictation_language,
                history_enabled,
                fast_release_previews: requested_fast_release_previews,
                prepare_local_model,
            } => {
                let _ = history.send(HistoryCommand::SetEnabled {
                    enabled: history_enabled,
                });
                output_mode = requested_output_mode;
                inference.set_language(requested_dictation_language);
                transcription.send(TranscriptionCommand::SetLanguage(
                    requested_dictation_language,
                ));
                fast_release_previews = previews_allowed(requested_fast_release_previews);
                selected_model_id = if catalog.find(&requested).is_some() {
                    requested
                } else {
                    catalog.default_model.clone()
                };
                local_model_enabled = prepare_local_model;
                let _ = events.send(SpeechWorkerEvent::Ready {
                    protocol_version: PROTOCOL_VERSION,
                    selected_model_id: selected_model_id.clone(),
                    models: model_summaries(&catalog, &installer, &selected_model_id),
                });
                if local_model_enabled {
                    models.send(ModelCommand::Ensure {
                        catalog_id: selected_model_id.clone(),
                        announce_selection: false,
                    });
                } else {
                    models.deactivate();
                }
            }
            SpeechWorkerCommand::Begin {
                session_id,
                started_unix_ms: _,
            } => match recorder.begin(session_id) {
                Ok(()) => {
                    inference.begin_session(session_id);
                    if !use_routing_actor(local_model_enabled, transcription.prefers_cloud()) {
                        speculation.begin(session_id, output_mode, fast_release_previews);
                    }
                    let _ = events.send(SpeechWorkerEvent::Listening { session_id });
                }
                Err(error) => {
                    let _ = events.send(SpeechWorkerEvent::Failed {
                        session_id: Some(session_id),
                        code: "recording_begin".to_owned(),
                        message: error.to_string(),
                        retryable: true,
                    });
                }
            },
            SpeechWorkerCommand::End { session_id } => {
                speculation.stop(session_id);
                let release_started = Instant::now();
                // The recorder keeps a short sample-counted post-roll so the OS audio
                // buffer cannot clip the final phoneme. The UI should still leave
                // Listening immediately when the physical gesture ends.
                let _ = events.send(SpeechWorkerEvent::Processing { session_id });
                let audio_finalize_started = Instant::now();
                let recording = match recorder.end(session_id) {
                    Ok(recording) => recording,
                    Err(error) => {
                        inference.cancel_session(session_id);
                        let _ = events.send(SpeechWorkerEvent::Failed {
                            session_id: Some(session_id),
                            code: "recording_end".to_owned(),
                            message: error.to_string(),
                            retryable: true,
                        });
                        continue;
                    }
                };
                let audio_finalize_ms = elapsed_ms(audio_finalize_started);
                if use_routing_actor(local_model_enabled, transcription.prefers_cloud()) {
                    transcription.send(TranscriptionCommand::Finalize {
                        recording: Box::new(recording),
                        output_mode,
                        audio_finalize_ms,
                        release_started,
                    });
                } else {
                    // Unchanged local path: no encoding, no extra thread.
                    inference.finalize(recording, output_mode, audio_finalize_ms, release_started);
                }
            }
            SpeechWorkerCommand::Cancel { session_id } => {
                speculation.stop(session_id);
                inference.cancel_session(session_id);
                // A walk already under way stops between providers, and a
                // late answer is not delivered into a window the user has
                // moved on from.
                transcription.cancel(session_id);
                if recorder.cancel(session_id) {
                    let _ = events.send(SpeechWorkerEvent::Cancelled { session_id });
                }
            }
            SpeechWorkerCommand::EnsureModel { catalog_id } => {
                models.send(ModelCommand::Ensure {
                    catalog_id,
                    announce_selection: false,
                });
            }
            SpeechWorkerCommand::CancelModelInstall => models.cancel(),
            SpeechWorkerCommand::SetTranscriptionProviders { providers } => {
                let enable_local = providers
                    .iter()
                    .any(|provider| provider.id == "local" && provider.enabled);
                if enable_local != local_model_enabled {
                    local_model_enabled = enable_local;
                    if enable_local {
                        models.send(ModelCommand::Ensure {
                            catalog_id: selected_model_id.clone(),
                            announce_selection: false,
                        });
                    } else {
                        models.deactivate();
                    }
                }
                transcription.send(TranscriptionCommand::SetProviders(providers));
            }
            SpeechWorkerCommand::SetProviderCredential {
                operation_id,
                provider_id,
                secret,
            } => {
                transcription.send(TranscriptionCommand::SetCredential {
                    operation_id,
                    provider_id,
                    secret,
                });
            }
            SpeechWorkerCommand::GetTranscriptionProviders => {
                transcription.send(TranscriptionCommand::ReportStatus);
            }
            SpeechWorkerCommand::SelectModel { catalog_id } => {
                // Choosing another model abandons an installation that is
                // still running for the previous one.
                models.cancel();
                selected_model_id.clone_from(&catalog_id);
                if local_model_enabled {
                    models.send(ModelCommand::Ensure {
                        catalog_id,
                        announce_selection: true,
                    });
                }
            }
            SpeechWorkerCommand::RemoveModel { catalog_id } => {
                if local_model_enabled && catalog_id == selected_model_id {
                    let _ = events.send(SpeechWorkerEvent::Failed {
                        session_id: None,
                        code: "model_in_use".to_owned(),
                        message: "Switch off the local speech provider before removing its selected model."
                            .to_owned(),
                        retryable: true,
                    });
                } else {
                    models.remove(catalog_id, selected_model_id.clone());
                }
            }
            SpeechWorkerCommand::SetOutputMode {
                output_mode: requested,
            } => {
                output_mode = requested;
            }
            SpeechWorkerCommand::SetDictationLanguage { language } => {
                inference.set_language(language);
                transcription.send(TranscriptionCommand::SetLanguage(language));
            }
            SpeechWorkerCommand::SetFastReleasePreviews { enabled } => {
                fast_release_previews = previews_allowed(enabled);
            }
            SpeechWorkerCommand::GetHistory { limit } => {
                let _ = history.send(HistoryCommand::Latest { limit });
            }
            SpeechWorkerCommand::SetHistoryEnabled { enabled } => {
                let _ = history.send(HistoryCommand::SetEnabled { enabled });
            }
            SpeechWorkerCommand::ClearHistory => {
                let _ = history.send(HistoryCommand::Clear);
            }
            SpeechWorkerCommand::Shutdown => break,
        }
    }

    models.cancel();
    models.send(ModelCommand::Shutdown);
    transcription.send(TranscriptionCommand::Shutdown);
    speculation.shutdown();
    inference.shutdown();
    let _ = history.send(HistoryCommand::Shutdown);
    Ok(())
}

/// The command loop with nothing behind it but the catalogue.
fn run_protocol_smoke(
    events: &Sender<SpeechWorkerEvent>,
    catalog: &ModelCatalog,
    installer: &ModelInstaller,
) -> Result<()> {
    let stdin = io::stdin();
    for line in BufReader::new(stdin.lock()).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let command = match serde_json::from_str::<SpeechWorkerCommand>(&line) {
            Ok(command) => command,
            Err(error) => {
                let _ = events.send(SpeechWorkerEvent::Failed {
                    session_id: None,
                    code: "invalid_command".to_owned(),
                    message: error.to_string(),
                    retryable: false,
                });
                continue;
            }
        };
        match command {
            SpeechWorkerCommand::Initialize {
                selected_model_id, ..
            } => {
                let selected_model_id = if catalog.find(&selected_model_id).is_some() {
                    selected_model_id
                } else {
                    catalog.default_model.clone()
                };
                let _ = events.send(SpeechWorkerEvent::Ready {
                    protocol_version: PROTOCOL_VERSION,
                    selected_model_id: selected_model_id.clone(),
                    models: model_summaries(catalog, installer, &selected_model_id),
                });
            }
            SpeechWorkerCommand::Begin { session_id, .. }
            | SpeechWorkerCommand::End { session_id }
            | SpeechWorkerCommand::Cancel { session_id } => {
                let _ = events.send(SpeechWorkerEvent::Failed {
                    session_id: Some(session_id),
                    code: "protocol_smoke".to_owned(),
                    message: "this diagnostic mode records nothing".to_owned(),
                    retryable: false,
                });
            }
            SpeechWorkerCommand::GetHistory { .. } => {
                let _ = events.send(SpeechWorkerEvent::History {
                    entries: Vec::new(),
                });
            }
            SpeechWorkerCommand::Shutdown => break,
            _ => {}
        }
    }
    Ok(())
}

fn application_data_root() -> PathBuf {
    rust_app_core::product::data_root()
}

pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod speculation_policy_tests {
    use super::{SPECULATION_IS_AFFORDABLE, previews_allowed, use_routing_actor};

    #[test]
    fn the_setting_can_switch_previews_off_but_never_back_on_where_they_cost_too_much() {
        assert!(!previews_allowed(false), "off is always off");
        assert_eq!(
            previews_allowed(true),
            SPECULATION_IS_AFFORDABLE,
            "asking for previews only gets them on a build that can afford them"
        );
    }

    #[test]
    fn the_cpu_build_never_speculates() {
        if cfg!(not(any(feature = "cuda", feature = "vulkan"))) {
            assert!(
                !previews_allowed(true),
                "one inference actor, and the final job waits behind every preview"
            );
        }
    }

    #[test]
    fn cloud_only_never_takes_the_local_path_during_a_provider_update() {
        assert!(use_routing_actor(false, false));
        assert!(use_routing_actor(false, true));
        assert!(!use_routing_actor(true, false));
        assert!(use_routing_actor(true, true));
    }
}
