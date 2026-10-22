#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod block_translation;
mod glossary;
mod glossary_actor;
mod history;
mod history_actor;
mod provider_actor;
mod providers;
mod runtime;

use std::{
    collections::{HashMap, HashSet},
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

use anyhow::{Result, anyhow, bail};
use glossary_actor::GlossaryActor;
use history_actor::TranslationHistoryActor;
use runtime::{LlamaRuntimeManager, RUNTIME_DOWNLOAD_BYTES};
use rust_model_manager::{BundleInstaller, CancelToken, HttpFetcher, ModelInstallError};
use rust_protocol::{
    BlockCompletion, GlossaryOrigin, PROTOCOL_VERSION, RequestId, ScreenBlockId, ScreenTextBlock,
    SourceContext, TranslatedScreenBlock, TranslationAttemptReport, TranslationBackendCapabilities,
    TranslationFailureKind, TranslationFailureReport, TranslationHistoryEntry, TranslationLanguage,
    TranslationLanguagePolicy, TranslationModelSummary, TranslationProviderEntry,
    TranslationResult, TranslationTier, TranslationWorkerCommand, TranslationWorkerEvent,
};
use rust_translation_engine::{
    TRANSLATION_OPERATION_TIMEOUT, TranslationCancelled, TranslationCatalog, TranslationEngine,
    TranslationPackDescriptor, detect_language_with_policy, same_script,
};

use crate::providers::{Providers, Routed};
use crate::{
    block_translation::{BlockTranslationPlan, validate_completed_blocks, validate_partial_block},
    provider_actor::ProviderActorHandle,
};

#[derive(Debug)]
struct TranslationRequestFailure(TranslationFailureReport);

impl std::fmt::Display for TranslationRequestFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0.detail)
    }
}

impl std::error::Error for TranslationRequestFailure {}

fn request_failure(
    kind: TranslationFailureKind,
    detail: impl Into<String>,
    attempts: Vec<TranslationAttemptReport>,
) -> anyhow::Error {
    TranslationRequestFailure(TranslationFailureReport {
        kind,
        provider_id: None,
        retry_after_ms: None,
        required_catalog_id: None,
        detail: detail.into(),
        attempts,
    })
    .into()
}

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
        selected_catalog_id: Option<String>,
        selected_tier: Option<TranslationTier>,
        keep_warm: bool,
        prepare_local_model: bool,
        skip_local_prepare: bool,
    },
    EnsureModel {
        catalog_id: String,
    },
    RemoveModel {
        catalog_id: String,
    },
    SelectModel {
        catalog_id: String,
        keep_warm: bool,
    },
    SetLocalEnabled {
        enabled: bool,
    },
    /// Drops only the resident engine. Selection, provider order and installed
    /// files stay intact so the next local request can prepare it again.
    UnloadResident,
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
        | InstallCommand::SelectModel { .. }
        | InstallCommand::SetLocalEnabled { enabled: true } => None,
        InstallCommand::RemoveModel { .. }
        | InstallCommand::SetLocalEnabled { enabled: false }
        | InstallCommand::UnloadResident
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
                    let installing = installing_catalog_id(&command);
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
                            catalog_id: installing,
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
    selected_catalog_id: Option<String>,
    selected_tier: Option<TranslationTier>,
    keep_warm: bool,
    local_model_enabled: bool,
    engine: Option<TranslationEngine>,
    /// When the resident engine last finished a request.
    engine_last_used: Option<Instant>,
    locale: String,
    history: TranslationHistoryActor,
}

/// Keep-warm keeps the compact model (tens of MB in RAM) loaded for good. A
/// contextual model holds gigabytes of RAM or VRAM (5.7 GB measured for 9B),
/// so it is released after this long without a request.
const CONTEXTUAL_MODEL_IDLE_RELEASE: Duration = Duration::from_secs(10 * 60);
const IDLE_RELEASE_CHECK: Duration = Duration::from_secs(30);

impl WorkerState {
    /// Releases an idle contextual (llama.cpp) engine. The compact engine stays.
    fn release_idle_contextual_engine(&mut self, writer: &EventWriter, now: Instant) -> Result<()> {
        let idle = self.engine_last_used.is_some_and(|used| {
            now.saturating_duration_since(used) >= CONTEXTUAL_MODEL_IDLE_RELEASE
        });
        let contextual = self.engine.as_ref().is_some_and(|engine| {
            self.catalog
                .by_id(engine.catalog_id())
                .is_some_and(|pack| pack.backend.requires_llama_runtime())
        });
        if idle && contextual {
            self.engine_last_used = None;
            release_engine_unless_warm(false, &mut self.engine, writer)?;
        }
        Ok(())
    }
}

/// Checks twice a minute whether a contextual model has gone unused.
fn start_idle_release(state: Arc<Mutex<WorkerState>>, writer: EventWriter) -> Result<()> {
    std::thread::Builder::new()
        .name("translation-idle-release".into())
        .spawn(move || {
            loop {
                std::thread::sleep(IDLE_RELEASE_CHECK);
                if recover_lock(&state)
                    .release_idle_contextual_engine(&writer, Instant::now())
                    .is_err()
                {
                    // The desktop closed our output; the worker is ending.
                    return;
                }
            }
        })?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum LocalPackSelectionError {
    NoSelection,
    UnsupportedPair,
    Missing { catalog_id: String },
}

impl std::fmt::Display for LocalPackSelectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSelection => formatter.write_str("no translation model is selected"),
            Self::UnsupportedPair => formatter.write_str("no compact package supports this pair"),
            Self::Missing { catalog_id } => {
                write!(
                    formatter,
                    "translation package {catalog_id} is not installed"
                )
            }
        }
    }
}

impl std::error::Error for LocalPackSelectionError {}

fn select_local_catalog_id(
    catalog: &TranslationCatalog,
    selected_tier: Option<TranslationTier>,
    selected_catalog_id: Option<&str>,
    source: TranslationLanguage,
    target: TranslationLanguage,
    is_installed: impl Fn(&str) -> bool,
) -> std::result::Result<String, LocalPackSelectionError> {
    let pack = match selected_tier {
        Some(TranslationTier::Compact) => catalog
            .compact_for_pair(source, target)
            .ok_or(LocalPackSelectionError::UnsupportedPair)?,
        Some(tier @ (TranslationTier::Tiny | TranslationTier::Teacher)) => {
            if let Some(catalog_id) = selected_catalog_id {
                catalog
                    .by_id(catalog_id)
                    .filter(|pack| pack.tier == tier)
                    .or_else(|| catalog.by_tier(tier))
            } else {
                catalog.by_tier(tier)
            }
            .ok_or(LocalPackSelectionError::NoSelection)?
        }
        None => return Err(LocalPackSelectionError::NoSelection),
    };
    if !pack.supports(source, target) {
        return Err(LocalPackSelectionError::UnsupportedPair);
    }
    if !is_installed(&pack.id) {
        return Err(LocalPackSelectionError::Missing {
            catalog_id: pack.id.clone(),
        });
    }
    Ok(pack.id.clone())
}

fn local_direction_fallback(
    catalog: &TranslationCatalog,
    selected_tier: Option<TranslationTier>,
    selected_catalog_id: Option<&str>,
    detected: (TranslationLanguage, TranslationLanguage),
    target_named: Option<TranslationLanguage>,
    policy: TranslationLanguagePolicy,
    is_installed: impl Fn(&str) -> bool,
) -> (TranslationLanguage, TranslationLanguage) {
    let selectable = |source: TranslationLanguage, target: TranslationLanguage| {
        source != target
            && select_local_catalog_id(
                catalog,
                selected_tier,
                selected_catalog_id,
                source,
                target,
                &is_installed,
            )
            .is_ok()
    };
    let (source, target) = detected;
    if selectable(source, target) {
        return detected;
    }
    let installed = catalog
        .packs
        .iter()
        .filter(|pack| is_installed(&pack.id))
        .flat_map(|pack| pack.languages.iter().copied())
        .collect::<Vec<_>>();
    [policy.primary, policy.secondary]
        .into_iter()
        .chain(installed)
        .filter(|candidate| *candidate != source && same_script(*candidate, source))
        .map(|candidate| {
            (
                candidate,
                target_named.unwrap_or_else(|| policy.target_for(candidate)),
            )
        })
        .find(|(candidate, target)| selectable(*candidate, *target))
        .unwrap_or(detected)
}

fn replace_resident_resource<T, E>(
    slot: &mut Option<T>,
    announce_release: impl FnOnce() -> Result<(), E>,
    create: impl FnOnce() -> Result<T, E>,
) -> Result<(), E> {
    // Dropping the old process before invoking the factory prevents the peak
    // where two model allocations briefly coexist in RAM or VRAM.
    let released = slot.take().is_some();
    if released {
        announce_release()?;
    }
    *slot = Some(create()?);
    Ok(())
}

fn release_resident_resource<T>(slot: &mut Option<T>) -> Option<T> {
    slot.take()
}

impl WorkerState {
    fn new(history: TranslationHistoryActor) -> Result<Self> {
        let root = data_root();
        Ok(Self {
            catalog: TranslationCatalog::load()?,
            installer: BundleInstaller::new(root.join("translation-models")),
            runtime: LlamaRuntimeManager::new(root.join("runtimes")),
            fetcher: HttpFetcher::new("translation-model-manager")?,
            selected_catalog_id: None,
            selected_tier: None,
            keep_warm: true,
            local_model_enabled: true,
            engine: None,
            engine_last_used: None,
            locale: system_language(),
            history,
        })
    }

    fn pack(&self, catalog_id: &str) -> Result<&TranslationPackDescriptor> {
        self.catalog
            .by_id(catalog_id)
            .ok_or_else(|| anyhow!("translation model is missing from the catalog"))
    }

    fn model_installed(&self, catalog_id: &str) -> bool {
        self.pack(catalog_id).ok().is_some_and(|pack| {
            self.installer
                .is_installed(&pack.as_bundle())
                .unwrap_or(false)
        })
    }

    fn initial_selection(
        &self,
        requested_catalog_id: Option<&str>,
        requested_tier: Option<TranslationTier>,
    ) -> Option<(String, TranslationTier)> {
        if let Some(catalog_id) = requested_catalog_id
            && self.model_installed(catalog_id)
        {
            let pack = self.pack(catalog_id).ok()?;
            return Some((pack.id.clone(), pack.tier));
        }
        let installed_for_tier = |tier| {
            self.catalog
                .packs
                .iter()
                .find(|pack| pack.tier == tier && self.model_installed(&pack.id))
        };
        let tier = choose_initial_tier(
            requested_tier,
            installed_for_tier(TranslationTier::Compact).is_some(),
            installed_for_tier(TranslationTier::Tiny).is_some(),
            installed_for_tier(TranslationTier::Teacher).is_some(),
        )?;
        let pack = installed_for_tier(tier)?;
        Some((pack.id.clone(), pack.tier))
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
                    self.selected_catalog_id.as_deref() == Some(pack.id.as_str()),
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

    fn load_engine(
        &mut self,
        writer: &EventWriter,
        catalog_id: &str,
        warm: bool,
    ) -> Result<(String, u64)> {
        let started = Instant::now();
        let pack = self.pack(catalog_id)?.clone();
        let root = self.installer.bundle_path(&pack.as_bundle());
        if pack.backend.requires_llama_runtime() && !self.runtime.is_installed() {
            return Err(anyhow!("llama.cpp translation runtime is not installed"));
        }
        let runtime_path = self.runtime.runtime_path();
        let released_catalog_id = self
            .engine
            .as_ref()
            .map(|engine| engine.catalog_id().to_owned());
        replace_resident_resource(
            &mut self.engine,
            || {
                writer.send(&TranslationWorkerEvent::LocalModelState {
                    catalog_id: released_catalog_id,
                    loaded: false,
                })
            },
            || {
                let mut engine = TranslationEngine::new(pack.clone(), root, runtime_path);
                if warm {
                    engine.warm()?;
                }
                Ok::<_, anyhow::Error>(engine)
            },
        )?;
        Ok((
            pack.id,
            started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        ))
    }

    /// A short phrase can be read as a language no local pack speaks: an
    /// English phrase detected as German. When the request named no source
    /// language, use a language of an installed pack written in the same
    /// script instead of failing with "model missing".
    fn local_direction(
        &self,
        detected: (TranslationLanguage, TranslationLanguage),
        source_named: bool,
        target_named: Option<TranslationLanguage>,
        policy: TranslationLanguagePolicy,
    ) -> (TranslationLanguage, TranslationLanguage) {
        if source_named {
            return detected;
        }
        local_direction_fallback(
            &self.catalog,
            self.selected_tier,
            self.selected_catalog_id.as_deref(),
            detected,
            target_named,
            policy,
            |candidate| self.model_installed(candidate),
        )
    }

    fn ensure_engine(
        &mut self,
        writer: &EventWriter,
        source: TranslationLanguage,
        target: TranslationLanguage,
        deadline: Instant,
        is_current: &impl Fn() -> bool,
    ) -> Result<TranslationTier> {
        if !is_current() {
            return Err(TranslationCancelled.into());
        }
        if Instant::now() >= deadline {
            bail!("translation exceeded its operation deadline");
        }
        if !self.local_model_enabled {
            return Err(anyhow!("the local translation engine is disabled"));
        }
        let catalog_id = select_local_catalog_id(
            &self.catalog,
            self.selected_tier,
            self.selected_catalog_id.as_deref(),
            source,
            target,
            |candidate| self.model_installed(candidate),
        )?;
        let tier = self.pack(&catalog_id)?.tier;
        if self.selected_catalog_id.as_deref() != Some(catalog_id.as_str()) {
            self.selected_catalog_id = Some(catalog_id.clone());
            writer.send(&TranslationWorkerEvent::ModelSelected {
                catalog_id: catalog_id.clone(),
                tier,
            })?;
        }
        if self
            .engine
            .as_ref()
            .is_none_or(|engine| engine.catalog_id() != catalog_id)
        {
            let (catalog_id, load_duration_ms) = self.load_engine(writer, &catalog_id, false)?;
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
    let root = data_root();
    let (history, history_thread) =
        TranslationHistoryActor::start(root.join("translation-history.json"))?;
    let (glossary, glossary_thread) = GlossaryActor::start(root.join("translation-glossary.json"))?;
    let state = Arc::new(Mutex::new(WorkerState::new(history.clone())?));
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
    start_idle_release(Arc::clone(&state), writer.clone())?;
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
        if reject_invalid_bounded_command(&command, &writer)? {
            continue;
        }
        if matches!(command, TranslationWorkerCommand::Shutdown) {
            break;
        }
        if let Err(error) = handle_command(
            command,
            Arc::clone(&state),
            &writer,
            &coordinator,
            &installs,
            &history,
            &glossary,
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
    history.shutdown();
    let _ = history_thread.join();
    glossary.shutdown();
    let _ = glossary_thread.join();
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

fn reject_invalid_bounded_command(
    command: &TranslationWorkerCommand,
    writer: &EventWriter,
) -> Result<bool> {
    let Err(error) = command.validate_limits() else {
        return Ok(false);
    };
    match command {
        TranslationWorkerCommand::SaveGlossaryEntry { operation_id, .. }
        | TranslationWorkerCommand::ListGlossary { operation_id, .. }
        | TranslationWorkerCommand::UpdateGlossaryEntry { operation_id, .. }
        | TranslationWorkerCommand::DeleteGlossaryEntry { operation_id, .. } => {
            writer.send(&TranslationWorkerEvent::GlossaryFailed {
                operation_id: *operation_id,
                code: "invalid_command".to_owned(),
                message: error.to_string(),
                retryable: false,
            })?;
        }
        TranslationWorkerCommand::TranslateBlocks { request_id, .. } => {
            writer.send(&TranslationWorkerEvent::Failed {
                request_id: Some(*request_id),
                catalog_id: None,
                code: "invalid_command".to_owned(),
                message: error.to_string(),
                retryable: false,
            })?;
        }
        _ => unreachable!("only bounded extension commands are validated"),
    }
    Ok(true)
}

fn handle_command(
    command: TranslationWorkerCommand,
    state: Arc<Mutex<WorkerState>>,
    writer: &EventWriter,
    coordinator: &ProviderActorHandle,
    installs: &InstallActor,
    history: &TranslationHistoryActor,
    glossary: &GlossaryActor,
) -> Result<()> {
    match command {
        TranslationWorkerCommand::Initialize {
            selected_catalog_id,
            selected_tier,
            keep_warm,
            history_enabled,
            prepare_local_model,
        } => {
            coordinator.cancel_all();
            history.set_enabled(history_enabled);
            installs.send(InstallCommand::Initialize {
                selected_catalog_id,
                selected_tier,
                keep_warm,
                prepare_local_model,
                skip_local_prepare: false,
            });
        }
        TranslationWorkerCommand::EnsureModel { catalog_id } => {
            coordinator.cancel_all();
            installs.send(InstallCommand::EnsureModel { catalog_id });
        }
        TranslationWorkerCommand::RemoveModel { catalog_id } => {
            coordinator.cancel_all();
            installs.cancel();
            installs.send(InstallCommand::RemoveModel { catalog_id });
        }
        TranslationWorkerCommand::CancelModelInstall => installs.cancel(),
        TranslationWorkerCommand::SelectModel {
            catalog_id,
            keep_warm,
        } => {
            coordinator.cancel_all();
            // Choosing another tier abandons an installation still running for
            // the previous one.
            installs.cancel();
            installs.send(InstallCommand::SelectModel {
                catalog_id,
                keep_warm,
            });
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
                Asked::Text {
                    request_id,
                    text,
                    source_language,
                    target_language,
                    language_policy,
                },
            )?;
        }
        TranslationWorkerCommand::TranslateBlocks {
            request_id,
            base_revision,
            blocks,
            context,
            source_language,
            target_language,
            language_policy,
        } => coordinator.submit(
            state,
            writer.clone(),
            Asked::Blocks {
                request_id,
                base_revision,
                blocks,
                context,
                source_language,
                target_language,
                language_policy,
                glossary: glossary.clone(),
            },
        )?,
        TranslationWorkerCommand::SaveGlossaryEntry {
            operation_id,
            request_id,
            block_id,
            expected_revision,
            entry,
        } => match entry.origin {
            GlossaryOrigin::Confirmed => glossary.confirm(
                operation_id,
                request_id,
                block_id,
                expected_revision,
                entry,
                writer.clone(),
            ),
            GlossaryOrigin::Corrected => glossary.save_correction(
                operation_id,
                request_id,
                block_id,
                expected_revision,
                entry,
                writer.clone(),
            ),
        },
        TranslationWorkerCommand::ListGlossary {
            operation_id,
            query,
            limit,
        } => glossary.list(operation_id, query, limit, writer.clone()),
        TranslationWorkerCommand::UpdateGlossaryEntry {
            operation_id,
            entry_id,
            entry,
        } => glossary.update(operation_id, entry_id, entry, writer.clone()),
        TranslationWorkerCommand::DeleteGlossaryEntry {
            operation_id,
            entry_id,
        } => glossary.delete(operation_id, entry_id, writer.clone()),
        TranslationWorkerCommand::Cancel { request_id } => {
            coordinator.cancel(request_id);
        }
        TranslationWorkerCommand::UnloadLocalModel => {
            installs.cancel();
            installs.send(InstallCommand::UnloadResident);
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
            history.recent(limit, writer.clone());
        }
        TranslationWorkerCommand::SetHistoryEnabled { enabled } => {
            history.set_enabled(enabled);
        }
        TranslationWorkerCommand::ClearHistory => {
            history.clear(writer.clone());
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
fn installing_catalog_id(command: &InstallCommand) -> Option<String> {
    match command {
        InstallCommand::Initialize {
            selected_catalog_id,
            ..
        } => selected_catalog_id.clone(),
        InstallCommand::EnsureModel { catalog_id }
        | InstallCommand::RemoveModel { catalog_id }
        | InstallCommand::SelectModel { catalog_id, .. } => Some(catalog_id.clone()),
        InstallCommand::SetLocalEnabled { .. }
        | InstallCommand::UnloadResident
        | InstallCommand::Shutdown => None,
    }
}

fn run_install(
    command: InstallCommand,
    state: &Arc<Mutex<WorkerState>>,
    writer: &EventWriter,
) -> Result<()> {
    match command {
        InstallCommand::Initialize {
            selected_catalog_id,
            selected_tier,
            keep_warm,
            prepare_local_model,
            skip_local_prepare,
        } => {
            let mut state = recover_lock(state);
            state.keep_warm = keep_warm;
            state.local_model_enabled = prepare_local_model;
            let selection = state.initial_selection(selected_catalog_id.as_deref(), selected_tier);
            state.selected_catalog_id = selection.as_ref().map(|(id, _)| id.clone());
            state.selected_tier = selection.map(|(_, tier)| tier);
            writer.send(&TranslationWorkerEvent::Ready {
                protocol_version: PROTOCOL_VERSION,
                selected_catalog_id: state.selected_catalog_id.clone(),
                selected_tier: state.selected_tier,
                models: state.summaries(),
            })?;
            if state.local_model_enabled
                && !skip_local_prepare
                && let Some(catalog_id) = state.selected_catalog_id.clone()
            {
                let tier = state.pack(&catalog_id)?.tier;
                if state.pack(&catalog_id)?.backend.requires_llama_runtime() {
                    state.ensure_runtime(writer, &catalog_id)?;
                }
                let (catalog_id, load_duration_ms) =
                    state.load_engine(writer, &catalog_id, keep_warm)?;
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
        InstallCommand::EnsureModel { catalog_id } => {
            let mut state = recover_lock(state);
            let pack = state.pack(&catalog_id)?.clone();
            let tier = pack.tier;
            let bundle = pack.as_bundle();
            let runtime_download =
                if !pack.backend.requires_llama_runtime() || state.runtime.is_installed() {
                    0
                } else {
                    RUNTIME_DOWNLOAD_BYTES
                };
            let model_download = bundle.total_bytes();
            let combined_total = runtime_download.saturating_add(model_download);
            if pack.backend.requires_llama_runtime() {
                state.runtime.ensure_installed(&state.fetcher, |progress| {
                    let _ = writer.send(&TranslationWorkerEvent::ModelDownloadProgress {
                        catalog_id: pack.id.clone(),
                        downloaded_bytes: progress.bytes_received.min(runtime_download),
                        total_bytes: combined_total,
                    });
                })?;
            }
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
            state.selected_catalog_id = Some(pack.id.clone());
            state.selected_tier = Some(tier);
            writer.send(&TranslationWorkerEvent::ModelSelected {
                catalog_id: pack.id.clone(),
                tier,
            })?;
            if state.local_model_enabled {
                let keep_warm = state.keep_warm;
                let (catalog_id, load_duration_ms) =
                    state.load_engine(writer, &pack.id, keep_warm)?;
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
        InstallCommand::RemoveModel { catalog_id } => {
            let mut state = recover_lock(state);
            let pack = state.pack(&catalog_id)?.clone();
            let tier = pack.tier;
            if state.selected_catalog_id.as_deref() == Some(pack.id.as_str())
                && state.local_model_enabled
            {
                bail!(
                    "switch off the local translation provider before removing its selected model"
                );
            }
            if state.selected_catalog_id.as_deref() == Some(pack.id.as_str()) {
                let had_engine = state.engine.take().is_some();
                if had_engine {
                    writer.send(&TranslationWorkerEvent::LocalModelState {
                        catalog_id: Some(pack.id.clone()),
                        loaded: false,
                    })?;
                }
                state.selected_catalog_id = None;
                state.selected_tier = None;
            }
            let mut removed = state.installer.remove_complete(&pack.as_bundle())?;
            let last_llama_pack_removed = !state
                .catalog
                .packs
                .iter()
                .filter(|candidate| candidate.backend.requires_llama_runtime())
                .any(|candidate| state.model_installed(&candidate.id));
            if last_llama_pack_removed {
                let runtime = state.runtime.remove()?;
                removed.removed_files = removed.removed_files.saturating_add(runtime.removed_files);
                removed.removed_bytes = removed.removed_bytes.saturating_add(runtime.removed_bytes);
            }
            writer.send(&TranslationWorkerEvent::ModelRemoved {
                catalog_id: pack.id,
                tier,
                removed_bytes: removed.removed_bytes,
                runtime_removed: last_llama_pack_removed,
                models: state.summaries(),
            })?;
        }
        InstallCommand::SelectModel {
            catalog_id,
            keep_warm,
        } => {
            let mut state = recover_lock(state);
            if !state.model_installed(&catalog_id) {
                return Err(anyhow!("translation model is not installed"));
            }
            let pack = state.pack(&catalog_id)?.clone();
            let tier = pack.tier;
            state.keep_warm = keep_warm;
            state.selected_catalog_id = Some(pack.id.clone());
            state.selected_tier = Some(tier);
            writer.send(&TranslationWorkerEvent::ModelSelected {
                catalog_id: pack.id.clone(),
                tier,
            })?;
            if state.local_model_enabled {
                if pack.backend.requires_llama_runtime() {
                    state.ensure_runtime(writer, &pack.id)?;
                }
                let (catalog_id, load_duration_ms) =
                    state.load_engine(writer, &pack.id, keep_warm)?;
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
                        state.selected_catalog_id.clone()
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
                    state.selected_catalog_id.clone()
                } else {
                    None
                };
                writer.send(&TranslationWorkerEvent::LocalModelState {
                    catalog_id: unloaded,
                    loaded: false,
                })?;
            } else if let Some(catalog_id) = state.selected_catalog_id.clone() {
                let tier = state.pack(&catalog_id)?.tier;
                if state.pack(&catalog_id)?.backend.requires_llama_runtime() {
                    state.ensure_runtime(writer, &catalog_id)?;
                }
                let keep_warm = state.keep_warm;
                let (catalog_id, load_duration_ms) =
                    state.load_engine(writer, &catalog_id, keep_warm)?;
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
        InstallCommand::UnloadResident => {
            let mut state = recover_lock(state);
            let catalog_id = state
                .engine
                .as_ref()
                .map(|engine| engine.catalog_id().to_owned());
            let _ = release_resident_resource(&mut state.engine);
            writer.send(&TranslationWorkerEvent::LocalModelState {
                catalog_id,
                loaded: false,
            })?;
        }
        InstallCommand::Shutdown => {}
    }
    Ok(())
}

fn choose_initial_tier(
    requested: Option<TranslationTier>,
    compact_installed: bool,
    fast_installed: bool,
    quality_installed: bool,
) -> Option<TranslationTier> {
    if requested == Some(TranslationTier::Compact) && compact_installed {
        return Some(TranslationTier::Compact);
    }
    if requested == Some(TranslationTier::Teacher) && quality_installed {
        return Some(TranslationTier::Teacher);
    }
    if requested == Some(TranslationTier::Tiny) && fast_installed {
        return Some(TranslationTier::Tiny);
    }
    match requested {
        // A stale Qwen choice stays in the contextual-model family when the
        // other Qwen pack is still present. This avoids silently changing an
        // existing user's quality/coverage expectations after an update.
        Some(TranslationTier::Tiny) if quality_installed => Some(TranslationTier::Teacher),
        Some(TranslationTier::Teacher) if fast_installed => Some(TranslationTier::Tiny),
        Some(TranslationTier::Compact) if quality_installed => Some(TranslationTier::Teacher),
        Some(TranslationTier::Compact) if fast_installed => Some(TranslationTier::Tiny),
        _ if compact_installed => Some(TranslationTier::Compact),
        _ if quality_installed => Some(TranslationTier::Teacher),
        _ if fast_installed => Some(TranslationTier::Tiny),
        _ => None,
    }
}

/// One translation the worker has been asked for.
///
/// The four of them travel together through the coordinator and into the
/// request, which is clearer than four more parameters on two signatures that
/// already carry a state, a list and a writer.
enum Asked {
    Text {
        request_id: RequestId,
        text: String,
        /// `None` for either language means "read it from the text", which is what
        /// the desktop sends for every selection.
        source_language: Option<TranslationLanguage>,
        target_language: Option<TranslationLanguage>,
        language_policy: TranslationLanguagePolicy,
    },
    Blocks {
        request_id: RequestId,
        base_revision: u64,
        blocks: Vec<ScreenTextBlock>,
        context: SourceContext,
        source_language: Option<TranslationLanguage>,
        target_language: Option<TranslationLanguage>,
        language_policy: TranslationLanguagePolicy,
        glossary: GlossaryActor,
    },
}

impl Asked {
    fn request_id(&self) -> RequestId {
        match self {
            Self::Text { request_id, .. } | Self::Blocks { request_id, .. } => *request_id,
        }
    }
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
    let source =
        source_language.unwrap_or_else(|| detect_language_with_policy(text, language_policy));
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
    match asked {
        Asked::Text {
            request_id,
            text,
            source_language,
            target_language,
            language_policy,
        } => translate_text_request(
            state,
            providers,
            writer,
            request_id,
            text,
            source_language,
            target_language,
            language_policy,
            is_current,
        ),
        Asked::Blocks {
            request_id,
            base_revision,
            blocks,
            context,
            source_language,
            target_language,
            language_policy,
            glossary,
        } => translate_blocks_request(
            state,
            providers,
            writer,
            request_id,
            base_revision,
            blocks,
            context,
            source_language,
            target_language,
            language_policy,
            &glossary,
            is_current,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn translate_text_request(
    state: &Arc<Mutex<WorkerState>>,
    providers: Option<&mut providers::ProvidersSnapshot>,
    writer: &EventWriter,
    request_id: RequestId,
    text: String,
    source_language: Option<TranslationLanguage>,
    target_language: Option<TranslationLanguage>,
    language_policy: TranslationLanguagePolicy,
    is_current: impl Fn() -> bool,
) -> Result<()> {
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
        return Err(request_failure(
            TranslationFailureKind::NoText,
            "there is no text to translate",
            Vec::new(),
        ));
    }
    let (source, target) =
        resolve_direction(&text, source_language, target_language, language_policy).map_err(
            |error| {
                request_failure(
                    TranslationFailureKind::UnsupportedRequest,
                    format!("{error:#}"),
                    Vec::new(),
                )
            },
        )?;

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
    let mut provider_attempts = Vec::new();
    if let Some(routed) = routed {
        match routed {
            Routed::Translated {
                provider,
                text: translated,
                attempts,
            } => {
                let backend_request_count = attempts
                    .len()
                    .saturating_add(1)
                    .try_into()
                    .unwrap_or(u32::MAX);
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
                        attempts,
                        processing_duration_ms: elapsed_ms(started),
                        first_token_duration_ms: 0,
                        backend_request_count,
                        tokenize_duration_ms: 0,
                        chunk_count: 1,
                        protected_values_changed: false,
                    },
                );
            }
            Routed::Cancelled => return Err(TranslationCancelled.into()),
            Routed::Exhausted { failure } => {
                // Nobody in the list could answer, and the local engine was not
                // in it or was switched off. Saying which service said what is
                // the only way the user can fix it.
                return Err(TranslationRequestFailure(failure).into());
            }
            // The walk reached the local engine. Below is what it does.
            Routed::UseLocalEngine { attempts } => provider_attempts = attempts,
        }
    }

    // The engine, and only now the worker's state.
    with_request_engine(state, writer, |state| {
        let local_failure_kind = if state.selected_tier.is_some() {
            TranslationFailureKind::Internal
        } else {
            TranslationFailureKind::LocalModelMissing
        };
        let (source, target) = state.local_direction(
            (source, target),
            source_language.is_some(),
            target_language,
            language_policy,
        );
        let tier = state
            .ensure_engine(
                writer,
                source,
                target,
                started + TRANSLATION_OPERATION_TIMEOUT,
                &is_current,
            )
            .map_err(|error| {
                local_engine_failure(error, local_failure_kind.clone(), provider_attempts.clone())
            })?;
        let mut last_partial_at = None;
        let mut last_partial_text = String::new();
        let output = state
            .engine
            .as_mut()
            .ok_or_else(|| {
                request_failure(
                    local_failure_kind,
                    "translation engine failed to initialize",
                    provider_attempts.clone(),
                )
            })?
            .translate_streaming_until(
                &text,
                Some(source),
                Some(target),
                started + TRANSLATION_OPERATION_TIMEOUT,
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
            )
            .map_err(|error| {
                if error.downcast_ref::<TranslationCancelled>().is_some() {
                    error
                } else {
                    request_failure(
                        TranslationFailureKind::Internal,
                        format!("{error:#}"),
                        provider_attempts.clone(),
                    )
                }
            })?;
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
            attempts: provider_attempts,
            processing_duration_ms: elapsed_ms(started),
            first_token_duration_ms: output.first_token_duration_ms,
            backend_request_count: output.backend_request_count,
            tokenize_duration_ms: output.tokenize_duration_ms,
            chunk_count: output.chunk_count,
            protected_values_changed: output.protected_values_changed,
        };
        finish(state, writer, result)
    })
}

#[allow(clippy::too_many_arguments)]
fn translate_blocks_request(
    state: &Arc<Mutex<WorkerState>>,
    providers: Option<&mut providers::ProvidersSnapshot>,
    writer: &EventWriter,
    request_id: RequestId,
    base_revision: u64,
    blocks: Vec<ScreenTextBlock>,
    context: SourceContext,
    source_language: Option<TranslationLanguage>,
    target_language: Option<TranslationLanguage>,
    language_policy: TranslationLanguagePolicy,
    glossary: &GlossaryActor,
    is_current: impl Fn() -> bool,
) -> Result<()> {
    let started = Instant::now();
    let combined = blocks
        .iter()
        .map(|block| block.source_text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let (source, target) =
        resolve_direction(&combined, source_language, target_language, language_policy).map_err(
            |error| {
                request_failure(
                    TranslationFailureKind::UnsupportedRequest,
                    format!("{error:#}"),
                    Vec::new(),
                )
            },
        )?;
    if !is_current() {
        return Err(TranslationCancelled.into());
    }

    let mut glossary_matches = Vec::new();
    let mut published = HashMap::new();
    let mut glossary_ids = HashSet::new();
    for block in &blocks {
        if rust_app_core::text_values::is_numeric_only_text(&block.source_text) {
            continue;
        }
        for entry in glossary.lookup(
            block.source_text.clone(),
            source,
            target,
            context.clone(),
            8,
        )? {
            if glossary_ids.insert(entry.entry_id.clone()) {
                glossary_matches.push(entry);
            }
        }
    }

    let plan = BlockTranslationPlan::new(
        blocks,
        context,
        glossary_matches,
        TranslationBackendCapabilities::structured(),
    )?;

    let publication_context = ScreenBlockPublicationContext {
        request_id,
        base_revision,
        source_language: source,
        target_language: target,
        started,
    };
    publish_numeric_screen_blocks(
        &plan,
        writer,
        publication_context,
        &mut published,
        &is_current,
    )?;
    let preserved_numeric_count = published.len();
    if preserved_numeric_count == plan.blocks.len() {
        let translated = merge_screen_blocks(&plan, Vec::new(), &published)?;
        return publish_screen_blocks(
            writer,
            publication_context,
            translated,
            TranslationBackendCapabilities::limited(),
            &mut published,
            &is_current,
        );
    }

    if let Some(snapshot) = providers {
        let publication_context = ScreenBlockPublicationContext {
            request_id,
            base_revision,
            source_language: source,
            target_language: target,
            started,
        };
        if let Some(translated) = translate_provider_blocks(
            &plan,
            writer,
            publication_context,
            &mut published,
            &is_current,
            |block| {
                snapshot.translate_started(
                    &block.source_text,
                    Some(source.code()),
                    target.code(),
                    started,
                    &is_current,
                )
            },
        )? {
            let translated = validate_completed_blocks(&plan, translated)?;
            publish_screen_blocks(
                writer,
                ScreenBlockPublicationContext {
                    request_id,
                    base_revision,
                    source_language: source,
                    target_language: target,
                    started,
                },
                translated,
                TranslationBackendCapabilities::limited(),
                &mut published,
                &is_current,
            )?;
            return Ok(());
        }
    }

    let retained_cloud_blocks = published.len() > preserved_numeric_count;
    with_request_engine(state, writer, |state| {
        let failure_kind = if state.selected_tier.is_some() {
            TranslationFailureKind::Internal
        } else {
            TranslationFailureKind::LocalModelMissing
        };
        let (source, target) = state.local_direction(
            (source, target),
            source_language.is_some(),
            target_language,
            language_policy,
        );
        let _tier = state
            .ensure_engine(
                writer,
                source,
                target,
                started + TRANSLATION_OPERATION_TIMEOUT,
                &is_current,
            )
            .map_err(|error| local_engine_failure(error, failure_kind, Vec::new()))?;
        let local_plan = BlockTranslationPlan::new(
            unpublished_screen_blocks(&plan, &published),
            plan.context.clone(),
            plan.glossary_matches.clone(),
            plan.backend_capabilities,
        )?;
        let publication_context = ScreenBlockPublicationContext {
            request_id,
            base_revision,
            source_language: source,
            target_language: target,
            started,
        };
        let mut publication_error = None;
        let output = state
            .engine
            .as_mut()
            .ok_or_else(|| anyhow!("translation engine failed to initialize"))?
            .translate_blocks_streaming_until(
                &local_plan.blocks,
                &local_plan.context,
                &local_plan.glossary_matches,
                Some(source),
                Some(target),
                started + TRANSLATION_OPERATION_TIMEOUT,
                &is_current,
                |block| {
                    let result = validate_partial_block(&local_plan, block).and_then(|()| {
                        publish_screen_block(
                            writer,
                            publication_context,
                            block,
                            &mut published,
                            &is_current,
                        )
                    });
                    if let Err(error) = result {
                        publication_error = Some(error);
                        false
                    } else {
                        true
                    }
                },
            );
        if let Some(error) = publication_error {
            return Err(error);
        }
        let output = output?;
        let translated = validate_completed_blocks(&local_plan, output.blocks)?;
        let translated = merge_screen_blocks(&plan, translated, &published)?;
        publish_screen_blocks(
            writer,
            ScreenBlockPublicationContext {
                request_id,
                base_revision,
                source_language: source,
                target_language: target,
                started,
            },
            translated,
            if retained_cloud_blocks {
                TranslationBackendCapabilities::limited()
            } else {
                output.capabilities
            },
            &mut published,
            &is_current,
        )?;
        release_engine_unless_warm(state.keep_warm, &mut state.engine, writer)
    })
}

fn publish_numeric_screen_blocks(
    plan: &BlockTranslationPlan,
    writer: &EventWriter,
    context: ScreenBlockPublicationContext,
    published: &mut HashMap<ScreenBlockId, String>,
    is_current: &impl Fn() -> bool,
) -> Result<()> {
    for block in &plan.blocks {
        if !is_current() {
            return Err(TranslationCancelled.into());
        }
        if !rust_app_core::text_values::is_numeric_only_text(&block.source_text) {
            continue;
        }
        let preserved = TranslatedScreenBlock {
            block_id: block.block_id.clone(),
            translated_text: block.source_text.clone(),
            completion: BlockCompletion::Completed,
            failure_message: None,
        };
        validate_partial_block(plan, &preserved)?;
        publish_screen_block(writer, context, &preserved, published, is_current)?;
    }
    Ok(())
}

fn unpublished_screen_blocks(
    plan: &BlockTranslationPlan,
    published: &HashMap<ScreenBlockId, String>,
) -> Vec<ScreenTextBlock> {
    plan.blocks
        .iter()
        .filter(|block| !published.contains_key(&block.block_id))
        .cloned()
        .collect()
}

fn local_engine_failure(
    error: anyhow::Error,
    fallback: TranslationFailureKind,
    attempts: Vec<TranslationAttemptReport>,
) -> anyhow::Error {
    if error.downcast_ref::<TranslationCancelled>().is_some() {
        return error;
    }
    if let Some(LocalPackSelectionError::Missing { catalog_id }) =
        error.downcast_ref::<LocalPackSelectionError>()
    {
        return TranslationRequestFailure(TranslationFailureReport {
            kind: TranslationFailureKind::LocalModelMissing,
            provider_id: None,
            retry_after_ms: None,
            required_catalog_id: Some(catalog_id.clone()),
            detail: format!("{error:#}"),
            attempts,
        })
        .into();
    }
    let kind = if error
        .downcast_ref::<LocalPackSelectionError>()
        .is_some_and(|error| matches!(error, LocalPackSelectionError::UnsupportedPair))
    {
        TranslationFailureKind::UnsupportedRequest
    } else {
        fallback
    };
    request_failure(kind, format!("{error:#}"), attempts)
}

fn merge_screen_blocks(
    plan: &BlockTranslationPlan,
    mut translated: Vec<TranslatedScreenBlock>,
    published: &HashMap<ScreenBlockId, String>,
) -> Result<Vec<TranslatedScreenBlock>> {
    for (block_id, text) in published {
        if let Some(block) = translated.iter().find(|block| &block.block_id == block_id) {
            if block.completion != BlockCompletion::Completed || &block.translated_text != text {
                bail!("translation attempted to replace an accepted screen block");
            }
        } else {
            translated.push(TranslatedScreenBlock {
                block_id: block_id.clone(),
                translated_text: text.clone(),
                completion: BlockCompletion::Completed,
                failure_message: None,
            });
        }
    }
    validate_completed_blocks(plan, translated)
}

fn with_request_engine<T>(
    state: &Mutex<WorkerState>,
    writer: &EventWriter,
    operation: impl FnOnce(&mut WorkerState) -> Result<T>,
) -> Result<T> {
    let mut state = recover_lock(state);
    let result = operation(&mut state);
    state.engine_last_used = Some(Instant::now());
    let keep_warm = state.keep_warm;
    // Keep ownership until the request's resident allocation has been released.
    // An installer or a subsequent request must not replace it between the
    // operation and cleanup. Preserve the original failure if output is closed.
    let cleanup = release_engine_unless_warm(keep_warm, &mut state.engine, writer);
    result.and_then(|value| cleanup.map(|()| value))
}

fn translate_provider_blocks(
    plan: &BlockTranslationPlan,
    writer: &EventWriter,
    context: ScreenBlockPublicationContext,
    published: &mut HashMap<ScreenBlockId, String>,
    is_current: &impl Fn() -> bool,
    mut route: impl FnMut(&ScreenTextBlock) -> Routed,
) -> Result<Option<Vec<TranslatedScreenBlock>>> {
    let mut translated = Vec::with_capacity(plan.blocks.len());
    for block in &plan.blocks {
        if !is_current() {
            return Err(TranslationCancelled.into());
        }
        if let Some(accepted) = published.get(&block.block_id) {
            translated.push(TranslatedScreenBlock {
                block_id: block.block_id.clone(),
                translated_text: accepted.clone(),
                completion: BlockCompletion::Completed,
                failure_message: None,
            });
            continue;
        }
        match route(block) {
            Routed::Translated { text, .. } => {
                let translated_block = TranslatedScreenBlock {
                    block_id: block.block_id.clone(),
                    translated_text: text,
                    completion: BlockCompletion::Completed,
                    failure_message: None,
                };
                validate_partial_block(plan, &translated_block)?;
                publish_screen_block(writer, context, &translated_block, published, is_current)?;
                translated.push(translated_block);
            }
            Routed::UseLocalEngine { .. } => return Ok(None),
            Routed::Cancelled => return Err(TranslationCancelled.into()),
            Routed::Exhausted { failure } => return Err(TranslationRequestFailure(failure).into()),
        }
    }
    Ok(Some(translated))
}

#[derive(Debug, Clone, Copy)]
struct ScreenBlockPublicationContext {
    request_id: RequestId,
    base_revision: u64,
    source_language: TranslationLanguage,
    target_language: TranslationLanguage,
    started: Instant,
}

fn publish_screen_blocks(
    writer: &EventWriter,
    context: ScreenBlockPublicationContext,
    blocks: Vec<TranslatedScreenBlock>,
    capabilities: TranslationBackendCapabilities,
    published: &mut HashMap<ScreenBlockId, String>,
    is_current: &impl Fn() -> bool,
) -> Result<()> {
    if !is_current() {
        return Err(TranslationCancelled.into());
    }
    for block in &blocks {
        publish_screen_block(writer, context, block, published, is_current)?;
    }
    if !is_current() {
        return Err(TranslationCancelled.into());
    }
    eprintln!(
        "screen_translation_completed request_id={} partial_count={} total_count={} elapsed_ms={}",
        context.request_id,
        published.len(),
        blocks.len(),
        elapsed_ms(context.started)
    );
    writer.send(&TranslationWorkerEvent::ScreenBlocksCompleted {
        request_id: context.request_id,
        base_revision: context.base_revision,
        blocks,
        source_language: context.source_language,
        target_language: context.target_language,
        capabilities,
        processing_duration_ms: elapsed_ms(context.started),
    })
}

fn publish_screen_block(
    writer: &EventWriter,
    context: ScreenBlockPublicationContext,
    block: &TranslatedScreenBlock,
    published: &mut HashMap<ScreenBlockId, String>,
    is_current: &impl Fn() -> bool,
) -> Result<()> {
    if !is_current() {
        return Err(TranslationCancelled.into());
    }
    if block.completion != BlockCompletion::Completed {
        return Ok(());
    }
    if let Some(previous) = published.get(&block.block_id) {
        if previous != &block.translated_text {
            bail!("translation attempted to replace an accepted screen block");
        }
        return Ok(());
    }
    writer.send(&TranslationWorkerEvent::ScreenBlockPartial {
        request_id: context.request_id,
        base_revision: context.base_revision,
        block_id: block.block_id.clone(),
        translated_text: block.translated_text.clone(),
    })?;
    published.insert(block.block_id.clone(), block.translated_text.clone());
    if published.len() == 1 {
        eprintln!(
            "screen_translation_first_partial request_id={} elapsed_ms={}",
            context.request_id,
            elapsed_ms(context.started)
        );
    }
    Ok(())
}

#[cfg(test)]
mod screen_progress_tests {
    use super::*;

    #[test]
    fn numeric_passthrough_finishes_without_model_and_cancellation_publishes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let (history, history_thread) =
            TranslationHistoryActor::start(root.path().join("history.json")).unwrap();
        let (glossary, glossary_thread) =
            GlossaryActor::start(root.path().join("glossary.json")).unwrap();
        let mut worker = WorkerState::new(history.clone()).unwrap();
        worker.local_model_enabled = false;
        worker.installer = BundleInstaller::new(root.path().join("missing-models"));
        worker.runtime = LlamaRuntimeManager::new(root.path().join("missing-runtime"));
        let state = Arc::new(Mutex::new(worker));
        let mut plan = provider_plan();
        plan.blocks[0].source_text = "198-5130".into();
        plan.blocks[1].source_text = "+25 35% 002-03".into();
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let result = translate_blocks_request(
            &state,
            None,
            &writer,
            RequestId::new(),
            0,
            plan.blocks.clone(),
            plan.context.clone(),
            Some(TranslationLanguage::English),
            Some(TranslationLanguage::Russian),
            TranslationLanguagePolicy::default(),
            &glossary,
            || true,
        );
        let emitted = received.try_iter().collect::<Vec<_>>();
        let cancelled = translate_blocks_request(
            &state,
            None,
            &writer,
            RequestId::new(),
            0,
            plan.blocks.clone(),
            plan.context.clone(),
            Some(TranslationLanguage::English),
            Some(TranslationLanguage::Russian),
            TranslationLanguagePolicy::default(),
            &glossary,
            || false,
        );
        glossary.shutdown();
        glossary_thread.join().unwrap();
        history.shutdown();
        history_thread.join().unwrap();
        result.unwrap();
        assert!(recover_lock(&state).engine.is_none());
        assert_eq!(emitted.len(), 3, "two partials and final completion only");
        let TranslationWorkerEvent::ScreenBlocksCompleted { blocks, .. } = emitted.last().unwrap()
        else {
            panic!("numeric request must complete without any backend");
        };
        for (source, translated) in plan.blocks.iter().zip(blocks) {
            assert_eq!(source.block_id, translated.block_id);
            assert_eq!(source.source_text, translated.translated_text);
            assert_eq!(translated.completion, BlockCompletion::Completed);
        }
        assert!(
            cancelled
                .unwrap_err()
                .downcast_ref::<TranslationCancelled>()
                .is_some()
        );
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn numeric_passthrough_never_routes_to_provider_but_mixed_text_stays_validated() {
        let mut plan = provider_plan();
        plan.blocks[0].source_text = "198-5130".into();
        for mixed_result in ["Урон 25", "Урон 26"] {
            let (events, received) = mpsc::channel();
            let writer = EventWriter { events };
            let mut published = HashMap::new();
            let mut routed = Vec::new();
            publish_numeric_screen_blocks(&plan, &writer, context(), &mut published, &|| true)
                .unwrap();
            let result = translate_provider_blocks(
                &plan,
                &writer,
                context(),
                &mut published,
                &|| true,
                |block| {
                    routed.push(block.block_id.clone());
                    Routed::Translated {
                        provider: "fixture".into(),
                        text: mixed_result.into(),
                        attempts: Vec::new(),
                    }
                },
            );
            assert_eq!(routed, vec![ScreenBlockId::from("b")]);
            assert_eq!(
                published.get(&ScreenBlockId::from("a")).map(String::as_str),
                Some("198-5130")
            );
            if mixed_result == "Урон 25" {
                let output = result.unwrap().unwrap();
                assert_eq!(output[0].translated_text, "198-5130");
                assert_eq!(output[1].translated_text, mixed_result);
                assert_eq!(received.try_iter().count(), 2);
            } else {
                assert!(result.is_err());
                assert_eq!(received.try_iter().count(), 1);
            }
        }
    }

    fn provider_plan() -> BlockTranslationPlan {
        BlockTranslationPlan::new(
            ["a", "b"]
                .into_iter()
                .map(|id| ScreenTextBlock {
                    block_id: id.into(),
                    source_text: "Damage 25".into(),
                    bounds: rust_protocol::PhysicalRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 20,
                    },
                    quad: None,
                    fragment_quads: Vec::new(),
                    fragments: Vec::new(),
                    confidence_milli: 900,
                })
                .collect(),
            SourceContext::default(),
            Vec::new(),
            TranslationBackendCapabilities::limited(),
        )
        .unwrap()
    }

    #[test]
    fn cloud_success_is_published_before_the_next_route_and_survives_later_failure() {
        for cancelled in [false, true] {
            let (events, received) = mpsc::channel();
            let writer = EventWriter { events };
            let mut published = HashMap::new();
            let result = translate_provider_blocks(
                &provider_plan(),
                &writer,
                context(),
                &mut published,
                &|| true,
                |block| {
                    if block.block_id.as_str() == "a" {
                        Routed::Translated {
                            provider: "fixture".into(),
                            text: "Урон 25".into(),
                            attempts: Vec::new(),
                        }
                    } else {
                        assert!(
                            matches!(received.try_recv(), Ok(TranslationWorkerEvent::ScreenBlockPartial { block_id, .. }) if block_id.as_str() == "a")
                        );
                        if cancelled {
                            Routed::Cancelled
                        } else {
                            Routed::Exhausted {
                                failure: TranslationFailureReport {
                                    kind: TranslationFailureKind::Internal,
                                    provider_id: Some("fixture".into()),
                                    retry_after_ms: None,
                                    required_catalog_id: None,
                                    detail: "fixture provider failure".into(),
                                    attempts: Vec::new(),
                                },
                            }
                        }
                    }
                },
            );
            assert!(result.is_err());
            assert_eq!(
                published.get(&ScreenBlockId::from("a")).map(String::as_str),
                Some("Урон 25")
            );
        }
    }

    #[test]
    fn cloud_success_survives_local_fallback_and_invalid_rows_are_not_published() {
        for last_route in ["local", "invalid"] {
            let (events, received) = mpsc::channel();
            let writer = EventWriter { events };
            let mut published = HashMap::new();
            let result = translate_provider_blocks(
                &provider_plan(),
                &writer,
                context(),
                &mut published,
                &|| true,
                |block| {
                    if block.block_id.as_str() == "a" {
                        Routed::Translated {
                            provider: "fixture".into(),
                            text: "Урон 25".into(),
                            attempts: Vec::new(),
                        }
                    } else if last_route == "local" {
                        Routed::UseLocalEngine {
                            attempts: Vec::new(),
                        }
                    } else {
                        Routed::Translated {
                            provider: "fixture".into(),
                            text: "Урон 26".into(),
                            attempts: Vec::new(),
                        }
                    }
                },
            );
            if last_route == "local" {
                assert!(matches!(result, Ok(None)));
            } else {
                assert!(result.is_err());
            }
            assert_eq!(
                published.get(&ScreenBlockId::from("a")).map(String::as_str),
                Some("Урон 25")
            );
            assert!(!published.contains_key(&ScreenBlockId::from("b")));
            assert_eq!(received.try_iter().count(), 1);
        }
    }

    #[test]
    fn local_fallback_only_requests_pending_rows_and_merges_without_replacing_cloud_success() {
        let plan = provider_plan();
        let published = HashMap::from([(ScreenBlockId::from("a"), "Урон 25".to_owned())]);
        let pending = unpublished_screen_blocks(&plan, &published);
        assert_eq!(
            pending
                .iter()
                .map(|block| block.block_id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        let local = TranslatedScreenBlock {
            block_id: "b".into(),
            translated_text: "Повреждение 25".into(),
            completion: BlockCompletion::Completed,
            failure_message: None,
        };
        let merged = merge_screen_blocks(&plan, vec![local], &published).unwrap();
        assert_eq!(
            merged
                .iter()
                .map(|block| (block.block_id.as_str(), block.translated_text.as_str()))
                .collect::<Vec<_>>(),
            [("a", "Урон 25"), ("b", "Повреждение 25")]
        );
        let replaced = TranslatedScreenBlock {
            block_id: "a".into(),
            translated_text: "Замена 25".into(),
            completion: BlockCompletion::Completed,
            failure_message: None,
        };
        assert!(merge_screen_blocks(&plan, vec![replaced], &published).is_err());
    }

    fn context() -> ScreenBlockPublicationContext {
        ScreenBlockPublicationContext {
            request_id: RequestId::new(),
            base_revision: 3,
            source_language: TranslationLanguage::English,
            target_language: TranslationLanguage::Russian,
            started: Instant::now(),
        }
    }

    fn translated() -> TranslatedScreenBlock {
        TranslatedScreenBlock {
            block_id: ScreenBlockId::from("a"),
            translated_text: "Урон 25".to_owned(),
            completion: BlockCompletion::Completed,
            failure_message: None,
        }
    }

    #[test]
    fn screen_final_completion_keeps_streamed_rows_without_duplicate_partials() {
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let mut published = HashMap::new();
        let context = context();
        let accepted = translated();
        let failed = TranslatedScreenBlock {
            block_id: ScreenBlockId::from("b"),
            translated_text: String::new(),
            completion: BlockCompletion::Failed,
            failure_message: Some("screen translation interrupted (backend_failure)".to_owned()),
        };
        publish_screen_block(&writer, context, &accepted, &mut published, &|| true).unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            TranslationWorkerEvent::ScreenBlockPartial { .. }
        ));
        publish_screen_blocks(
            &writer,
            context,
            vec![accepted, failed],
            TranslationBackendCapabilities::structured(),
            &mut published,
            &|| true,
        )
        .unwrap();
        match received.try_recv().unwrap() {
            TranslationWorkerEvent::ScreenBlocksCompleted {
                blocks,
                source_language,
                target_language,
                ..
            } => {
                assert_eq!(blocks.len(), 2);
                assert_eq!(blocks[0].translated_text, "Урон 25");
                assert_eq!(blocks[1].completion, BlockCompletion::Failed);
                assert_eq!(source_language, TranslationLanguage::English);
                assert_eq!(target_language, TranslationLanguage::Russian);
            }
            event => panic!("expected terminal completion, got {event:?}"),
        }
        assert_eq!(received.try_iter().count(), 0);
    }

    #[test]
    fn screen_partial_writer_failure_is_returned_without_recording_publication() {
        let (events, received) = mpsc::channel();
        drop(received);
        let writer = EventWriter { events };
        let mut published = HashMap::new();
        assert!(
            publish_screen_block(&writer, context(), &translated(), &mut published, &|| true)
                .is_err()
        );
        assert!(published.is_empty());
    }

    #[test]
    fn screen_partial_is_sent_once_and_cannot_be_replaced() {
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let mut published = HashMap::new();
        let context = context();
        let mut block = translated();
        publish_screen_block(&writer, context, &block, &mut published, &|| true).unwrap();
        publish_screen_block(&writer, context, &block, &mut published, &|| true).unwrap();
        assert_eq!(received.try_iter().count(), 1);
        block.translated_text = "Иной текст 25".to_owned();
        assert!(publish_screen_block(&writer, context, &block, &mut published, &|| true).is_err());
        assert_eq!(received.try_iter().count(), 0);
    }

    #[test]
    fn screen_partial_never_publishes_failed_or_stale_rows() {
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let mut published = HashMap::new();
        let context = context();
        let mut block = translated();
        assert!(publish_screen_block(&writer, context, &block, &mut published, &|| false).is_err());
        block.completion = BlockCompletion::Failed;
        publish_screen_block(&writer, context, &block, &mut published, &|| true).unwrap();
        assert_eq!(received.try_iter().count(), 0);
    }
}

/// With keep-warm off the engine lives for one request. Dropping it without a
/// word left the desktop showing the model as loaded (Resources, Models) long
/// after its memory was returned.
fn release_engine_unless_warm(
    keep_warm: bool,
    engine: &mut Option<TranslationEngine>,
    writer: &EventWriter,
) -> Result<()> {
    if keep_warm {
        return Ok(());
    }
    let Some(engine) = engine.take() else {
        return Ok(());
    };
    let catalog_id = engine.catalog_id().to_owned();
    drop(engine);
    writer.send(&TranslationWorkerEvent::LocalModelState {
        catalog_id: Some(catalog_id),
        loaded: false,
    })
}

/// Records a finished translation and tells the desktop about it.
///
/// Shared by both paths so that a translation from a service is kept, reported
/// and counted exactly like one from the machine — the only difference being
/// what the result says about where it came from.
fn finish(state: &mut WorkerState, writer: &EventWriter, result: TranslationResult) -> Result<()> {
    let created_unix_ms = unix_ms();
    state.history.append(TranslationHistoryEntry {
        result: result.clone(),
        created_unix_ms,
    });
    let result = writer.send(&TranslationWorkerEvent::Completed {
        result,
        created_unix_ms,
    });
    let cleanup = release_engine_unless_warm(state.keep_warm, &mut state.engine, writer);
    result.and(cleanup)
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
            choose_initial_tier(Some(TranslationTier::Tiny), true, true, true),
            Some(TranslationTier::Tiny)
        );
        assert_eq!(
            choose_initial_tier(Some(TranslationTier::Teacher), true, true, true),
            Some(TranslationTier::Teacher)
        );
        assert_eq!(
            choose_initial_tier(Some(TranslationTier::Compact), true, true, true),
            Some(TranslationTier::Compact)
        );
    }

    #[test]
    fn recovers_to_best_installed_model_when_selection_is_missing_or_stale() {
        assert_eq!(
            choose_initial_tier(None, true, true, true),
            Some(TranslationTier::Compact)
        );
        assert_eq!(
            choose_initial_tier(Some(TranslationTier::Tiny), true, false, true),
            Some(TranslationTier::Teacher)
        );
        assert_eq!(
            choose_initial_tier(Some(TranslationTier::Teacher), true, true, false),
            Some(TranslationTier::Tiny)
        );
        assert_eq!(
            choose_initial_tier(Some(TranslationTier::Compact), false, true, true),
            Some(TranslationTier::Teacher)
        );
    }

    #[test]
    fn leaves_translation_unselected_when_no_model_is_installed() {
        assert_eq!(choose_initial_tier(None, false, false, false), None);
    }
}

#[cfg(test)]
mod direction_tests {
    use rust_protocol::{TranslationLanguage, TranslationLanguagePolicy};
    use rust_translation_engine::TranslationCatalog;

    use super::{LocalPackSelectionError, resolve_direction, select_local_catalog_id};

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

    #[test]
    fn compact_mode_routes_each_request_to_its_installed_language_package() {
        let catalog = TranslationCatalog::load().unwrap();
        let installed = ["firefox-ru-en-v3.1", "firefox-de-en-v3"];

        assert_eq!(
            select_local_catalog_id(
                &catalog,
                Some(rust_protocol::TranslationTier::Compact),
                Some("firefox-ru-en-v3.1"),
                TranslationLanguage::German,
                TranslationLanguage::English,
                |id| installed.contains(&id),
            )
            .unwrap(),
            "firefox-de-en-v3"
        );
    }

    #[test]
    fn a_detected_language_without_a_pack_falls_back_to_an_installed_one_of_the_same_script() {
        let catalog = TranslationCatalog::load().unwrap();
        let policy = TranslationLanguagePolicy {
            primary: TranslationLanguage::Russian,
            secondary: TranslationLanguage::English,
        };
        let only_ru_en = |id: &str| id == "firefox-ru-en-v3.1";
        assert_eq!(
            super::local_direction_fallback(
                &catalog,
                Some(rust_protocol::TranslationTier::Compact),
                Some("firefox-ru-en-v3.1"),
                (TranslationLanguage::German, TranslationLanguage::Russian),
                None,
                policy,
                only_ru_en,
            ),
            (TranslationLanguage::English, TranslationLanguage::Russian)
        );
        // An installed German pack keeps the detected German.
        assert_eq!(
            super::local_direction_fallback(
                &catalog,
                Some(rust_protocol::TranslationTier::Compact),
                Some("firefox-ru-en-v3.1"),
                (TranslationLanguage::German, TranslationLanguage::English),
                None,
                policy,
                |id| id == "firefox-ru-en-v3.1" || id == "firefox-de-en-v3",
            ),
            (TranslationLanguage::German, TranslationLanguage::English)
        );
        // No script match: the detected direction stays and fails as before.
        assert_eq!(
            super::local_direction_fallback(
                &catalog,
                Some(rust_protocol::TranslationTier::Compact),
                Some("firefox-ru-en-v3.1"),
                (TranslationLanguage::Japanese, TranslationLanguage::Russian),
                None,
                policy,
                only_ru_en,
            ),
            (TranslationLanguage::Japanese, TranslationLanguage::Russian)
        );
    }

    #[test]
    fn noisy_english_screen_text_uses_the_installed_english_russian_package() {
        let catalog = TranslationCatalog::load().unwrap();
        for text in [
            "Wrapped up past your usage limit Resets at 5:40 PM Learn more\n2\nGet more usage\nX\nc",
            "Wrapped up past your usage limit  Resets at 5:40 PM О Learn more\nGet more usage\nХ\nо,",
            "Wrappedup past your usage limit  Resets at 5:40 PM Learn more\nGet more usage\nХ",
            "Wrappedup past your usage limit\nResets at 5:40 PM\nLearn more\nGet more usage\nХ",
        ] {
            let (source, target) =
                resolve_direction(text, None, None, TranslationLanguagePolicy::default()).unwrap();
            assert_eq!(
                (source, target),
                (TranslationLanguage::English, TranslationLanguage::Russian),
                "{text}",
            );
            assert_eq!(
                select_local_catalog_id(
                    &catalog,
                    Some(rust_protocol::TranslationTier::Compact),
                    Some("firefox-ru-en-v3.1"),
                    source,
                    target,
                    |id| id == "firefox-ru-en-v3.1",
                )
                .unwrap(),
                "firefox-ru-en-v3.1",
            );
        }
    }

    #[test]
    fn an_ambiguous_word_uses_the_configured_pair_instead_of_an_unrelated_language() {
        assert_eq!(
            resolve_direction(
                "hello",
                None,
                None,
                TranslationLanguagePolicy {
                    primary: TranslationLanguage::German,
                    secondary: TranslationLanguage::French,
                },
            )
            .unwrap(),
            (TranslationLanguage::German, TranslationLanguage::French),
        );
    }

    #[test]
    fn explicit_languages_override_screen_detection_and_the_configured_pair() {
        assert_eq!(
            resolve_direction(
                "Wrapped up past your usage limit Resets at 5:40 PM Learn more",
                Some(TranslationLanguage::French),
                Some(TranslationLanguage::German),
                TranslationLanguagePolicy::default(),
            )
            .unwrap(),
            (TranslationLanguage::French, TranslationLanguage::German),
        );
    }

    #[test]
    fn real_languages_outside_the_configured_pair_keep_their_direction() {
        for (text, expected) in [
            ("Bonjour tout le monde", TranslationLanguage::French),
            ("En savoir plus", TranslationLanguage::French),
            ("Guten Morgen", TranslationLanguage::German),
            ("Mehr erfahren", TranslationLanguage::German),
            ("Hola mundo", TranslationLanguage::Spanish),
            (
                "Це повне українське речення.",
                TranslationLanguage::Ukrainian,
            ),
        ] {
            assert_eq!(
                resolve_direction(text, None, None, TranslationLanguagePolicy::default()).unwrap(),
                (expected, TranslationLanguage::English),
                "{text}",
            );
        }
    }

    #[test]
    fn compact_mode_names_the_exact_package_that_is_missing() {
        let catalog = TranslationCatalog::load().unwrap();

        assert_eq!(
            select_local_catalog_id(
                &catalog,
                Some(rust_protocol::TranslationTier::Compact),
                Some("firefox-ru-en-v3.1"),
                TranslationLanguage::German,
                TranslationLanguage::English,
                |id| id == "firefox-ru-en-v3.1",
            )
            .unwrap_err(),
            LocalPackSelectionError::Missing {
                catalog_id: "firefox-de-en-v3".to_owned()
            }
        );
    }
}

#[cfg(test)]
mod local_model_policy_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use rust_protocol::{TranslationProviderEntry, TranslationTier};

    use super::{
        InstallCommand, command_after_queued_cancellation, install_failure_code,
        local_translation_enabled, release_resident_resource, replace_resident_resource,
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
                catalog_id: "firefox-de-en-v3".to_owned(),
            }),
            "model_remove"
        );
        assert_eq!(
            install_failure_code(&InstallCommand::EnsureModel {
                catalog_id: "firefox-ru-en-v3.1".to_owned(),
            }),
            "translation_worker"
        );
    }

    #[test]
    fn a_cancel_supersedes_expensive_work_that_was_still_queued() {
        for command in [
            InstallCommand::EnsureModel {
                catalog_id: "firefox-ru-en-v3.1".to_owned(),
            },
            InstallCommand::SelectModel {
                catalog_id: "qwen35-9b-q4".to_owned(),
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
                selected_catalog_id: Some("qwen35-9b-q4".to_owned()),
                selected_tier: Some(TranslationTier::Teacher),
                keep_warm: true,
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
                    catalog_id: "firefox-de-en-v3".to_owned(),
                },
                1,
                2,
            ),
            Some(InstallCommand::RemoveModel {
                catalog_id,
            })
            if catalog_id == "firefox-de-en-v3"
        ));
    }

    #[test]
    fn model_replacement_drops_the_old_resident_before_loading_the_new_one() {
        struct Resident(Arc<AtomicUsize>);
        impl Drop for Resident {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }

        let resident_count = Arc::new(AtomicUsize::new(1));
        let mut slot = Some(Resident(Arc::clone(&resident_count)));
        let release_announced = Arc::new(AtomicUsize::new(0));
        replace_resident_resource(
            &mut slot,
            || {
                assert_eq!(resident_count.load(Ordering::SeqCst), 0);
                release_announced.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(())
            },
            || {
                assert_eq!(release_announced.load(Ordering::SeqCst), 1);
                resident_count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(Resident(Arc::clone(&resident_count)))
            },
        )
        .unwrap();

        assert_eq!(resident_count.load(Ordering::SeqCst), 1);
        assert_eq!(release_announced.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_model_replacement_still_announces_that_the_old_model_was_released() {
        let mut slot = Some("old");
        let release_announced = AtomicUsize::new(0);

        let result = replace_resident_resource(
            &mut slot,
            || {
                release_announced.fetch_add(1, Ordering::SeqCst);
                Ok::<_, &'static str>(())
            },
            || Err("new model failed"),
        );

        assert_eq!(result, Err("new model failed"));
        assert!(slot.is_none());
        assert_eq!(release_announced.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn manual_memory_release_drops_only_the_resident_engine_slot() {
        let mut slot = Some(String::from("resident"));

        assert_eq!(
            release_resident_resource(&mut slot),
            Some(String::from("resident"))
        );
        assert!(slot.is_none());
        assert_eq!(release_resident_resource(&mut slot), None);
    }
}

#[cfg(test)]
mod keep_warm_release_tests {
    use std::sync::mpsc;

    use rust_protocol::TranslationWorkerEvent;
    use rust_translation_engine::{TranslationCatalog, TranslationEngine};

    use super::{EventWriter, release_engine_unless_warm};

    fn engine() -> TranslationEngine {
        let catalog = TranslationCatalog::load().unwrap();
        let pack = catalog.packs.first().unwrap().clone();
        let root = tempfile::tempdir().unwrap();
        TranslationEngine::new(
            pack,
            root.path().join("models"),
            root.path().join("runtimes"),
        )
    }

    fn engine_for(requires_llama: bool) -> TranslationEngine {
        let catalog = TranslationCatalog::load().unwrap();
        let pack = catalog
            .packs
            .iter()
            .find(|pack| pack.backend.requires_llama_runtime() == requires_llama)
            .unwrap()
            .clone();
        let root = tempfile::tempdir().unwrap();
        TranslationEngine::new(
            pack,
            root.path().join("models"),
            root.path().join("runtimes"),
        )
    }

    #[test]
    fn only_an_idle_contextual_model_is_released_while_warm() {
        let root = tempfile::tempdir().unwrap();
        let (history, history_thread) =
            super::TranslationHistoryActor::start(root.path().join("history.json")).unwrap();
        let mut state = super::WorkerState::new(history.clone()).unwrap();
        state.keep_warm = true;
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let now = std::time::Instant::now();
        let long_ago = now - super::CONTEXTUAL_MODEL_IDLE_RELEASE;

        state.engine = Some(engine_for(false));
        state.engine_last_used = Some(long_ago);
        state.release_idle_contextual_engine(&writer, now).unwrap();
        assert!(state.engine.is_some(), "the compact model stays warm");

        state.engine = Some(engine_for(true));
        state.engine_last_used = Some(now);
        state.release_idle_contextual_engine(&writer, now).unwrap();
        assert!(state.engine.is_some(), "a recently used model stays");

        state.engine_last_used = Some(long_ago);
        state.release_idle_contextual_engine(&writer, now).unwrap();
        assert!(state.engine.is_none(), "an idle contextual model is released");
        assert!(received.try_iter().any(|event| matches!(
            event,
            TranslationWorkerEvent::LocalModelState { loaded: false, .. }
        )));
        history.shutdown();
        history_thread.join().unwrap();
    }

    #[test]
    fn a_cold_engine_is_released_after_the_request_and_the_desktop_is_told() {
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let mut slot = Some(engine());
        let catalog_id = slot.as_ref().unwrap().catalog_id().to_owned();

        release_engine_unless_warm(false, &mut slot, &writer).unwrap();

        assert!(slot.is_none());
        match received.try_recv().unwrap() {
            TranslationWorkerEvent::LocalModelState {
                catalog_id: released,
                loaded,
            } => {
                assert_eq!(released.as_deref(), Some(catalog_id.as_str()));
                assert!(!loaded);
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    #[test]
    fn a_warm_engine_stays_and_nothing_is_announced() {
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let mut slot = Some(engine());

        release_engine_unless_warm(true, &mut slot, &writer).unwrap();

        assert!(slot.is_some());
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn local_request_failure_releases_its_engine_without_masking_the_failure() {
        let root = tempfile::tempdir().unwrap();
        let (history, history_thread) =
            super::TranslationHistoryActor::start(root.path().join("history.json")).unwrap();
        let mut state = super::WorkerState::new(history.clone()).unwrap();
        state.keep_warm = false;
        state.engine = Some(engine());
        state.selected_tier = None;
        let state = std::sync::Arc::new(std::sync::Mutex::new(state));
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        let result = super::translate_request(
            &state,
            None,
            &writer,
            super::Asked::Text {
                request_id: rust_protocol::RequestId::new(),
                text: "Hello".into(),
                source_language: Some(rust_protocol::TranslationLanguage::English),
                target_language: Some(rust_protocol::TranslationLanguage::Russian),
                language_policy: rust_protocol::TranslationLanguagePolicy::default(),
            },
            || true,
        );
        history.shutdown();
        history_thread.join().unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no translation model")
        );
        assert!(super::recover_lock(&state).engine.is_none());
        assert!(received.try_iter().any(|event| matches!(
            event,
            TranslationWorkerEvent::LocalModelState { loaded: false, .. }
        )));
    }

    #[test]
    fn request_cleanup_covers_success_failure_and_cancel_while_holding_the_owner_lock() {
        let root = tempfile::tempdir().unwrap();
        let (history, history_thread) =
            super::TranslationHistoryActor::start(root.path().join("history.json")).unwrap();
        for warm in [false, true] {
            for outcome in ["success", "failure", "cancelled"] {
                let mut state = super::WorkerState::new(history.clone()).unwrap();
                state.keep_warm = warm;
                state.engine = Some(engine());
                let state = std::sync::Mutex::new(state);
                let (events, received) = mpsc::channel();
                let result = super::with_request_engine(&state, &EventWriter { events }, |_| {
                    assert!(
                        state.try_lock().is_err(),
                        "the request must retain exclusive ownership"
                    );
                    match outcome {
                        "success" => Ok(()),
                        "failure" => Err(anyhow::anyhow!("fixture failure")),
                        _ => Err(rust_translation_engine::TranslationCancelled.into()),
                    }
                });
                assert_eq!(result.is_ok(), outcome == "success");
                if outcome == "cancelled" {
                    assert!(
                        result
                            .unwrap_err()
                            .downcast_ref::<rust_translation_engine::TranslationCancelled>()
                            .is_some()
                    );
                }
                assert_eq!(
                    super::recover_lock(&state).engine.is_some(),
                    warm,
                    "{outcome}, warm={warm}"
                );
                assert_eq!(received.try_iter().count(), usize::from(!warm));
            }
        }
        history.shutdown();
        history_thread.join().unwrap();
    }

    #[test]
    fn expired_or_cancelled_worker_request_never_initializes_a_model() {
        let root = tempfile::tempdir().unwrap();
        let (history, history_thread) =
            super::TranslationHistoryActor::start(root.path().join("history.json")).unwrap();
        let mut state = super::WorkerState::new(history.clone()).unwrap();
        let (events, received) = mpsc::channel();
        let writer = EventWriter { events };
        for cancelled in [false, true] {
            let error = state
                .ensure_engine(
                    &writer,
                    rust_protocol::TranslationLanguage::English,
                    rust_protocol::TranslationLanguage::Russian,
                    std::time::Instant::now(),
                    &|| !cancelled,
                )
                .unwrap_err();
            if cancelled {
                assert!(
                    error
                        .downcast_ref::<rust_translation_engine::TranslationCancelled>()
                        .is_some(),
                    "{error:#}"
                );
            } else {
                assert!(error.to_string().contains("deadline"), "{error:#}");
            }
            assert!(state.engine.is_none());
            assert!(received.try_recv().is_err());
        }
        history.shutdown();
        history_thread.join().unwrap();
    }

    #[test]
    fn closed_completion_stream_still_releases_a_cold_engine() {
        let root = tempfile::tempdir().unwrap();
        let (history, history_thread) =
            super::TranslationHistoryActor::start(root.path().join("history.json")).unwrap();
        let mut state = super::WorkerState::new(history.clone()).unwrap();
        state.keep_warm = false;
        state.engine = Some(engine());
        let (events, received) = mpsc::channel();
        drop(received);
        let result = super::finish(
            &mut state,
            &EventWriter { events },
            rust_protocol::TranslationResult {
                request_id: rust_protocol::RequestId::new(),
                source_text: "Hello".into(),
                translated_text: "Привет".into(),
                source_language: rust_protocol::TranslationLanguage::English,
                target_language: rust_protocol::TranslationLanguage::Russian,
                tier: None,
                provider: Some("fixture".into()),
                attempts: Vec::new(),
                processing_duration_ms: 1,
                first_token_duration_ms: 0,
                backend_request_count: 1,
                tokenize_duration_ms: 0,
                chunk_count: 1,
                protected_values_changed: false,
            },
        );
        history.shutdown();
        history_thread.join().unwrap();
        assert!(result.is_err());
        assert!(state.engine.is_none());
    }
}
