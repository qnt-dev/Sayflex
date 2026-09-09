//! Owns model installation so the command loop never blocks on it.
//!
//! Installing a speech model downloads gigabytes and then hashes them. Running
//! that inside the loop that reads standard input meant the worker stopped
//! reading commands for the whole operation: `Cancel`, `Shutdown` and every
//! dictation request queued up behind it in a pipe buffer of a few kilobytes,
//! and the installation itself could not be stopped at all.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread,
};

use anyhow::{Context, Result};
use rust_model_manager::{
    CancelToken, HttpFetcher, InstallPhase, ModelCatalog, ModelDescriptor, ModelInstallError,
    ModelInstaller,
};
use rust_protocol::SpeechWorkerEvent;

use crate::inference::InferenceHandle;

pub enum ModelCommand {
    /// Installs the model if needed and loads it. `announce_selection` marks an
    /// explicit user choice rather than start-up restoration.
    Ensure {
        catalog_id: String,
        announce_selection: bool,
    },
    /// Cancels any install/load ahead of this command, then releases the
    /// currently prepared model from the inference actor.
    Deactivate,
    Shutdown,
}

/// One queued request, carrying the cancellation count it was sent under.
///
/// Without it, a cancel arriving while the request is still in the queue is
/// erased by the reset the installer performs on the way in, and the download
/// the user just stopped starts anyway.
struct QueuedCommand {
    command: ModelCommand,
    cancels_at_send: u64,
}

#[derive(Clone)]
pub struct ModelActor {
    commands: Sender<QueuedCommand>,
    cancellation: CancelToken,
    cancels: Arc<AtomicU64>,
}

impl ModelActor {
    pub fn send(&self, command: ModelCommand) {
        let _ = self.commands.send(QueuedCommand {
            command,
            cancels_at_send: self.cancels.load(Ordering::Acquire),
        });
    }

    /// Asks the current installation to stop, and any already queued.
    pub fn cancel(&self) {
        self.cancels.fetch_add(1, Ordering::AcqRel);
        self.cancellation.cancel();
    }

    pub fn deactivate(&self) {
        self.cancel();
        self.send(ModelCommand::Deactivate);
    }
}

pub fn start(
    catalog: ModelCatalog,
    installer: ModelInstaller,
    fetcher: HttpFetcher,
    events: Sender<SpeechWorkerEvent>,
    inference: InferenceHandle,
) -> Result<ModelActor> {
    let (commands, requests) = mpsc::channel::<QueuedCommand>();
    let cancellation = fetcher.cancellation();
    let installer_cancellation = cancellation.clone();
    let cancels = Arc::new(AtomicU64::new(0));
    let worker_cancels = Arc::clone(&cancels);
    thread::Builder::new()
        .name("speech-model-installer".to_owned())
        .spawn(move || {
            for request in requests {
                match request.command {
                    ModelCommand::Ensure {
                        catalog_id,
                        announce_selection,
                    } => {
                        if worker_cancels.load(Ordering::Acquire) != request.cancels_at_send {
                            // Cancelled while it was still in the queue.
                            continue;
                        }
                        // A cancellation older than this request must not
                        // cancel it; a newer one already skipped it above.
                        installer_cancellation.reset();
                        match ensure_model(&catalog, &installer, &fetcher, &catalog_id, &events) {
                            Ok(path) => {
                                inference.load_model(catalog_id, path, announce_selection);
                            }
                            Err(error) => {
                                let cancelled =
                                    error.downcast_ref::<ModelInstallError>().is_some_and(
                                        |error| matches!(error, ModelInstallError::Cancelled),
                                    );
                                if cancelled {
                                    let _ = events.send(SpeechWorkerEvent::Failed {
                                        session_id: None,
                                        code: "model_install_cancelled".to_owned(),
                                        message: "Model installation was cancelled.".to_owned(),
                                        retryable: true,
                                    });
                                } else {
                                    let _ = events.send(SpeechWorkerEvent::Failed {
                                        session_id: None,
                                        code: "model_install".to_owned(),
                                        message: format!("{error:#}"),
                                        retryable: true,
                                    });
                                }
                            }
                        }
                    }
                    ModelCommand::Deactivate => inference.unload_model(),
                    ModelCommand::Shutdown => break,
                }
            }
        })?;
    Ok(ModelActor {
        commands,
        cancellation,
        cancels,
    })
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

/// Reuses the model the earlier Windows application already verified, without
/// copying or modifying it.
fn try_adopt_existing_windows_model(installer: &ModelInstaller, model: &ModelDescriptor) {
    let destination = installer.model_path(model);
    if destination.exists() {
        return;
    }
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let existing = std::path::Path::new(&local)
        .join("Keyboop")
        .join("models")
        .join(&model.artifact.file_name);
    if std::fs::metadata(&existing).is_ok_and(|metadata| metadata.len() == model.artifact.bytes) {
        if let Some(parent) = destination.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::hard_link(&existing, &destination) {
            eprintln!(
                "could not adopt existing model {}: {error}",
                existing.display()
            );
        }
    }
}
