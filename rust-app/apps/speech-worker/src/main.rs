mod audio;
mod engine;
mod history;
mod history_actor;
mod inference;
mod model_actor;
mod speculation;
mod transcription;

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
use model_actor::ModelCommand;
use rust_model_manager::{BuiltInCatalog, HttpFetcher, ModelCatalog, ModelInstaller, ModelTier};
use rust_protocol::{
    ModelSummary, PROTOCOL_VERSION, SessionId, SpeechOutputMode, SpeechWorkerCommand,
    SpeechWorkerEvent, Transcript, TranscriptionLatency,
};
use speculation::SpeculationScheduler;
use transcription::TranscriptionCommand;

fn main() -> Result<()> {
    rust_platform_windows::configure_safe_dll_search()?;
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
    let recorder = AudioRecorder::open(events.clone())?;
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
    let mut fast_release_previews = true;

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
                history_enabled,
                fast_release_previews: requested_fast_release_previews,
            } => {
                let _ = history.send(HistoryCommand::SetEnabled {
                    enabled: history_enabled,
                });
                output_mode = requested_output_mode;
                fast_release_previews = requested_fast_release_previews;
                let selected_model_id = if catalog.find(&requested).is_some() {
                    requested
                } else {
                    catalog.default_model.clone()
                };
                let _ = events.send(SpeechWorkerEvent::Ready {
                    protocol_version: PROTOCOL_VERSION,
                    selected_model_id: selected_model_id.clone(),
                    models: model_summaries(&catalog, &installer, &selected_model_id),
                });
                models.send(ModelCommand::Ensure {
                    catalog_id: selected_model_id,
                    announce_selection: false,
                });
            }
            SpeechWorkerCommand::Begin {
                session_id,
                started_unix_ms: _,
            } => match recorder.begin(session_id) {
                Ok(()) => {
                    inference.begin_session(session_id);
                    speculation.begin(session_id, output_mode, fast_release_previews);
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
                if transcription.prefers_cloud() {
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
                transcription.send(TranscriptionCommand::SetProviders(providers));
            }
            SpeechWorkerCommand::SetProviderCredential {
                provider_id,
                secret,
            } => {
                transcription.send(TranscriptionCommand::SetCredential {
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
                models.send(ModelCommand::Ensure {
                    catalog_id,
                    announce_selection: true,
                });
            }
            SpeechWorkerCommand::SetOutputMode {
                output_mode: requested,
            } => {
                output_mode = requested;
            }
            SpeechWorkerCommand::SetFastReleasePreviews { enabled } => {
                fast_release_previews = enabled;
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

fn model_summaries(
    catalog: &ModelCatalog,
    installer: &ModelInstaller,
    selected: &str,
) -> Vec<ModelSummary> {
    catalog
        .models
        .iter()
        .map(|model| ModelSummary {
            catalog_id: model.id.clone(),
            display_name: model.display_name.clone(),
            description: model.description.clone(),
            size_bytes: model.artifact.bytes,
            installed: installer.is_installed(model).unwrap_or(false),
            selected: model.id == selected,
            recommended: model.tier == ModelTier::Recommended,
            supports_speech_translation: model.supports_speech_translation,
        })
        .collect()
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
