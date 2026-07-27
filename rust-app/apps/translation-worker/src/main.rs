#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod download;
mod history;

use std::{
    env,
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use download::HttpFetcher;
use history::TranslationHistory;
use rust_model_manager::BundleInstaller;
use rust_protocol::{
    PROTOCOL_VERSION, RequestId, TranslationHistoryEntry, TranslationLanguage,
    TranslationModelSummary, TranslationResult, TranslationTier, TranslationWorkerCommand,
    TranslationWorkerEvent,
};
use rust_translation_engine::{TranslationCatalog, TranslationEngine, TranslationPackDescriptor};

struct EventWriter {
    writer: BufWriter<io::Stdout>,
}

impl EventWriter {
    fn new() -> Self {
        Self {
            writer: BufWriter::new(io::stdout()),
        }
    }

    fn send(&mut self, event: &TranslationWorkerEvent) -> Result<()> {
        serde_json::to_writer(&mut self.writer, event)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

struct WorkerState {
    catalog: TranslationCatalog,
    installer: BundleInstaller,
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
            fetcher: HttpFetcher::new()?,
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

    fn installed(&self, tier: TranslationTier) -> bool {
        self.pack(tier).ok().is_some_and(|pack| {
            self.installer
                .is_installed(&pack.as_bundle())
                .unwrap_or(false)
        })
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

    fn load_engine(&mut self, tier: TranslationTier, warm: bool) -> Result<(String, u64)> {
        let started = Instant::now();
        let pack = self.pack(tier)?.clone();
        let root = self.installer.bundle_path(&pack.as_bundle());
        let mut engine = TranslationEngine::new(pack.clone(), root);
        if warm {
            engine.warm()?;
        }
        self.engine = Some(engine);
        Ok((
            pack.id,
            started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        ))
    }

    fn ensure_engine(&mut self, writer: &mut EventWriter) -> Result<TranslationTier> {
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
    let mut state = WorkerState::new()?;
    let stdin = io::stdin();
    let mut writer = EventWriter::new();
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
        if let Err(error) = handle_command(command, &mut state, &mut writer) {
            writer.send(&TranslationWorkerEvent::Failed {
                request_id: None,
                code: "translation_worker".to_owned(),
                message: format!("{error:#}"),
                retryable: true,
            })?;
        }
    }
    Ok(())
}

fn handle_command(
    command: TranslationWorkerCommand,
    state: &mut WorkerState,
    writer: &mut EventWriter,
) -> Result<()> {
    match command {
        TranslationWorkerCommand::Initialize {
            selected_tier,
            keep_warm,
            history_enabled,
        } => {
            state.keep_warm = keep_warm;
            state.history_enabled = history_enabled;
            state.selected_tier = selected_tier.filter(|tier| state.installed(*tier));
            writer.send(&TranslationWorkerEvent::Ready {
                protocol_version: PROTOCOL_VERSION,
                selected_tier: state.selected_tier,
                models: state.summaries(),
            })?;
            if let Some(tier) = state.selected_tier {
                let (catalog_id, load_duration_ms) = state.load_engine(tier, keep_warm)?;
                writer.send(&TranslationWorkerEvent::ModelReady {
                    catalog_id,
                    tier,
                    load_duration_ms,
                })?;
            }
        }
        TranslationWorkerCommand::EnsureModel { tier } => {
            let pack = state.pack(tier)?.clone();
            let bundle = pack.as_bundle();
            let path = state
                .installer
                .ensure_installed(&bundle, &state.fetcher, |progress| {
                    let _ = writer.send(&TranslationWorkerEvent::ModelDownloadProgress {
                        catalog_id: pack.id.clone(),
                        downloaded_bytes: progress.bytes_received,
                        total_bytes: progress.total_bytes,
                    });
                })?;
            writer.send(&TranslationWorkerEvent::ModelInstalled {
                catalog_id: pack.id.clone(),
                tier,
                path: path.to_string_lossy().into_owned(),
            })?;
            state.selected_tier = Some(tier);
            let (catalog_id, load_duration_ms) = state.load_engine(tier, state.keep_warm)?;
            writer.send(&TranslationWorkerEvent::ModelSelected { tier })?;
            writer.send(&TranslationWorkerEvent::ModelReady {
                catalog_id,
                tier,
                load_duration_ms,
            })?;
        }
        TranslationWorkerCommand::SelectTier { tier, keep_warm } => {
            if !state.installed(tier) {
                return Err(anyhow!("translation model is not installed"));
            }
            state.keep_warm = keep_warm;
            state.selected_tier = Some(tier);
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
            if let Err(error) = translate_request(
                state,
                writer,
                request_id,
                text,
                source_language,
                target_language,
            ) {
                writer.send(&TranslationWorkerEvent::Failed {
                    request_id: Some(request_id),
                    code: if state.selected_tier.is_some() {
                        "translation_failed"
                    } else {
                        "model_required"
                    }
                    .to_owned(),
                    message: format!("{error:#}"),
                    retryable: state.selected_tier.is_some(),
                })?;
            }
        }
        TranslationWorkerCommand::GetHistory { limit } => {
            writer.send(&TranslationWorkerEvent::History {
                entries: state.history.recent(limit),
            })?;
        }
        TranslationWorkerCommand::SetHistoryEnabled { enabled } => {
            state.history_enabled = enabled;
        }
        TranslationWorkerCommand::ClearHistory => {
            state.history.clear();
            writer.send(&TranslationWorkerEvent::History {
                entries: Vec::new(),
            })?;
        }
        TranslationWorkerCommand::Shutdown => {}
    }
    Ok(())
}

fn translate_request(
    state: &mut WorkerState,
    writer: &mut EventWriter,
    request_id: RequestId,
    text: String,
    source_language: Option<TranslationLanguage>,
    target_language: Option<TranslationLanguage>,
) -> Result<()> {
    let tier = state.ensure_engine(writer)?;
    let started = Instant::now();
    let (translated_text, source_language, target_language) = state
        .engine
        .as_mut()
        .context("translation engine failed to initialize")?
        .translate(&text, source_language, target_language)?;
    let result = TranslationResult {
        request_id,
        source_text: text,
        translated_text,
        source_language,
        target_language,
        tier,
        processing_duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
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
    env::var_os("LOCALAPPDATA").map_or_else(
        || {
            env::current_dir()
                .unwrap_or_default()
                .join(".rust-voice-data")
        },
        |root| Path::new(&root).join("KeyboopRust"),
    )
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
