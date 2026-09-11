#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod history;
mod provider_actor;
mod providers;
mod runtime;

use std::{
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use history::TranslationHistory;
use runtime::{LlamaRuntimeManager, RUNTIME_DOWNLOAD_BYTES};
use rust_model_manager::{BundleInstaller, CancelToken, HttpFetcher, ModelInstallError};
use rust_protocol::{
    PROTOCOL_VERSION, RequestId, TranslationHistoryEntry, TranslationLanguage,
    TranslationLanguagePolicy, TranslationModelSummary, TranslationProviderEntry,
    TranslationResult, TranslationTier, TranslationWorkerCommand, TranslationWorkerEvent,
};
use rust_translation_engine::{
    TranslationCancelled, TranslationCatalog, TranslationEngine, TranslationPackDescriptor,
    detect_language,
};

use crate::provider_actor::ProviderActorHandle;
use crate::providers::{Providers, Routed};

#[derive(Clone)]
struct EventWriter {
    events: Sender<TranslationWorkerEvent>,
}

impl EventWriter {
    fn start() -> Result<(Self, JoinHandle<()>)> {
        let (events, receiver) = mpsc::channel::<TranslationWorkerEvent>();
        let thread = thread::Builder::new()
            .name("translation-worker-json-output".to_owned())
            .spawn(move || {
                let mut writer = BufWriter::new(io::stdout());
                for event in receiver {
                    if serde_json::to_writer(&mut writer, &event).is_err()
                        || writer.write_all(b"\n").is_err()
                        || writer.flush().is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok((Self { events }, thread))
    }

    fn send(&self, event: &TranslationWorkerEvent) -> Result<()> {
        self.events
            .send(event.clone())
            .map_err(|error| anyhow!("translation event stream closed: {error}"))
    }
}

/// Installation commands, run on their own thread.
///
/// Installing a pack downloads the llama.cpp runtime and several gigabytes of
/// weights. Doing that inside the loop that reads standard input stopped the
/// worker from reading anything else for the duration, so neither a
/// cancellation nor a shutdown could reach it.
enum InstallCommand {
    Initialize {
        selected_tier: Option<TranslationTier>,
        keep_warm: bool,
        history_enabled: bool,
        prepare_local_model: bool,
        skip_local_prepare: bool,
    },
    EnsureModel {
        tier: TranslationTier,
    },
    RemoveModel {
        tier: TranslationTier,
    },
    SelectTier {
        tier: TranslationTier,
        keep_warm: bool,
    },
    SetLocalEnabled {
        enabled: bool,
    },
    Shutdown,
}

struct QueuedInstallCommand {
    command: InstallCommand,
    cancels_at_send: u64,
}

/// Applies cancellations that happened while a command was waiting in the
/// mailbox. Expensive work and a request to re-enable the local engine may be
/// superseded. Initialization still has to announce Ready, but must no longer
/// prepare a model after cloud-only mode was requested. Disabling and an
/// explicitly confirmed removal are durable control decisions and must not be
/// erased by a later cancellation.
fn command_after_queued_cancellation(
    mut command: InstallCommand,
    cancels_at_send: u64,
    current_cancels: u64,
) -> Option<InstallCommand> {
    if cancels_at_send == current_cancels {
        return Some(command);
    }
    match &mut command {
        InstallCommand::Initialize {
            skip_local_prepare, ..
        } => {
            *skip_local_prepare = true;
            Some(command)
        }
        InstallCommand::EnsureModel { .. }
        | InstallCommand::SelectTier { .. }
        | InstallCommand::SetLocalEnabled { enabled: true } => None,
        InstallCommand::RemoveModel { .. }
        | InstallCommand::SetLocalEnabled { enabled: false }
        | InstallCommand::Shutdown => Some(command),
    }
}

struct InstallActor {
    commands: Sender<QueuedInstallCommand>,
    cancellation: CancelToken,
    cancels: Arc<AtomicU64>,
}

impl InstallActor {
    fn start(
        state: Arc<Mutex<WorkerState>>,
        writer: EventWriter,
        cancellation: CancelToken,
    ) -> Result<(Self, JoinHandle<()>)> {
        let (commands, requests) = mpsc::channel::<QueuedInstallCommand>();
        let worker_cancellation = cancellation.clone();
        let cancels = Arc::new(AtomicU64::new(0));
        let worker_cancels = Arc::clone(&cancels);
        let thread = thread::Builder::new()
            .name("translation-model-installer".to_owned())
            .spawn(move || {
                for request in requests {
                    let Some(command) = command_after_queued_cancellation(
                        request.command,
                        request.cancels_at_send,
                        worker_cancels.load(Ordering::Acquire),
                    ) else {
                        continue;
                    };
                    if matches!(command, InstallCommand::Shutdown) {
                        break;
                    }
                    // A cancellation raised while nothing was installing must
                    // not cancel this operation.
                    worker_cancellation.reset();
                    // Read before the command is consumed: a failure has to say
                    // which row it belongs to, and after this the command is
                    // gone.
                    let installing = installing_tier(&command);
                    let failure_code = install_failure_code(&command);
                    if let Err(error) = run_install(command, &state, &writer) {
                        let cancelled = error
                            .downcast_ref::<ModelInstallError>()
                            .is_some_and(|error| matches!(error, ModelInstallError::Cancelled));
                        let _ = writer.send(&TranslationWorkerEvent::Failed {
                            request_id: None,
                            // The row that was installing gets its own failure,
                            // so it can stop offering a Cancel for a download
                            // that is already over.
                            catalog_id: installing.and_then(|tier| {
                                recover_lock(&state)
                                    .pack(tier)
                                    .ok()
                                    .map(|pack| pack.id.clone())
                            }),
                            code: if cancelled {
                                "model_install_cancelled".to_owned()
                            } else {
                                failure_code.to_owned()
                            },
                            message: format!("{error:#}"),
                            retryable: true,
                        });
                    }
                }
            })?;
        Ok((
            Self {
                commands,
                cancellation,
                cancels,
            },
            thread,
        ))
    }

    fn send(&self, command: InstallCommand) {
        let _ = self.commands.send(QueuedInstallCommand {
            command,
            cancels_at_send: self.cancels.load(Ordering::Acquire),
        });
    }

    fn cancel(&self) {
        self.cancels.fetch_add(1, Ordering::AcqRel);
        self.cancellation.cancel();
    }
}

fn install_failure_code(command: &InstallCommand) -> &'static str {
    match command {
        InstallCommand::RemoveModel { .. } => "model_remove",
        _ => "translation_worker",
    }
}

struct WorkerState {
    catalog: TranslationCatalog,
    installer: BundleInstaller,
    runtime: LlamaRuntimeManager,
    fetcher: HttpFetcher,
    selected_tier: Option<TranslationTier>,
    keep_warm: bool,
    local_model_enabled: bool,
    engine: Option<TranslationEngine>,
    locale: String,
    history: TranslationHistory,
    history_enabled: bool,
}

impl WorkerState {
    fn new() -> Result<Self> {
        let root = data_root();
        Ok(Self {
            catalog: TranslationCatalog::load()?,
            installer: BundleInstaller::new(root.join("translation-models")),
            runtime: LlamaRuntimeManager::new(root.join("runtimes")),
            fetcher: HttpFetcher::new("translation-model-manager")?,
            selected_tier: None,
            keep_warm: true,
            local_model_enabled: true,
            engine: None,
            locale: system_language(),
            history: TranslationHistory::load(root.join("translation-history.json")),
            history_enabled: true,
        })
    }

    fn pack(&self, tier: TranslationTier) -> Result<&TranslationPackDescriptor> {
        self.catalog
            .by_tier(tier)
            .ok_or_else(|| anyhow!("translation tier is missing from the catalog"))
    }

    fn model_installed(&self, tier: TranslationTier) -> bool {
        self.pack(tier).ok().is_some_and(|pack| {
            self.installer
                .is_installed(&pack.as_bundle())
                .unwrap_or(false)
        })
    }

    fn initial_tier(&self, requested: Option<TranslationTier>) -> Option<TranslationTier> {
        choose_initial_tier(
            requested,
            self.model_installed(TranslationTier::Tiny),
            self.model_installed(TranslationTier::Teacher),
        )
    }

    fn summaries(&self) -> Vec<TranslationModelSummary> {
        self.catalog
            .packs
            .iter()
            .map(|pack| {
                pack.summary(
                    self.installer
                        .is_installed(&pack.as_bundle())
                        .unwrap_or(false),
                    self.selected_tier == Some(pack.tier),
                    &self.locale,
                )
            })
            .collect()
    }

    fn ensure_runtime(&self, writer: &EventWriter, catalog_id: &str) -> Result<()> {
        self.runtime.ensure_installed(&self.fetcher, |progress| {
            let _ = writer.send(&TranslationWorkerEvent::ModelDownloadProgress {
                catalog_id: catalog_id.to_owned(),
                downloaded_bytes: progress.bytes_received,
                total_bytes: RUNTIME_DOWNLOAD_BYTES,
            });
        })?;
        Ok(())
    }

    fn load_engine(&mut self, tier: TranslationTier, warm: bool) -> Result<(String, u64)> {
        let started = Instant::now();
        let pack = self.pack(tier)?.clone();
        let root = self.installer.bundle_path(&pack.as_bundle());
        if !self.runtime.is_installed() {
            return Err(anyhow!("llama.cpp translation runtime is not installed"));
        }
        let mut engine = TranslationEngine::new(pack.clone(), root, self.runtime.runtime_path());
        if warm {
            engine.warm()?;
        }
        self.engine = Some(engine);
        Ok((
            pack.id,
            started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        ))
    }

    fn ensure_engine(&mut self, writer: &EventWriter) -> Result<TranslationTier> {
        if !self.local_model_enabled {
            return Err(anyhow!("the local translation engine is disabled"));
        }
        let tier = self
            .selected_tier
            .ok_or_else(|| anyhow!("no translation model is selected"))?;
        if self
            .engine
            .as_ref()
            .is_none_or(|engine| engine.tier() != tier)
        {
            let (catalog_id, load_duration_ms) = self.load_engine(tier, false)?;
            writer.send(&TranslationWorkerEvent::ModelReady {
                catalog_id,
                tier,
                load_duration_ms,
            })?;
        }
        Ok(tier)
    }
}

fn main() -> Result<()> {
    rust_platform_windows::configure_safe_dll_search()?;
    // Asked by the release check, which otherwise has no way to find out what a
    // packaged worker speaks short of driving a whole handshake.
    if std::env::args().any(|argument| argument == "--build-id") {
        println!("{}", rust_protocol::BUILD_COMMIT);
        return Ok(());
    }
    if std::env::args().any(|argument| argument == "--protocol-version") {
        println!("{PROTOCOL_VERSION}");
        return Ok(());
    }
    let state = Arc::new(Mutex::new(WorkerState::new()?));
    // Its own lock, not the worker's. A translation out on the network holds
    // this one for as long as the walk takes; nothing else in the worker should
    // have to wait for that.
    // Taken before any installation can hold the lock: cancelling must not
    // have to wait for the operation it is cancelling.
    let cancellation = recover_lock(&state).fetcher.cancellation();
    let stdin = io::stdin();
    let (writer, output_thread) = EventWriter::start()?;
    let (installs, install_thread) =
        InstallActor::start(Arc::clone(&state), writer.clone(), cancellation)?;
    let coordinator = ProviderActorHandle::start(Providers::new(
        data_root().join("translation-credentials.dat"),
    ))?;
    for line in BufReader::new(stdin.lock()).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let command = match serde_json::from_str::<TranslationWorkerCommand>(&line) {
            Ok(command) => command,
            Err(error) => {
                writer.send(&TranslationWorkerEvent::Failed {
                    request_id: None,
                    catalog_id: None,
                    code: "invalid_command".to_owned(),
                    message: error.to_string(),
                    retryable: false,
                })?;
                continue;
            }
        };
        if matches!(command, TranslationWorkerCommand::Shutdown) {
            break;
        }
        if let Err(error) = handle_command(
            command,
            Arc::clone(&state),
            &writer,
            &coordinator,
            &installs,
        ) {
            writer.send(&TranslationWorkerEvent::Failed {
                request_id: None,
                catalog_id: None,
                code: "translation_worker".to_owned(),
                message: format!("{error:#}"),
                retryable: true,
            })?;
        }
    }
    let provider_stopped = coordinator.shutdown();
    installs.cancel();
    installs.send(InstallCommand::Shutdown);
    drop(installs);
    let _ = install_thread.join();
    drop(writer);
    if provider_stopped {
        let _ = output_thread.join();
    } else {
        // A blocked provider still owns an EventWriter. Joining its output
        // thread would reintroduce the same unbounded shutdown wait.
        drop(output_thread);
    }
    Ok(())
}

fn handle_command(
    command: TranslationWorkerCommand,
    state: Arc<Mutex<WorkerState>>,
    writer: &EventWriter,
    coordinator: &ProviderActorHandle,
    installs: &InstallActor,
) -> Result<()> {
    match command {
        TranslationWorkerCommand::Initialize {
            selected_tier,
            keep_warm,
            history_enabled,
            prepare_local_model,
        } => {
            coordinator.cancel_all();
            installs.send(InstallCommand::Initialize {
                selected_tier,
                keep_warm,
                history_enabled,
                prepare_local_model,
                skip_local_prepare: false,
            });
        }
        TranslationWorkerCommand::EnsureModel { tier } => {
            coordinator.cancel_all();
            installs.send(InstallCommand::EnsureModel { tier });
        }
        TranslationWorkerCommand::RemoveModel { tier } => {
            coordinator.cancel_all();
            installs.cancel();
            installs.send(InstallCommand::RemoveModel { tier });
        }
        TranslationWorkerCommand::CancelModelInstall => installs.cancel(),
        TranslationWorkerCommand::SelectTier { tier, keep_warm } => {
            coordinator.cancel_all();
            // Choosing another tier abandons an installation still running for
            // the previous one.
            installs.cancel();
            installs.send(InstallCommand::SelectTier { tier, keep_warm });
        }
        TranslationWorkerCommand::Translate {
            request_id,
            text,
            source_language,
            target_language,
            language_policy,
        } => {
            coordinator.submit(
                state,
                writer.clone(),
                Asked {
                    request_id,
                    text,
                    source_language,
                    target_language,
                    language_policy,
                },
            )?;
        }
        TranslationWorkerCommand::Cancel { request_id } => {
            coordinator.cancel(request_id);
        }
        TranslationWorkerCommand::SetTranslationProviders { providers: entries } => {
            let local_enabled = local_translation_enabled(&entries);
            writer.send(&TranslationWorkerEvent::TranslationProviders {
                statuses: coordinator.set_entries(&entries),
            })?;
            if !local_enabled {
                installs.cancel();
            }
            installs.send(InstallCommand::SetLocalEnabled {
                enabled: local_enabled,
            });
        }
        TranslationWorkerCommand::SetTranslationProviderCredential {
            operation_id,
            provider_id,
            secret,
        } => {
            let (outcome, statuses) = coordinator.set_credential(&provider_id, secret.as_deref());
            // Answered either way, because a window is waiting on it and a
            // silence is indistinguishable from a success.
            writer.send(&TranslationWorkerEvent::CredentialSaved {
                operation_id,
                provider_id: provider_id.clone(),
                error: outcome.err().unwrap_or_default(),
            })?;
            writer.send(&TranslationWorkerEvent::TranslationProviders { statuses })?;
        }
        TranslationWorkerCommand::GetTranslationProviders => {
            let (trouble, statuses) = coordinator.statuses();
            // A key file this account cannot decrypt is a thing the user has to
            // be told about: it means the profile moved, every key is gone, and
            // the rows will simply say they need one.
            if let Some(trouble) = trouble {
                writer.send(&TranslationWorkerEvent::Failed {
                    request_id: None,
                    catalog_id: None,
                    code: "credential_store".to_owned(),
                    message: trouble,
                    retryable: false,
                })?;
            }
            writer.send(&TranslationWorkerEvent::TranslationProviders { statuses })?;
        }
        TranslationWorkerCommand::GetHistory { limit } => {
            let state = recover_lock(&state);
            writer.send(&TranslationWorkerEvent::History {
                entries: state.history.recent(limit),
            })?;
        }
        TranslationWorkerCommand::SetHistoryEnabled { enabled } => {
            let mut state = recover_lock(&state);
            state.history_enabled = enabled;
        }
        TranslationWorkerCommand::ClearHistory => {
            let mut state = recover_lock(&state);
            state.history.clear();
            writer.send(&TranslationWorkerEvent::History {
                entries: Vec::new(),
            })?;
        }
        TranslationWorkerCommand::Shutdown => {}
    }
    Ok(())
}

fn local_translation_enabled(entries: &[TranslationProviderEntry]) -> bool {
    entries.is_empty()
        || entries
            .iter()
            .any(|entry| entry.id == "local" && entry.enabled)
}

/// Runs one installation command. Called only from the installer thread, which
/// is why it may hold the worker lock for minutes.
/// Which model an install command is about, when it is about one.
///
/// A failure has to name the row it belongs to. Without it the desktop guessed
/// from whatever happened to be downloading, and a download that died of a
/// network fault left its row offering a Cancel for something already over.
const fn installing_tier(command: &InstallCommand) -> Option<TranslationTier> {
    match command {
        InstallCommand::Initialize { selected_tier, .. } => *selected_tier,
        InstallCommand::EnsureModel { tier }
        | InstallCommand::RemoveModel { tier }
        | InstallCommand::SelectTier { tier, .. } => Some(*tier),
        InstallCommand::SetLocalEnabled { .. } | InstallCommand::Shutdown => None,
    }
}

fn run_install(
    command: InstallCommand,
    state: &Arc<Mutex<WorkerState>>,
    writer: &EventWriter,
) -> Result<()> {
    match command {
        InstallCommand::Initialize {
            selected_tier,
            keep_warm,
            history_enabled,
            prepare_local_model,
            skip_local_prepare,
        } => {
            let mut state = recover_lock(state);
            state.keep_warm = keep_warm;
            state.local_model_enabled = prepare_local_model;
            state.history_enabled = history_enabled;
            state.selected_tier = state.initial_tier(selected_tier);
            writer.send(&TranslationWorkerEvent::Ready {
                protocol_version: PROTOCOL_VERSION,
                selected_tier: state.selected_tier,
                models: state.summaries(),
            })?;
            if state.local_model_enabled
                && !skip_local_prepare
                && let Some(tier) = state.selected_tier
            {
                let catalog_id = state.pack(tier)?.id.clone();
                state.ensure_runtime(writer, &catalog_id)?;
                let (catalog_id, load_duration_ms) = state.load_engine(tier, keep_warm)?;
                writer.send(&TranslationWorkerEvent::ModelReady {
                    catalog_id: catalog_id.clone(),
                    tier,
                    load_duration_ms,
                })?;
                writer.send(&TranslationWorkerEvent::LocalModelState {
                    catalog_id: Some(catalog_id),
                    loaded: true,
                })?;
            } else {
                writer.send(&TranslationWorkerEvent::LocalModelState {
                    catalog_id: None,
                    loaded: false,
                })?;
            }
        }
        InstallCommand::EnsureModel { tier } => {
            let mut state = recover_lock(state);
            let pack = state.pack(tier)?.clone();
            let bundle = pack.as_bundle();
            let runtime_download = if state.runtime.is_installed() {
                0
            } else {
                RUNTIME_DOWNLOAD_BYTES
            };
            let model_download = bundle.total_bytes();
            let combined_total = runtime_download.saturating_add(model_download);
            state.runtime.ensure_installed(&state.fetcher, |progress| {
                let _ = writer.send(&TranslationWorkerEvent::ModelDownloadProgress {
                    catalog_id: pack.id.clone(),
                    downloaded_bytes: progress.bytes_received.min(runtime_download),
                    total_bytes: combined_total,
                });
            })?;
            let path = state
                .installer
                .ensure_installed(&bundle, &state.fetcher, |progress| {
                    let _ = writer.send(&TranslationWorkerEvent::ModelDownloadProgress {
                        catalog_id: pack.id.clone(),
                        downloaded_bytes: runtime_download.saturating_add(progress.bytes_received),
                        total_bytes: combined_total,
                    });
                })?;
            writer.send(&TranslationWorkerEvent::ModelInstalled {
                catalog_id: pack.id.clone(),
                tier,
                path: path.to_string_lossy().into_owned(),
            })?;
            state.selected_tier = Some(tier);
            writer.send(&TranslationWorkerEvent::ModelSelected { tier })?;
            if state.local_model_enabled {
                let keep_warm = state.keep_warm;
                let (catalog_id, load_duration_ms) = state.load_engine(tier, keep_warm)?;
                writer.send(&TranslationWorkerEvent::ModelReady {
                    catalog_id: catalog_id.clone(),
                    tier,
                    load_duration_ms,
                })?;
                writer.send(&TranslationWorkerEvent::LocalModelState {
                    catalog_id: Some(catalog_id),
                    loaded: true,
                })?;
            }
        }
        InstallCommand::RemoveModel { tier } => {
            let mut state = recover_lock(state);
            let pack = state.pack(tier)?.clone();
            if state.selected_tier == Some(tier) && state.local_model_enabled {
                bail!(
                    "switch off the local translation provider before removing its selected model"
                );
            }
            if state.selected_tier == Some(tier) {
                let had_engine = state.engine.take().is_some();
                if had_engine {
                    writer.send(&TranslationWorkerEvent::LocalModelState {
                        catalog_id: Some(pack.id.clone()),
                        loaded: false,
                    })?;
                }
            }
            let mut removed = state.installer.remove_complete(&pack.as_bundle())?;
            let last_pack_removed = !state
                .catalog
                .packs
                .iter()
                .any(|candidate| state.model_installed(candidate.tier));
            if last_pack_removed {
                let runtime = state.runtime.remove()?;
                removed.removed_files = removed.removed_files.saturating_add(runtime.removed_files);
                removed.removed_bytes = removed.removed_bytes.saturating_add(runtime.removed_bytes);
            }
            writer.send(&TranslationWorkerEvent::ModelRemoved {
                catalog_id: pack.id,
                tier,
                removed_bytes: removed.removed_bytes,
                runtime_removed: last_pack_removed,
                models: state.summaries(),
            })?;
        }
        InstallCommand::SelectTier { tier, keep_warm } => {
            let mut state = recover_lock(state);
            if !state.model_installed(tier) {
                return Err(anyhow!("translation model is not installed"));
            }
            state.keep_warm = keep_warm;
            state.selected_tier = Some(tier);
            writer.send(&TranslationWorkerEvent::ModelSelected { tier })?;
            if state.local_model_enabled {
                let catalog_id = state.pack(tier)?.id.clone();
                state.ensure_runtime(writer, &catalog_id)?;
                let (catalog_id, load_duration_ms) = state.load_engine(tier, keep_warm)?;
                writer.send(&TranslationWorkerEvent::ModelReady {
                    catalog_id: catalog_id.clone(),
                    tier,
                    load_duration_ms,
                })?;
                writer.send(&TranslationWorkerEvent::LocalModelState {
                    catalog_id: Some(catalog_id),
                    loaded: true,
                })?;
            }
        }
        InstallCommand::SetLocalEnabled { enabled } => {
            let mut state = recover_lock(state);
            if state.local_model_enabled == enabled {
                writer.send(&TranslationWorkerEvent::LocalModelState {
                    catalog_id: if state.engine.is_some() {
                        state
                            .selected_tier
                            .and_then(|tier| state.pack(tier).ok())
                            .map(|pack| pack.id.clone())
                    } else {
                        None
                    },
                    loaded: state.engine.is_some(),
                })?;
                return Ok(());
            }
            state.local_model_enabled = enabled;
            if !enabled {
                let had_engine = state.engine.take().is_some();
                let unloaded = if had_engine {
                    state
                        .selected_tier
                        .and_then(|tier| state.pack(tier).ok())
                        .map(|pack| pack.id.clone())
                } else {
                    None
                };
                writer.send(&TranslationWorkerEvent::LocalModelState {
                    catalog_id: unloaded,
                    loaded: false,
                })?;
            } else if let Some(tier) = state.selected_tier {
                let catalog_id = state.pack(tier)?.id.clone();
                state.ensure_runtime(writer, &catalog_id)?;
                let keep_warm = state.keep_warm;
                let (catalog_id, load_duration_ms) = state.load_engine(tier, keep_warm)?;
                writer.send(&TranslationWorkerEvent::ModelReady {
                    catalog_id: catalog_id.clone(),
                    tier,
                    load_duration_ms,
                })?;
                writer.send(&TranslationWorkerEvent::LocalModelState {
                    catalog_id: Some(catalog_id),
                    loaded: true,
                })?;
            }
        }
        InstallCommand::Shutdown => {}
    }
    Ok(())
}

fn choose_initial_tier(
    requested: Option<TranslationTier>,
    fast_installed: bool,
    quality_installed: bool,
) -> Option<TranslationTier> {
    if requested == Some(TranslationTier::Teacher) && quality_installed {
        return Some(TranslationTier::Teacher);
    }
    if requested == Some(TranslationTier::Tiny) && fast_installed {
        return Some(TranslationTier::Tiny);
    }
    if quality_installed {
        Some(TranslationTier::Teacher)
    } else if fast_installed {
        Some(TranslationTier::Tiny)
    } else {
        None
    }
}

/// One translation the worker has been asked for.
///
/// The four of them travel together through the coordinator and into the
/// request, which is clearer than four more parameters on two signatures that
/// already carry a state, a list and a writer.
struct Asked {
    request_id: RequestId,
    text: String,
    /// `None` for either language means "read it from the text", which is what
    /// the desktop sends for every selection.
    source_language: Option<TranslationLanguage>,
    target_language: Option<TranslationLanguage>,
    language_policy: TranslationLanguagePolicy,
}

/// The direction one request will actually be translated in.
///
/// The caller may name neither language, one, or both. Whatever it names, both
/// paths must agree, and both must be told: the local engine would otherwise
/// detect the language for itself while a service was told to translate English
/// into English.
fn resolve_direction(
    text: &str,
    source_language: Option<TranslationLanguage>,
    target_language: Option<TranslationLanguage>,
    language_policy: TranslationLanguagePolicy,
) -> Result<(TranslationLanguage, TranslationLanguage)> {
    if !language_policy.is_valid() {
        anyhow::bail!("primary and secondary translation languages are the same");
    }
    let source = source_language.unwrap_or_else(|| detect_language(text));
    let target = target_language.unwrap_or_else(|| language_policy.target_for(source));
    if source == target {
        anyhow::bail!("source and target languages are the same");
    }
    Ok((source, target))
}

fn translate_request(
    state: &Arc<Mutex<WorkerState>>,
    providers: Option<&mut providers::ProvidersSnapshot>,
    writer: &EventWriter,
    asked: Asked,
    is_current: impl Fn() -> bool,
) -> Result<()> {
    let Asked {
        request_id,
        text,
        source_language,
        target_language,
        language_policy,
    } = asked;
    let started = Instant::now();
    // One direction, worked out once, used by whichever path takes the request.
    //
    // The desktop sends no languages at all: it does not know what the user
    // selected, and the local engine has always read the text and translated
    // into the other language. A service cannot do that for us — it is told
    // what to translate into — and `None` was quietly becoming
    // `TranslationLanguage::default()`, which is English. So an English
    // selection was sent to DeepL to be translated from English into English,
    // and the result claimed to be exactly that.
    // Nothing to translate is refused here rather than sent to a service, which
    // would spend an allowance to be told the same thing.
    if text.trim().is_empty() {
        anyhow::bail!("there is no text to translate");
    }
    let (source, target) =
        resolve_direction(&text, source_language, target_language, language_policy)?;

    // The list decides first, and the local engine is one row in it. When it is
    // the only row switched on — the default — this costs one comparison and
    // the request goes straight down the path it always did, without so much as
    // building a client.
    //
    // The control actor already captured this immutable provider snapshot. It
    // remains responsive to credentials and status requests while this thread
    // performs network I/O, and later adopts the request ledger only if its
    // configuration revision is still current.
    if !is_current() {
        return Err(TranslationCancelled.into());
    }
    let routed = providers
        .map(|snapshot| snapshot.translate(&text, Some(source.code()), target.code(), &is_current));
    if let Some(routed) = routed {
        match routed {
            Routed::Translated {
                provider,
                text: translated,
            } => {
                // The state is taken only to record the result, not for the
                // walk that produced it.
                return finish(
                    &mut recover_lock(state),
                    writer,
                    TranslationResult {
                        request_id,
                        source_text: text,
                        translated_text: translated,
                        source_language: source,
                        target_language: target,
                        tier: None,
                        provider: Some(provider),
                        processing_duration_ms: elapsed_ms(started),
                        first_token_duration_ms: 0,
                        backend_request_count: 1,
                        tokenize_duration_ms: 0,
                        chunk_count: 1,
                    },
                );
            }
            Routed::Cancelled => return Err(TranslationCancelled.into()),
            Routed::Exhausted { detail } => {
                // Nobody in the list could answer, and the local engine was not
                // in it or was switched off. Saying which service said what is
                // the only way the user can fix it.
                return Err(anyhow::anyhow!(detail));
            }
            // The walk reached the local engine. Below is what it does.
            Routed::UseLocalEngine => {}
        }
    }

    // The engine, and only now the worker's state.
    let state = &mut *recover_lock(state);
    let tier = state.ensure_engine(writer)?;
    let mut last_partial_at = None;
    let mut last_partial_text = String::new();
    let output = state
        .engine
        .as_mut()
        .context("translation engine failed to initialize")?
        .translate_streaming(
            &text,
            Some(source),
            Some(target),
            &is_current,
            |partial, progress| {
                if !is_current() {
                    return false;
                }
                let now = Instant::now();
                let should_emit = last_partial_at.is_none()
                    || last_partial_at.is_some_and(|last: Instant| {
                        now.duration_since(last) >= Duration::from_millis(30)
                    })
                    || partial.ends_with(['\n', '.', '!', '?']);
                if should_emit && partial != last_partial_text {
                    if writer
                        .send(&TranslationWorkerEvent::Partial {
                            request_id,
                            translated_text: partial.to_owned(),
                            first_token_duration_ms: progress.first_token_duration_ms,
                            tokenize_duration_ms: progress.tokenize_duration_ms,
                            chunk_index: progress.chunk_index,
                            chunk_count: progress.chunk_count,
                        })
                        .is_err()
                    {
                        return false;
                    }
                    last_partial_at = Some(now);
                    partial.clone_into(&mut last_partial_text);
                }
                true
            },
        )?;
    if !is_current() {
        return Err(TranslationCancelled.into());
    }
    let result = TranslationResult {
        request_id,
        source_text: text,
        translated_text: output.text,
        source_language: output.source_language,
        target_language: output.target_language,
        tier: Some(tier),
        provider: None,
        processing_duration_ms: elapsed_ms(started),
        first_token_duration_ms: output.first_token_duration_ms,
        backend_request_count: output.backend_request_count,
        tokenize_duration_ms: output.tokenize_duration_ms,
        chunk_count: output.chunk_count,
    };
    finish(state, writer, result)
}

/// Records a finished translation and tells the desktop about it.
///
/// Shared by both paths so that a translation from a service is kept, reported
/// and counted exactly like one from the machine — the only difference being
/// what the result says about where it came from.
fn finish(state: &mut WorkerState, writer: &EventWriter, result: TranslationResult) -> Result<()> {
    let created_unix_ms = unix_ms();
    if state.history_enabled {
        state.history.push(TranslationHistoryEntry {
            result: result.clone(),
            created_unix_ms,
        });
    }
    writer.send(&TranslationWorkerEvent::Completed {
        result,
        created_unix_ms,
    })?;
    if !state.keep_warm {
        state.engine = None;
    }
    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn data_root() -> PathBuf {
    rust_app_core::product::data_root()
}

fn system_language() -> String {
    sys_locale::get_locale()
        .unwrap_or_else(|| "en".to_owned())
        .split(['-', '_'])
        .next()
        .unwrap_or("en")
        .to_ascii_lowercase()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn recover_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod selection_tests {
    use rust_protocol::TranslationTier;

    use super::choose_initial_tier;

    #[test]
    fn preserves_an_installed_explicit_selection() {
        assert_eq!(
            choose_initial_tier(Some(TranslationTier::Tiny), true, true),
            Some(TranslationTier::Tiny)
        );
        assert_eq!(
            choose_initial_tier(Some(TranslationTier::Teacher), true, true),
            Some(TranslationTier::Teacher)
        );
    }

    #[test]
    fn recovers_to_best_installed_model_when_selection_is_missing_or_stale() {
        assert_eq!(
            choose_initial_tier(None, true, true),
            Some(TranslationTier::Teacher)
        );
        assert_eq!(
            choose_initial_tier(Some(TranslationTier::Tiny), false, true),
            Some(TranslationTier::Teacher)
        );
        assert_eq!(
            choose_initial_tier(Some(TranslationTier::Teacher), true, false),
            Some(TranslationTier::Tiny)
        );
    }

    #[test]
    fn leaves_translation_unselected_when_no_model_is_installed() {
        assert_eq!(choose_initial_tier(None, false, false), None);
    }
}

#[cfg(test)]
mod direction_tests {
    use rust_protocol::{TranslationLanguage, TranslationLanguagePolicy};

    use super::resolve_direction;

    #[test]
    fn a_request_that_names_nothing_is_read_from_the_text() {
        // The failure this replaces: the desktop names neither language for
        // every selection the user translates, and a service was therefore
        // told to translate English into English.
        assert_eq!(
            resolve_direction(
                "The quick brown fox",
                None,
                None,
                TranslationLanguagePolicy::default(),
            )
            .unwrap(),
            (TranslationLanguage::English, TranslationLanguage::Russian)
        );
        assert_eq!(
            resolve_direction(
                "Проверка связи",
                None,
                None,
                TranslationLanguagePolicy::default(),
            )
            .unwrap(),
            (TranslationLanguage::Russian, TranslationLanguage::English)
        );
    }

    #[test]
    fn a_named_source_uses_the_language_policy() {
        assert_eq!(
            resolve_direction(
                "anything",
                Some(TranslationLanguage::Russian),
                None,
                TranslationLanguagePolicy::default(),
            )
            .unwrap(),
            (TranslationLanguage::Russian, TranslationLanguage::English)
        );
    }

    #[test]
    fn ukrainian_is_in_the_current_policy_not_a_future_fallback() {
        assert_eq!(
            resolve_direction(
                "Українська мова має власну історію",
                None,
                None,
                TranslationLanguagePolicy::default(),
            )
            .unwrap(),
            (TranslationLanguage::Ukrainian, TranslationLanguage::English)
        );
    }

    #[test]
    fn the_request_that_would_leave_the_machine_carries_the_right_direction() {
        // Not a unit test of the arithmetic: this builds the request a service
        // would actually receive, from exactly what the desktop sends for every
        // selection — neither language named. It used to arrive asking for
        // English into English.
        use rust_cloud_translation::{ProviderEntry, ProviderId, plan_request};

        let deepl = ProviderEntry::new(ProviderId::new(ProviderId::DEEPL));
        // DeepL's own names: a plain language to read, a regional variant to
        // write into.
        for (text, source, target) in [
            ("The quick brown fox", "EN", "RU"),
            ("Проверка связи, это тестовая диктовка", "RU", "EN-GB"),
        ] {
            let (from, to) =
                resolve_direction(text, None, None, TranslationLanguagePolicy::default()).unwrap();
            let plan = plan_request(&deepl, "key", text, Some(from.code()), to.code())
                .expect("the plan is buildable");

            assert!(
                plan.body.contains(&format!("\"target_lang\":\"{target}\"")),
                "asked {text:?} to become {target}: {}",
                plan.body
            );
            assert!(
                plan.body.contains(&format!("\"source_lang\":\"{source}\"")),
                "told the service the text is {source}: {}",
                plan.body
            );
        }
    }

    #[test]
    fn nothing_named_reaches_the_service_as_the_right_direction() {
        // The whole path the defect lived on, in one test: the desktop names
        // neither language, the worker resolves the pair, the router walks the
        // list, and this is the body the service receives. The two halves were
        // covered separately and the join between them was not.
        use rust_cloud_translation::{
            Chain, ProviderEntry, ProviderFailure, ProviderId, RequestExecutor, RequestPlan,
            RouteOutcome, Router, Translation,
        };
        use std::cell::RefCell;

        struct Capture(RefCell<Vec<String>>);
        impl RequestExecutor for Capture {
            fn execute(
                &self,
                plan: &RequestPlan,
                _budget: std::time::Duration,
            ) -> Result<String, ProviderFailure> {
                self.0.borrow_mut().push(plan.body.clone());
                Ok("answered".to_owned())
            }
        }

        for (text, expect_source, expect_target) in [
            (
                "The quick brown fox",
                "\"source_lang\":\"EN\"",
                "\"target_lang\":\"RU\"",
            ),
            (
                "Проверка связи",
                "\"source_lang\":\"RU\"",
                "\"target_lang\":\"EN-GB\"",
            ),
        ] {
            // Exactly what `TranslationWorkerCommand::Translate` carries for a
            // selection: no languages at all.
            let (source, target) =
                resolve_direction(text, None, None, TranslationLanguagePolicy::default()).unwrap();

            let mut router = Router::new(Chain::new(vec![
                ProviderEntry::new(ProviderId::new(ProviderId::DEEPL)),
                ProviderEntry::new(ProviderId::local()),
            ]));
            router
                .chain_mut()
                .set_enabled(&ProviderId::new(ProviderId::DEEPL), true);
            let executor = Capture(RefCell::new(Vec::new()));

            let outcome = router.translate(
                Translation {
                    text,
                    source: Some(source.code()),
                    target: target.code(),
                },
                std::time::Instant::now(),
                &|_: &ProviderId| Some("key".to_owned()),
                &executor,
                &|| false,
            );

            assert!(
                matches!(outcome, RouteOutcome::Translated { .. }),
                "{text}: {outcome:?}"
            );
            let bodies = executor.0.borrow();
            let body = bodies.first().expect("one request went out");
            assert!(body.contains(expect_source), "{text}: {body}");
            assert!(body.contains(expect_target), "{text}: {body}");
        }
    }

    #[test]
    fn a_direction_that_goes_nowhere_is_refused_rather_than_sent() {
        assert!(
            resolve_direction(
                "anything",
                Some(TranslationLanguage::English),
                Some(TranslationLanguage::English),
                TranslationLanguagePolicy::default(),
            )
            .is_err(),
            "translating a language into itself spends an allowance for nothing"
        );
    }
}

#[cfg(test)]
mod local_model_policy_tests {
    use rust_protocol::{TranslationProviderEntry, TranslationTier};

    use super::{
        InstallCommand, command_after_queued_cancellation, install_failure_code,
        local_translation_enabled,
    };

    fn entry(id: &str, enabled: bool) -> TranslationProviderEntry {
        TranslationProviderEntry {
            id: id.to_owned(),
            enabled,
            region: None,
        }
    }

    #[test]
    fn cloud_only_does_not_prepare_the_local_translation_model() {
        assert!(
            local_translation_enabled(&[]),
            "an old empty setting is the local default"
        );
        assert!(local_translation_enabled(&[
            entry("deepl", true),
            entry("local", true),
        ]));
        assert!(!local_translation_enabled(&[
            entry("deepl", true),
            entry("local", false),
        ]));
    }

    #[test]
    fn a_remove_failure_can_release_the_desktop_removing_state() {
        assert_eq!(
            install_failure_code(&InstallCommand::RemoveModel {
                tier: TranslationTier::Tiny,
            }),
            "model_remove"
        );
        assert_eq!(
            install_failure_code(&InstallCommand::EnsureModel {
                tier: TranslationTier::Tiny,
            }),
            "translation_worker"
        );
    }

    #[test]
    fn a_cancel_supersedes_expensive_work_that_was_still_queued() {
        for command in [
            InstallCommand::EnsureModel {
                tier: TranslationTier::Tiny,
            },
            InstallCommand::SelectTier {
                tier: TranslationTier::Teacher,
                keep_warm: true,
            },
            InstallCommand::SetLocalEnabled { enabled: true },
        ] {
            assert!(
                command_after_queued_cancellation(command, 4, 5).is_none(),
                "stale work must not download or reload a model after cloud-only was requested"
            );
        }
    }

    #[test]
    fn a_cancelled_initialization_still_becomes_ready_without_loading_local() {
        let command = command_after_queued_cancellation(
            InstallCommand::Initialize {
                selected_tier: Some(TranslationTier::Teacher),
                keep_warm: true,
                history_enabled: true,
                prepare_local_model: true,
                skip_local_prepare: false,
            },
            9,
            10,
        )
        .expect("initialization still owns the Ready handshake");

        assert!(matches!(
            command,
            InstallCommand::Initialize {
                prepare_local_model: true,
                skip_local_prepare: true,
                ..
            }
        ));
    }

    #[test]
    fn cancellation_never_erases_disable_or_confirmed_removal() {
        assert!(matches!(
            command_after_queued_cancellation(
                InstallCommand::SetLocalEnabled { enabled: false },
                1,
                2,
            ),
            Some(InstallCommand::SetLocalEnabled { enabled: false })
        ));
        assert!(matches!(
            command_after_queued_cancellation(
                InstallCommand::RemoveModel {
                    tier: TranslationTier::Teacher,
                },
                1,
                2,
            ),
            Some(InstallCommand::RemoveModel {
                tier: TranslationTier::Teacher,
            })
        ));
    }
}
