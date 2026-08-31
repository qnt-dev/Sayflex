//! Routing a finished recording to a transcription service.
//!
//! The user builds an ordered list in Settings; this actor walks it. Reaching
//! the local engine, or running out of services, hands the recording to the
//! inference actor exactly as before — so a list where the local engine is
//! first, which is what a fresh installation has, behaves identically to the
//! application before any of this existed.
//!
//! It runs on its own thread for the same reason installation does: a network
//! request takes seconds, and the worker must keep reading commands throughout.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::Instant,
};

use anyhow::Result;
use rust_app_core::credentials::CredentialStore;
use rust_cloud_speech::{
    Chain, HttpExecutor, ProviderEntry, ProviderId, RouteOutcome, Router, Unavailable,
    audio::{UPLOAD_SAMPLE_RATE_HZ, encode_wav},
    http::DEFAULT_TIMEOUT,
};
use rust_platform_windows::credentials::DpapiCipher;
use rust_protocol::{
    SpeechOutputMode, SpeechWorkerEvent, Transcript, TranscriptionLatency,
    TranscriptionProviderEntry, TranscriptionProviderStatus,
};

use crate::{audio::RecordedAudio, history_actor::HistoryCommand, inference::InferenceHandle};

pub enum TranscriptionCommand {
    /// Replaces the ordered list.
    SetProviders(Vec<TranscriptionProviderEntry>),
    /// Stores or clears one key.
    SetCredential {
        provider_id: String,
        secret: Option<String>,
    },
    /// Asks for the current list and its availability.
    ReportStatus,
    /// A finished recording to transcribe.
    Finalize {
        recording: Box<RecordedAudio>,
        output_mode: SpeechOutputMode,
        audio_finalize_ms: u64,
        release_started: Instant,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct TranscriptionActor {
    commands: Sender<TranscriptionCommand>,
    /// Read on the command loop for every recording, so the local path costs
    /// nothing extra: no audio is encoded and no thread is crossed unless a
    /// service is actually ahead of the local engine.
    prefers_cloud: Arc<AtomicBool>,
}

impl TranscriptionActor {
    pub fn send(&self, command: TranscriptionCommand) {
        let _ = self.commands.send(command);
    }

    pub fn prefers_cloud(&self) -> bool {
        self.prefers_cloud.load(Ordering::Acquire)
    }
}

pub fn start(
    credentials_path: PathBuf,
    events: Sender<SpeechWorkerEvent>,
    history: Sender<HistoryCommand>,
    inference: InferenceHandle,
) -> Result<TranscriptionActor> {
    let (commands, requests) = mpsc::channel::<TranscriptionCommand>();
    let prefers_cloud = Arc::new(AtomicBool::new(false));
    let worker_prefers_cloud = Arc::clone(&prefers_cloud);
    thread::Builder::new()
        .name("speech-transcription-router".to_owned())
        .spawn(move || {
            let mut state = State {
                router: Router::new(Chain::default_order()),
                credentials: CredentialStore::open(credentials_path, DpapiCipher),
                executor: HttpExecutor::new(DEFAULT_TIMEOUT).ok(),
                events,
                history,
                inference,
                prefers_cloud: worker_prefers_cloud,
            };
            if let Some(message) = state.credentials.take_recovery_message() {
                let _ = state.events.send(SpeechWorkerEvent::Failed {
                    session_id: None,
                    code: "credentials_unreadable".to_owned(),
                    message,
                    retryable: false,
                });
            }
            for command in requests {
                if matches!(command, TranscriptionCommand::Shutdown) {
                    break;
                }
                state.handle(command);
            }
        })?;
    Ok(TranscriptionActor {
        commands,
        prefers_cloud,
    })
}

struct State {
    router: Router,
    credentials: CredentialStore<DpapiCipher>,
    /// `None` when a client could not be built at all; the local engine still
    /// works, which is the point of the list.
    executor: Option<HttpExecutor>,
    events: Sender<SpeechWorkerEvent>,
    history: Sender<HistoryCommand>,
    inference: InferenceHandle,
    prefers_cloud: Arc<AtomicBool>,
}

impl State {
    /// Keeps the flag the command loop reads in step with the list, the stored
    /// keys and anything the ledger has learned.
    fn refresh_preference(&self) {
        let local_first = self.router.local_is_first(Instant::now(), |id| {
            !id.needs_credential() || self.credentials.has(id.as_str())
        });
        self.prefers_cloud.store(!local_first, Ordering::Release);
    }

    fn handle(&mut self, command: TranscriptionCommand) {
        match command {
            TranscriptionCommand::SetProviders(providers) => {
                let mut chain = Chain::new(providers.iter().map(to_entry).collect());
                // A build that knows a service the stored list has never seen
                // still offers it, switched off and last.
                chain.merge_known();
                *self.router.chain_mut() = chain;
                self.refresh_preference();
                self.report_status();
            }
            TranscriptionCommand::SetCredential {
                provider_id,
                secret,
            } => {
                let result = match secret {
                    Some(secret) => self.credentials.set(&provider_id, &secret),
                    None => self.credentials.remove(&provider_id),
                };
                match result {
                    Ok(()) => {
                        // A key the user has just corrected deserves another
                        // chance without waiting for a cooldown.
                        self.router
                            .ledger_mut()
                            .clear_credential_refusal(&ProviderId::new(&provider_id));
                    }
                    Err(error) => {
                        let _ = self.events.send(SpeechWorkerEvent::Failed {
                            session_id: None,
                            code: "credential_store".to_owned(),
                            message: format!("The key could not be stored: {error}"),
                            retryable: false,
                        });
                    }
                }
                self.refresh_preference();
                self.report_status();
            }
            TranscriptionCommand::ReportStatus => self.report_status(),
            TranscriptionCommand::Finalize {
                recording,
                output_mode,
                audio_finalize_ms,
                release_started,
            } => self.finalize(*recording, output_mode, audio_finalize_ms, release_started),
            TranscriptionCommand::Shutdown => {}
        }
    }

    fn finalize(
        &mut self,
        recording: RecordedAudio,
        output_mode: SpeechOutputMode,
        audio_finalize_ms: u64,
        release_started: Instant,
    ) {
        let Some(executor) = self.executor.as_ref() else {
            self.inference
                .finalize(recording, output_mode, audio_finalize_ms, release_started);
            return;
        };

        let wav = encode_wav(
            &crate::audio::resample_bandlimited(
                &recording.samples,
                recording.sample_rate_hz,
                UPLOAD_SAMPLE_RATE_HZ,
            ),
            UPLOAD_SAMPLE_RATE_HZ,
        );
        let session_id = recording.session_id;
        let started = Instant::now();
        let credentials = &self.credentials;
        let outcome = self.router.transcribe(
            &wav,
            None,
            Instant::now(),
            &|id: &ProviderId| credentials.secret(id.as_str()).map(str::to_owned),
            executor,
        );

        self.refresh_preference();

        for attempt in outcome_attempts(&outcome) {
            // Standard error is the worker's diagnostics channel; standard
            // output carries protocol messages only.
            eprintln!(
                "transcription provider={} result=failed reason={}",
                attempt.provider, attempt.failure
            );
        }

        match outcome {
            RouteOutcome::Transcribed { provider, text, .. } => {
                let _ = self.events.send(SpeechWorkerEvent::TranscribedRemotely {
                    session_id,
                    provider_id: provider.to_string(),
                });
                let created_unix_ms = crate::unix_ms();
                let transcript = Transcript {
                    session_id,
                    text,
                    // The services detect the language themselves and do not
                    // all report which one they chose.
                    language: "auto".to_owned(),
                    audio_duration_ms: recording.duration_ms,
                    processing_duration_ms: crate::elapsed_ms(started),
                    latency: TranscriptionLatency {
                        audio_finalize_ms,
                        after_release_ms: crate::elapsed_ms(release_started),
                        ..TranscriptionLatency::default()
                    },
                };
                // Completion reaches the user before the durable write, the
                // same order the local path uses.
                let _ = self.events.send(SpeechWorkerEvent::Completed {
                    transcript: transcript.clone(),
                    created_unix_ms,
                });
                let _ = self.history.send(HistoryCommand::Append {
                    transcript,
                    created_unix_ms,
                });
            }
            RouteOutcome::UseLocalEngine { .. } => {
                self.inference
                    .finalize(recording, output_mode, audio_finalize_ms, release_started);
            }
            RouteOutcome::Exhausted { attempts } => {
                if attempts.is_empty() {
                    let _ = self.events.send(SpeechWorkerEvent::Failed {
                        session_id: Some(session_id),
                        code: "no_transcription_provider".to_owned(),
                        message: "Every transcription provider is switched off.".to_owned(),
                        retryable: false,
                    });
                    return;
                }
                // Nothing in the list worked, and the local engine was not in
                // it. The recording is not thrown away: the last word on what
                // happened goes to the user with the reason.
                let _ = self.events.send(SpeechWorkerEvent::Failed {
                    session_id: Some(session_id),
                    code: "transcription_failed".to_owned(),
                    message: attempts.last().map_or_else(
                        || "No provider could transcribe the recording.".to_owned(),
                        |attempt| format!("{}: {}", attempt.provider, attempt.failure),
                    ),
                    retryable: true,
                });
            }
        }
    }

    fn report_status(&self) {
        let statuses = self
            .router
            .chain()
            .status(self.router.ledger(), Instant::now(), |id| {
                !id.needs_credential() || self.credentials.has(id.as_str())
            })
            .into_iter()
            .map(|status| TranscriptionProviderStatus {
                detail: describe(&status.unavailable),
                enabled: !matches!(status.unavailable, Some(Unavailable::Disabled)),
                available: status.is_available(),
                has_credential: self.credentials.has(status.id.as_str()),
                credential_hint: self.credentials.hint(status.id.as_str()),
                position: status.position,
                id: status.id.to_string(),
            })
            .collect();
        let _ = self
            .events
            .send(SpeechWorkerEvent::TranscriptionProviders { statuses });
    }
}

fn outcome_attempts(outcome: &RouteOutcome) -> &[rust_cloud_speech::Attempt] {
    match outcome {
        RouteOutcome::Transcribed { attempts, .. }
        | RouteOutcome::UseLocalEngine { attempts }
        | RouteOutcome::Exhausted { attempts } => attempts,
    }
}

fn to_entry(entry: &TranscriptionProviderEntry) -> ProviderEntry {
    ProviderEntry {
        id: ProviderId::new(&entry.id),
        enabled: entry.enabled,
        endpoint: entry.endpoint.clone(),
        model: entry.model.clone(),
        account: entry.account.clone(),
    }
}

fn describe(unavailable: &Option<rust_cloud_speech::Unavailable>) -> String {
    use rust_cloud_speech::Unavailable;
    match unavailable {
        None => String::new(),
        Some(Unavailable::Disabled) => "Switched off".to_owned(),
        Some(Unavailable::MissingCredential) => "No key stored".to_owned(),
        Some(Unavailable::Unsupported) => "Not supported by this version".to_owned(),
        Some(Unavailable::CredentialRefused(detail)) => format!("Key refused: {detail}"),
        Some(Unavailable::CoolingDown { remaining, reason }) => format!(
            "{reason}; available again in about {} min",
            remaining.as_secs().div_ceil(60)
        ),
    }
}
