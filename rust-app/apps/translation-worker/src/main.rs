#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod history;
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

use anyhow::{Context, Result, anyhow};
use history::TranslationHistory;
use runtime::{LlamaRuntimeManager, RUNTIME_DOWNLOAD_BYTES};
use rust_model_manager::{BundleInstaller, HttpFetcher};
use rust_protocol::{
    PROTOCOL_VERSION, RequestId, TranslationHistoryEntry, TranslationLanguage,
    TranslationModelSummary, TranslationResult, TranslationTier, TranslationWorkerCommand,
    TranslationWorkerEvent,
};
use rust_translation_engine::{
    TranslationCancelled, TranslationCatalog, TranslationEngine, TranslationPackDescriptor,
};

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

struct TranslationCoordinator {
    generation: Arc<AtomicU64>,
    current_request: Arc<Mutex<Option<RequestId>>>,
    threads: Vec<JoinHandle<()>>,
}

impl TranslationCoordinator {
    fn new() -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            current_request: Arc::new(Mutex::new(None)),
            threads: Vec::new(),
        }
    }

    fn submit(
        &mut self,
        state: Arc<Mutex<WorkerState>>,
        writer: EventWriter,
        request_id: RequestId,
        text: String,
        source_language: Option<TranslationLanguage>,
        target_language: Option<TranslationLanguage>,
    ) -> Result<()> {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *recover_lock(&self.current_request) = Some(request_id);
        let current_generation = Arc::clone(&self.generation);
        let current_request = Arc::clone(&self.current_request);
        let thread = thread::Builder::new()
            .name(format!("translation-{}", short_id(request_id)))
            .spawn(move || {
                let mut state = recover_lock(&state);
                let result = translate_request(
                    &mut state,
                    &writer,
                    request_id,
                    text,
                    source_language,
                    target_language,
                    || current_generation.load(Ordering::Acquire) == generation,
                );
                let cancelled = current_generation.load(Ordering::Acquire) != generation
                    || result.as_ref().err().is_some_and(|error| {
                        error.downcast_ref::<TranslationCancelled>().is_some()
                    });
                if cancelled {
                    let _ = writer.send(&TranslationWorkerEvent::Cancelled { request_id });
                } else if let Err(error) = result {
                    let _ = writer.send(&TranslationWorkerEvent::Failed {
                        request_id: Some(request_id),
                        code: if state.selected_tier.is_some() {
                            "translation_failed"
                        } else {
                            "model_required"
                        }
                        .to_owned(),
                        message: format!("{error:#}"),
                        retryable: state.selected_tier.is_some(),
                    });
                }
                let mut active = recover_lock(&current_request);
                if *active == Some(request_id) {
                    *active = None;
                }
            })?;
        self.threads.push(thread);
        Ok(())
    }

    fn cancel(&self, request_id: RequestId) {
        if *recover_lock(&self.current_request) == Some(request_id) {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn cancel_all(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn reap(&mut self) {
        let mut index = 0;
        while index < self.threads.len() {
            if self.threads[index].is_finished() {
                let thread = self.threads.swap_remove(index);
                let _ = thread.join();
            } else {
                index += 1;
            }
        }
    }

    fn shutdown(mut self) {
        self.cancel_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

struct WorkerState {
    catalog: TranslationCatalog,
    installer: BundleInstaller,
    runtime: LlamaRuntimeManager,
    fetcher: HttpFetcher,
    selected_tier: Option<TranslationTier>,
    keep_warm: bool,
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
    let state = Arc::new(Mutex::new(WorkerState::new()?));
    let stdin = io::stdin();
    let (writer, output_thread) = EventWriter::start()?;
    let mut coordinator = TranslationCoordinator::new();
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
        coordinator.reap();
        if let Err(error) = handle_command(command, Arc::clone(&state), &writer, &mut coordinator) {
            writer.send(&TranslationWorkerEvent::Failed {
                request_id: None,
                code: "translation_worker".to_owned(),
                message: format!("{error:#}"),
                retryable: true,
            })?;
        }
    }
    coordinator.shutdown();
    drop(writer);
    let _ = output_thread.join();
    Ok(())
}

fn handle_command(
    command: TranslationWorkerCommand,
    state: Arc<Mutex<WorkerState>>,
    writer: &EventWriter,
    coordinator: &mut TranslationCoordinator,
) -> Result<()> {
    match command {
        TranslationWorkerCommand::Initialize {
            selected_tier,
            keep_warm,
            history_enabled,
        } => {
            coordinator.cancel_all();
            let mut state = recover_lock(&state);
            state.keep_warm = keep_warm;
            state.history_enabled = history_enabled;
            state.selected_tier = state.initial_tier(selected_tier);
            writer.send(&TranslationWorkerEvent::Ready {
                protocol_version: PROTOCOL_VERSION,
                selected_tier: state.selected_tier,
                models: state.summaries(),
            })?;
            if let Some(tier) = state.selected_tier {
                let catalog_id = state.pack(tier)?.id.clone();
                state.ensure_runtime(writer, &catalog_id)?;
                let (catalog_id, load_duration_ms) = state.load_engine(tier, keep_warm)?;
                writer.send(&TranslationWorkerEvent::ModelReady {
                    catalog_id,
                    tier,
                    load_duration_ms,
                })?;
            }
        }
        TranslationWorkerCommand::EnsureModel { tier } => {
            coordinator.cancel_all();
            let mut state = recover_lock(&state);
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
            let keep_warm = state.keep_warm;
            let (catalog_id, load_duration_ms) = state.load_engine(tier, keep_warm)?;
            writer.send(&TranslationWorkerEvent::ModelSelected { tier })?;
            writer.send(&TranslationWorkerEvent::ModelReady {
                catalog_id,
                tier,
                load_duration_ms,
            })?;
        }
        TranslationWorkerCommand::SelectTier { tier, keep_warm } => {
            coordinator.cancel_all();
            let mut state = recover_lock(&state);
            if !state.model_installed(tier) {
                return Err(anyhow!("translation model is not installed"));
            }
            state.keep_warm = keep_warm;
            state.selected_tier = Some(tier);
            let catalog_id = state.pack(tier)?.id.clone();
            state.ensure_runtime(writer, &catalog_id)?;
            let (catalog_id, load_duration_ms) = state.load_engine(tier, keep_warm)?;
            writer.send(&TranslationWorkerEvent::ModelSelected { tier })?;
            writer.send(&TranslationWorkerEvent::ModelReady {
                catalog_id,
                tier,
                load_duration_ms,
            })?;
        }
        TranslationWorkerCommand::Translate {
            request_id,
            text,
            source_language,
            target_language,
        } => {
            coordinator.submit(
                state,
                writer.clone(),
                request_id,
                text,
                source_language,
                target_language,
            )?;
        }
        TranslationWorkerCommand::Cancel { request_id } => {
            coordinator.cancel(request_id);
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

fn translate_request(
    state: &mut WorkerState,
    writer: &EventWriter,
    request_id: RequestId,
    text: String,
    source_language: Option<TranslationLanguage>,
    target_language: Option<TranslationLanguage>,
    is_current: impl Fn() -> bool,
) -> Result<()> {
    let tier = state.ensure_engine(writer)?;
    let started = Instant::now();
    let mut last_partial_at = None;
    let mut last_partial_text = String::new();
    let output = state
        .engine
        .as_mut()
        .context("translation engine failed to initialize")?
        .translate_streaming(
            &text,
            source_language,
            target_language,
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
        tier,
        processing_duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        first_token_duration_ms: output.first_token_duration_ms,
        backend_request_count: output.backend_request_count,
        tokenize_duration_ms: output.tokenize_duration_ms,
        chunk_count: output.chunk_count,
    };
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

fn short_id(request_id: RequestId) -> String {
    request_id.to_string().chars().take(8).collect()
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
