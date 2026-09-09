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
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::Instant,
};

use anyhow::Result;
use rust_app_core::credentials::CredentialStore;
use rust_cloud_speech::{
    Chain, HttpExecutor, LanguageHint, ProviderEntry, ProviderId, RouteOutcome, Router,
    TranscriptionRequest, Unavailable,
    audio::{UPLOAD_SAMPLE_RATE_HZ, encode_wav},
    http::DEFAULT_TIMEOUT,
    router::CancelSignal,
};
use rust_platform_windows::credentials::DpapiCipher;
use rust_protocol::{
    RequestId, SessionId, SpeechOutputMode, SpeechWorkerEvent, Transcript, TranscriptionLatency,
    TranscriptionProviderEntry, TranscriptionProviderStatus,
};

use crate::{audio::RecordedAudio, history_actor::HistoryCommand, inference::InferenceHandle};

pub enum TranscriptionCommand {
    /// Replaces the ordered list.
    SetProviders(Vec<TranscriptionProviderEntry>),
    /// Stores or clears one key.
    SetCredential {
        /// Named so the window that is waiting can recognise the answer. The
        /// write happens here, on another thread in another process, long
        /// after the command was put in the pipe.
        operation_id: RequestId,
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

/// The session a `Cancel` command named, seen by the routing thread while it
/// is between providers.
///
/// The command loop cannot interrupt a request in flight, but it can stop the
/// walk from continuing down a list nobody is waiting for, and it can stop a
/// late answer from being delivered into a window the user has moved on from.
#[derive(Clone, Default)]
struct CancelledSession(Arc<Mutex<Option<SessionId>>>);

impl CancelledSession {
    fn cancel(&self, session_id: SessionId) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session_id);
    }

    fn watcher(&self, session_id: SessionId) -> SessionWatcher {
        SessionWatcher {
            cancelled: self.clone(),
            session_id,
        }
    }
}

struct SessionWatcher {
    cancelled: CancelledSession,
    session_id: SessionId,
}

impl CancelSignal for SessionWatcher {
    fn is_cancelled(&self) -> bool {
        *self
            .cancelled
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            == Some(self.session_id)
    }
}

#[derive(Clone)]
pub struct TranscriptionActor {
    commands: Sender<TranscriptionCommand>,
    cancelled: CancelledSession,
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

    /// Tells the routing thread that this dictation no longer has a waiting
    /// user. Safe to call for a session it never saw.
    pub fn cancel(&self, session_id: SessionId) {
        self.cancelled.cancel(session_id);
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
    let cancelled = CancelledSession::default();
    let worker_cancelled = cancelled.clone();
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
                cancelled: worker_cancelled,
                language: system_language(),
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
        cancelled,
    })
}

/// The fallback language for services that cannot detect one themselves.
///
/// This is deliberately not forwarded to providers that support automatic
/// detection. The Windows UI language is useful as a last resort for Azure's
/// fixed-language endpoint, but says nothing reliable about what the user is
/// speaking to Groq or Deepgram.
fn system_language() -> Option<LanguageHint> {
    sys_locale::get_locale().and_then(|locale| LanguageHint::new(&locale))
}

fn reported_language(provider: &ProviderId, fallback: Option<&LanguageHint>) -> String {
    if provider.as_str() == ProviderId::AZURE {
        fallback.map_or_else(|| "unknown".to_owned(), |hint| hint.primary().to_owned())
    } else {
        "auto".to_owned()
    }
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
    cancelled: CancelledSession,
    language: Option<LanguageHint>,
}

impl State {
    /// Keeps the flag the command loop reads in step with the list, the stored
    /// keys and anything the ledger has learned.
    fn refresh_preference(&self) {
        // Deliberately a question about the configuration, not about the
        // ledger: a value that changed as cooldowns came and went would leave
        // the command loop taking the direct local path at the exact moment a
        // recovered service should have been tried again, with nothing to
        // recompute it afterwards.
        let routing_required = self
            .router
            .chain()
            .routing_required(|id| !id.needs_credential() || self.credentials.has(id.as_str()));
        self.prefers_cloud
            .store(routing_required, Ordering::Release);
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
                operation_id,
                provider_id,
                secret,
            } => {
                let result = match secret {
                    Some(secret) => self.credentials.set(&provider_id, &secret),
                    None => self.credentials.remove(&provider_id),
                };
                // Answered either way, because a window is waiting on it and a
                // silence is indistinguishable from a success.
                let _ = self.events.send(SpeechWorkerEvent::CredentialSaved {
                    operation_id,
                    provider_id: provider_id.clone(),
                    error: result
                        .as_ref()
                        .err()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                });
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

        let session_id = recording.session_id;
        let wav = encode_wav(
            &crate::audio::resample_bandlimited(
                &recording.samples,
                recording.sample_rate_hz,
                UPLOAD_SAMPLE_RATE_HZ,
            ),
            UPLOAD_SAMPLE_RATE_HZ,
        );
        let request = TranscriptionRequest {
            audio_duration_ms: recording.duration_ms,
            upload_bytes: wav.len() as u64,
            translate_to_english: output_mode == SpeechOutputMode::TranslateToEnglish,
            language: self.language.clone(),
        };
        let started = Instant::now();
        let credentials = &self.credentials;
        let watcher = self.cancelled.watcher(session_id);
        let outcome = self.router.transcribe(
            &wav,
            &request,
            started,
            &|id: &ProviderId| credentials.secret(id.as_str()).map(str::to_owned),
            executor,
            &watcher,
        );

        self.refresh_preference();
        self.report_attempts(&outcome);

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
                    language: reported_language(&provider, request.language.as_ref()),
                    audio_duration_ms: recording.duration_ms,
                    processing_duration_ms: crate::elapsed_ms(started),
                    provider: Some(provider.to_string()),
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
            RouteOutcome::Cancelled { .. } => {
                let _ = self
                    .events
                    .send(SpeechWorkerEvent::Cancelled { session_id });
            }
            RouteOutcome::NoSpeech { .. } => {
                // The same answer the local engine gives, in the same words.
                let _ = self.events.send(SpeechWorkerEvent::Failed {
                    session_id: Some(session_id),
                    code: "transcription".to_owned(),
                    message: "no speech was recognized".to_owned(),
                    retryable: true,
                });
            }
            RouteOutcome::Exhausted { attempts } => {
                let message = if attempts.is_empty() {
                    "Every transcription provider is switched off.".to_owned()
                } else {
                    attempts.last().map_or_else(
                        || "No provider could transcribe the recording.".to_owned(),
                        |attempt| format!("{}: {}", attempt.provider, attempt.reason()),
                    )
                };
                let _ = self.events.send(SpeechWorkerEvent::Failed {
                    session_id: Some(session_id),
                    code: "transcription_failed".to_owned(),
                    message,
                    retryable: true,
                });
            }
        }
    }

    /// Puts every step of the walk in the diagnostics, with any key the
    /// service echoed back removed first.
    fn report_attempts(&self, outcome: &RouteOutcome) {
        for attempt in outcome_attempts(outcome) {
            let mut reason = attempt.reason();
            if let Some(secret) = self.credentials.secret(attempt.provider.as_str()) {
                reason = rust_cloud_speech::redact_secret(&reason, secret);
            }
            // Standard error is the worker's diagnostics channel; standard
            // output carries protocol messages only.
            eprintln!(
                "transcription provider={} result=not_used reason={reason}",
                attempt.provider
            );
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
        | RouteOutcome::Cancelled { attempts }
        | RouteOutcome::NoSpeech { attempts }
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
        Some(Unavailable::Incapable(reason)) => format!("Not for this recording: {reason}"),
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

#[cfg(test)]
mod language_policy_tests {
    use rust_cloud_speech::{LanguageHint, ProviderId};

    use super::reported_language;

    #[test]
    fn an_english_windows_locale_is_not_reported_as_groqs_detected_language() {
        let windows_locale = LanguageHint::new("en-US");

        assert_eq!(
            reported_language(&ProviderId::new(ProviderId::GROQ), windows_locale.as_ref()),
            "auto"
        );
        assert_eq!(
            reported_language(
                &ProviderId::new(ProviderId::DEEPGRAM),
                windows_locale.as_ref()
            ),
            "auto"
        );
        assert_eq!(
            reported_language(&ProviderId::new(ProviderId::AZURE), windows_locale.as_ref()),
            "en"
        );
    }
}
