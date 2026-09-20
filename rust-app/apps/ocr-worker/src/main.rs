#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    env,
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
use rust_ocr_engine::{CancellationToken, OcrEngine, OcrError, decode_rgba_request};
#[cfg(feature = "screen-translation-dev")]
use rust_ocr_engine::{PpOcrAdapter, PpOcrModelPaths};
use rust_protocol::{
    OcrFailureKind, OcrWorkerCommand, OcrWorkerEvent, PROTOCOL_VERSION, RequestId,
};

#[derive(Clone)]
struct EventWriter {
    events: Sender<OcrWorkerEvent>,
}

impl EventWriter {
    fn stdout() -> Result<(Self, JoinHandle<()>)> {
        let (events, receiver) = mpsc::channel::<OcrWorkerEvent>();
        let output_thread = thread::Builder::new()
            .name("ocr-worker-json-output".to_owned())
            .spawn(move || {
                let stdout = io::stdout();
                let mut writer = BufWriter::new(stdout.lock());
                for event in receiver {
                    if serde_json::to_writer(&mut writer, &event).is_err()
                        || writer.write_all(b"\n").is_err()
                        || writer.flush().is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok((Self { events }, output_thread))
    }

    fn send(&self, event: OcrWorkerEvent) -> Result<()> {
        self.events
            .send(event)
            .map_err(|error| anyhow!("OCR event stream closed: {error}"))
    }

    #[cfg(test)]
    fn test_channel() -> (Self, mpsc::Receiver<OcrWorkerEvent>) {
        let (events, receiver) = mpsc::channel();
        (Self { events }, receiver)
    }
}

struct ActiveRequest {
    generation: u64,
    cancellation: CancellationToken,
}

struct RuntimeInner {
    engine: Option<OcrEngine>,
    writer: EventWriter,
    active: Mutex<HashMap<RequestId, ActiveRequest>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    next_generation: AtomicU64,
}

#[derive(Clone)]
struct WorkerRuntime {
    inner: Arc<RuntimeInner>,
}

impl WorkerRuntime {
    fn new(engine: Option<OcrEngine>, writer: EventWriter) -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                engine,
                writer,
                active: Mutex::new(HashMap::new()),
                threads: Mutex::new(Vec::new()),
                next_generation: AtomicU64::new(1),
            }),
        }
    }

    #[cfg(test)]
    fn unavailable(writer: EventWriter) -> Self {
        Self::new(None, writer)
    }

    fn handle(&self, command: OcrWorkerCommand) -> bool {
        match command {
            OcrWorkerCommand::Recognize {
                request_id,
                width,
                height,
                stride,
                pixel_format: _,
                language_policy,
                rgba_base64,
            } => {
                let generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
                let cancellation = CancellationToken::new();
                if let Some(replaced) = lock(&self.inner.active).insert(
                    request_id,
                    ActiveRequest {
                        generation,
                        cancellation: cancellation.clone(),
                    },
                ) {
                    replaced.cancellation.cancel();
                }
                let runtime = self.clone();
                let handle = thread::Builder::new()
                    .name(format!("ocr-request-{request_id}"))
                    .spawn(move || {
                        runtime.process_recognition(
                            request_id,
                            generation,
                            width,
                            height,
                            stride,
                            rgba_base64,
                            language_policy,
                            cancellation,
                        );
                    });
                match handle {
                    Ok(handle) => {
                        let mut threads = lock(&self.inner.threads);
                        reap_finished(&mut threads);
                        threads.push(handle);
                    }
                    Err(_) => {
                        if self.finish_if_current(request_id, generation) {
                            let _ = self.inner.writer.send(OcrWorkerEvent::Failed {
                                request_id,
                                kind: OcrFailureKind::Engine,
                                message: error_message(OcrError::Engine).to_owned(),
                            });
                        }
                    }
                }
                true
            }
            OcrWorkerCommand::Cancel { request_id } => {
                if let Some(active) = lock(&self.inner.active).remove(&request_id) {
                    active.cancellation.cancel();
                    let _ = self
                        .inner
                        .writer
                        .send(OcrWorkerEvent::Cancelled { request_id });
                }
                true
            }
            OcrWorkerCommand::Shutdown => {
                self.shutdown();
                false
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process_recognition(
        &self,
        request_id: RequestId,
        generation: u64,
        width: u32,
        height: u32,
        stride: u32,
        rgba_base64: String,
        language_policy: rust_protocol::OcrLanguagePolicy,
        cancellation: CancellationToken,
    ) {
        let started = Instant::now();
        let result = decode_rgba_request(width, height, stride, &rgba_base64).and_then(|image| {
            if cancellation.is_cancelled() {
                return Err(OcrError::Cancelled);
            }
            self.inner
                .engine
                .as_ref()
                .ok_or(OcrError::ModelUnavailable)?
                .recognize(&image, &language_policy, &cancellation)
        });
        drop(rgba_base64);

        if !self.finish_if_current(request_id, generation) {
            return;
        }
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match result {
            Ok(document) => {
                let average_confidence = if document.lines.is_empty() {
                    0
                } else {
                    document
                        .lines
                        .iter()
                        .map(|line| u64::from(line.confidence_milli))
                        .sum::<u64>()
                        / document.lines.len() as u64
                };
                eprintln!(
                    "ocr request={request_id} dimensions={width}x{height} lines={} confidence_milli={average_confidence} duration_ms={elapsed_ms}",
                    document.lines.len()
                );
                let _ = self.inner.writer.send(OcrWorkerEvent::Recognized {
                    request_id,
                    lines: document.lines,
                    language: document.language,
                    processing_duration_ms: elapsed_ms,
                });
            }
            Err(OcrError::Cancelled) => {
                let _ = self
                    .inner
                    .writer
                    .send(OcrWorkerEvent::Cancelled { request_id });
            }
            Err(error) => {
                eprintln!(
                    "ocr request={request_id} dimensions={width}x{height} failure={} duration_ms={elapsed_ms}",
                    failure_label(error)
                );
                let _ = self.inner.writer.send(OcrWorkerEvent::Failed {
                    request_id,
                    kind: failure_kind(error),
                    message: error_message(error).to_owned(),
                });
            }
        }
    }

    fn finish_if_current(&self, request_id: RequestId, generation: u64) -> bool {
        let mut active = lock(&self.inner.active);
        if active
            .get(&request_id)
            .is_some_and(|active| active.generation == generation)
        {
            active.remove(&request_id);
            true
        } else {
            false
        }
    }

    fn shutdown(&self) {
        let cancelled = lock(&self.inner.active)
            .drain()
            .map(|(request_id, active)| {
                active.cancellation.cancel();
                request_id
            })
            .collect::<Vec<_>>();
        for request_id in cancelled {
            let _ = self
                .inner
                .writer
                .send(OcrWorkerEvent::Cancelled { request_id });
        }
        for handle in lock(&self.inner.threads).drain(..) {
            let _ = handle.join();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn reap_finished(threads: &mut Vec<JoinHandle<()>>) {
    let mut pending = Vec::with_capacity(threads.len());
    for handle in threads.drain(..) {
        if handle.is_finished() {
            let _ = handle.join();
        } else {
            pending.push(handle);
        }
    }
    *threads = pending;
}

const fn failure_kind(error: OcrError) -> OcrFailureKind {
    match error {
        OcrError::ModelUnavailable => OcrFailureKind::ModelUnavailable,
        OcrError::LanguageUnavailable => OcrFailureKind::LanguageUnavailable,
        OcrError::InvalidImage | OcrError::ImageTooLarge => OcrFailureKind::InvalidImage,
        OcrError::Engine => OcrFailureKind::Engine,
        OcrError::Cancelled => OcrFailureKind::Cancelled,
    }
}

const fn failure_label(error: OcrError) -> &'static str {
    match error {
        OcrError::ModelUnavailable => "model_unavailable",
        OcrError::LanguageUnavailable => "language_unavailable",
        OcrError::InvalidImage => "invalid_image",
        OcrError::ImageTooLarge => "image_too_large",
        OcrError::Engine => "engine",
        OcrError::Cancelled => "cancelled",
    }
}

const fn error_message(error: OcrError) -> &'static str {
    match error {
        OcrError::ModelUnavailable => "The local OCR model is unavailable.",
        OcrError::LanguageUnavailable => "The requested OCR language is unavailable.",
        OcrError::InvalidImage | OcrError::ImageTooLarge => "The captured image is invalid.",
        OcrError::Engine => "The local OCR engine failed.",
        OcrError::Cancelled => "The OCR request was cancelled.",
    }
}

#[cfg(feature = "screen-translation-dev")]
fn configured_engine() -> (Option<OcrEngine>, &'static str) {
    let bundle = rust_model_manager::screen_ocr_bundle();
    let root = env::var_os("SAYFLEX_OCR_BUNDLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            rust_app_core::product::data_root()
                .join("ocr-models")
                .join(&bundle.id)
        });
    let Some(installer_root) = root.parent().map(PathBuf::from) else {
        return (None, "ppocrv5-cpu-unavailable");
    };
    let installer = rust_model_manager::BundleInstaller::new(installer_root);
    if installer.bundle_path(&bundle) != root || !installer.is_installed(&bundle).unwrap_or(false) {
        return (None, "ppocrv5-cpu-unavailable");
    }
    let paths = PpOcrModelPaths::from_bundle_root(root);
    match PpOcrAdapter::load(&paths) {
        Ok(adapter) => (Some(OcrEngine::new(Arc::new(adapter))), "ppocrv5-cpu"),
        Err(_) => (None, "ppocrv5-cpu-unavailable"),
    }
}

#[cfg(not(feature = "screen-translation-dev"))]
fn configured_engine() -> (Option<OcrEngine>, &'static str) {
    let _ = PathBuf::new();
    (None, "disabled")
}

fn main() -> Result<()> {
    rust_platform_windows::configure_safe_dll_search()?;
    if env::args().any(|argument| argument == "--build-id") {
        println!("{}", rust_protocol::BUILD_COMMIT);
        return Ok(());
    }
    if env::args().any(|argument| argument == "--protocol-version") {
        println!("{PROTOCOL_VERSION}");
        return Ok(());
    }

    let (writer, output_thread) = EventWriter::stdout()?;
    let (engine, engine_name) = configured_engine();
    writer.send(OcrWorkerEvent::Ready {
        protocol_version: PROTOCOL_VERSION,
        engine: engine_name.to_owned(),
    })?;
    let runtime = WorkerRuntime::new(engine, writer.clone());
    let stdin = io::stdin();
    for line in BufReader::new(stdin.lock()).lines() {
        let line = line.context("failed to read OCR command")?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(command) = serde_json::from_str::<OcrWorkerCommand>(&line) else {
            eprintln!("ocr invalid_command");
            continue;
        };
        if !runtime.handle(command) {
            break;
        }
    }
    runtime.shutdown();
    drop(runtime);
    drop(writer);
    output_thread
        .join()
        .map_err(|_| anyhow!("OCR output thread panicked"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Condvar, Mutex},
        time::Duration,
    };

    use base64::Engine as _;
    use image::{Rgba, RgbaImage};
    use rust_ocr_engine::{CancellationToken, InferenceAdapter, OcrDocument, OcrError};
    use rust_protocol::{
        OcrFailureKind, OcrLanguagePolicy, OcrLine, OcrPixelFormat, OcrWorkerCommand,
        OcrWorkerEvent, PhysicalRect, RequestId,
    };

    use super::{EventWriter, WorkerRuntime};

    struct FixtureAdapter;

    impl InferenceAdapter for FixtureAdapter {
        fn recognize(
            &self,
            _image: &RgbaImage,
            _language_policy: &OcrLanguagePolicy,
            _cancellation: &CancellationToken,
        ) -> Result<OcrDocument, OcrError> {
            Ok(OcrDocument {
                lines: vec![OcrLine {
                    text: "recognized".to_owned(),
                    bounds: PhysicalRect {
                        x: 1,
                        y: 2,
                        width: 3,
                        height: 4,
                    },
                    confidence_milli: 950,
                }],
                language: Some("en".to_owned()),
            })
        }
    }

    struct BlockingAdapter {
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl InferenceAdapter for BlockingAdapter {
        fn recognize(
            &self,
            _image: &RgbaImage,
            _language_policy: &OcrLanguagePolicy,
            cancellation: &CancellationToken,
        ) -> Result<OcrDocument, OcrError> {
            let (open, changed) = &*self.gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = changed.wait(open).unwrap();
            }
            if cancellation.is_cancelled() {
                Err(OcrError::Cancelled)
            } else {
                Ok(OcrDocument {
                    lines: Vec::new(),
                    language: None,
                })
            }
        }
    }

    fn request(request_id: RequestId, pixels: &str) -> OcrWorkerCommand {
        OcrWorkerCommand::Recognize {
            request_id,
            width: 1,
            height: 1,
            stride: 4,
            pixel_format: OcrPixelFormat::Rgba8,
            language_policy: OcrLanguagePolicy::Auto,
            rgba_base64: pixels.to_owned(),
        }
    }

    #[test]
    fn invalid_image_is_typed_without_echoing_payload() {
        let (writer, events) = EventWriter::test_channel();
        let runtime = WorkerRuntime::unavailable(writer);
        let request_id = RequestId::new();

        runtime.handle(request(request_id, "private-screen-pixels"));

        let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            event,
            OcrWorkerEvent::Failed {
                request_id,
                kind: OcrFailureKind::InvalidImage,
                message: "The captured image is invalid.".to_owned(),
            }
        );
        assert!(!format!("{event:?}").contains("private-screen-pixels"));
        runtime.shutdown();
    }

    #[test]
    fn recognized_event_preserves_geometry() {
        let (writer, events) = EventWriter::test_channel();
        let engine = rust_ocr_engine::OcrEngine::new(Arc::new(FixtureAdapter));
        let runtime = WorkerRuntime::new(Some(engine), writer);
        let request_id = RequestId::new();
        let pixels = base64::engine::general_purpose::STANDARD
            .encode(RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255])).as_raw());

        runtime.handle(request(request_id, &pixels));

        let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
        let OcrWorkerEvent::Recognized {
            lines, language, ..
        } = event
        else {
            panic!("expected recognized event");
        };
        assert_eq!(lines[0].bounds.x, 1);
        assert_eq!(language.as_deref(), Some("en"));
        runtime.shutdown();
    }

    #[test]
    fn cancellation_emits_once_and_suppresses_late_result() {
        let (writer, events) = EventWriter::test_channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let engine = rust_ocr_engine::OcrEngine::new(Arc::new(BlockingAdapter {
            gate: Arc::clone(&gate),
        }));
        let runtime = WorkerRuntime::new(Some(engine), writer);
        let request_id = RequestId::new();
        let pixels = base64::engine::general_purpose::STANDARD.encode([1_u8, 2, 3, 255]);
        runtime.handle(request(request_id, &pixels));
        runtime.handle(OcrWorkerCommand::Cancel { request_id });

        assert_eq!(
            events.recv_timeout(Duration::from_secs(2)).unwrap(),
            OcrWorkerEvent::Cancelled { request_id }
        );
        let (open, changed) = &*gate;
        *open.lock().unwrap() = true;
        changed.notify_all();
        runtime.shutdown();
        assert!(events.try_recv().is_err());
    }
}
