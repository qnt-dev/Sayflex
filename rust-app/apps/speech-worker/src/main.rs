mod audio;
mod download;
mod engine;
mod history;
mod history_actor;
mod inference;
mod speculation;

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
use download::HttpFetcher;
use engine::WhisperEngine;
use history_actor::HistoryCommand;
use inference::InferenceHandle;
use rust_model_manager::{
    BuiltInCatalog, InstallPhase, ModelCatalog, ModelDescriptor, ModelInstaller, ModelTier,
};
use rust_protocol::{
    ModelSummary, PROTOCOL_VERSION, SessionId, SpeechOutputMode, SpeechWorkerCommand,
    SpeechWorkerEvent, Transcript, TranscriptionLatency,
};
use speculation::SpeculationScheduler;

fn main() -> Result<()> {
    rust_platform_windows::configure_safe_dll_search()?;
    if env::args().nth(1).as_deref() == Some("--transcribe-wav") {
        return transcribe_wav_diagnostic();
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

    if let Err(error) = run(event_sender.clone()) {
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

fn run(events: Sender<SpeechWorkerEvent>) -> Result<()> {
    let data_root = application_data_root();
    fs::create_dir_all(&data_root)?;
    let models_root = data_root.join("models");
    let history_path = data_root.join("history.json");
    let catalog = BuiltInCatalog::load()?;
    let installer = ModelInstaller::new(models_root);
    let fetcher = HttpFetcher::new()?;
    let history = history_actor::start(history_path, events.clone())?;
    let recorder = AudioRecorder::open(events.clone())?;
    let inference = InferenceHandle::start(events.clone(), history.clone())?;
    let speculation = SpeculationScheduler::start(recorder.snapshot_source(), inference.clone())?;
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
                match ensure_model(&catalog, &installer, &fetcher, &selected_model_id, &events) {
                    Ok(path) => {
                        inference.load_model(selected_model_id.clone(), path, false);
                    }
                    Err(error) => {
                        let _ = events.send(SpeechWorkerEvent::Failed {
                            session_id: None,
                            code: "model_install".to_owned(),
                            message: format!("{error:#}"),
                            retryable: true,
                        });
                    }
                }
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
                let _ = events.send(SpeechWorkerEvent::Processing { session_id });
                inference.finalize(recording, output_mode, audio_finalize_ms, release_started);
            }
            SpeechWorkerCommand::Cancel { session_id } => {
                speculation.stop(session_id);
                inference.cancel_session(session_id);
                if recorder.cancel(session_id) {
                    let _ = events.send(SpeechWorkerEvent::Cancelled { session_id });
                }
            }
            SpeechWorkerCommand::EnsureModel { catalog_id } => {
                if let Err(error) =
                    ensure_model(&catalog, &installer, &fetcher, &catalog_id, &events)
                {
                    let _ = events.send(SpeechWorkerEvent::Failed {
                        session_id: None,
                        code: "model_install".to_owned(),
                        message: format!("{error:#}"),
                        retryable: true,
                    });
                }
            }
            SpeechWorkerCommand::SelectModel { catalog_id } => {
                match ensure_model(&catalog, &installer, &fetcher, &catalog_id, &events) {
                    Ok(path) => {
                        inference.load_model(catalog_id, path, true);
                    }
                    Err(error) => {
                        let _ = events.send(SpeechWorkerEvent::Failed {
                            session_id: None,
                            code: "model_select".to_owned(),
                            message: format!("{error:#}"),
                            retryable: true,
                        });
                    }
                }
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

    speculation.shutdown();
    inference.shutdown();
    let _ = history.send(HistoryCommand::Shutdown);
    Ok(())
}

fn ensure_model(
    catalog: &ModelCatalog,
    installer: &ModelInstaller,
    fetcher: &HttpFetcher,
    catalog_id: &str,
    events: &Sender<SpeechWorkerEvent>,
) -> Result<PathBuf> {
    let model = catalog
        .find(catalog_id)
        .with_context(|| format!("unknown model: {catalog_id}"))?;
    try_adopt_existing_windows_model(installer, model);
    let path = installer.ensure_installed(model, fetcher, |progress| {
        if progress.phase != InstallPhase::Ready {
            let _ = events.send(SpeechWorkerEvent::ModelDownloadProgress {
                catalog_id: catalog_id.to_owned(),
                downloaded_bytes: progress.bytes_received,
                total_bytes: progress.total_bytes,
            });
        }
    })?;
    let _ = events.send(SpeechWorkerEvent::ModelInstalled {
        catalog_id: catalog_id.to_owned(),
        path: path.display().to_string(),
    });
    Ok(path)
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

fn try_adopt_existing_windows_model(installer: &ModelInstaller, model: &ModelDescriptor) {
    let destination = installer.model_path(model);
    if destination.exists() {
        return;
    }
    let Some(local) = env::var_os("LOCALAPPDATA") else {
        return;
    };
    let existing = Path::new(&local)
        .join("Keyboop")
        .join("models")
        .join(&model.artifact.file_name);
    if fs::metadata(&existing).is_ok_and(|metadata| metadata.len() == model.artifact.bytes) {
        if let Some(parent) = destination.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(error) = fs::hard_link(&existing, &destination) {
            eprintln!(
                "could not adopt existing model {}: {error}",
                existing.display()
            );
        }
    }
}

fn application_data_root() -> PathBuf {
    env::var_os("LOCALAPPDATA").map_or_else(
        || {
            env::current_dir()
                .unwrap_or_default()
                .join(".rust-voice-data")
        },
        |root| Path::new(&root).join("KeyboopRust"),
    )
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
