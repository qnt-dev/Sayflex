#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env,
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
use rust_ocr_engine::{CancellationToken, OcrEngine, OcrError, decode_rgba_request};
#[cfg(feature = "screen-translation")]
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
    request_id: RequestId,
    generation: u64,
    cancellation: CancellationToken,
}

struct RecognitionJob {
    active: ActiveRequest,
    width: u32,
    height: u32,
    stride: u32,
    rgba_base64: String,
    language_policy: rust_protocol::OcrLanguagePolicy,
}

#[derive(Default)]
struct RecognitionQueue {
    active: Option<ActiveRequest>,
    pending: Option<RecognitionJob>,
    shutdown: bool,
}

struct RuntimeInner {
    engine: Option<OcrEngine>,
    writer: EventWriter,
    queue: Mutex<RecognitionQueue>,
    queue_changed: Condvar,
    worker_thread: Mutex<Option<JoinHandle<()>>>,
    next_generation: AtomicU64,
}

#[derive(Clone)]
struct WorkerRuntime {
    inner: Arc<RuntimeInner>,
}

impl WorkerRuntime {
    fn new(engine: Option<OcrEngine>, writer: EventWriter) -> Self {
        let runtime = Self {
            inner: Arc::new(RuntimeInner {
                engine,
                writer,
                queue: Mutex::new(RecognitionQueue::default()),
                queue_changed: Condvar::new(),
                worker_thread: Mutex::new(None),
                next_generation: AtomicU64::new(1),
            }),
        };
        let worker_runtime = runtime.clone();
        let worker = thread::Builder::new()
            .name("ocr-inference".to_owned())
            .spawn(move || worker_runtime.run_queue())
            .expect("could not start the OCR inference thread");
        *lock(&runtime.inner.worker_thread) = Some(worker);
        runtime
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
                let job = RecognitionJob {
                    active: ActiveRequest {
                        request_id,
                        generation,
                        cancellation: cancellation.clone(),
                    },
                    width,
                    height,
                    stride,
                    rgba_base64,
                    language_policy,
                };
                let replaced = {
                    let mut queue = lock(&self.inner.queue);
                    if queue.shutdown {
                        return false;
                    }
                    queue.pending.replace(job)
                };
                if let Some(replaced) = replaced {
                    replaced.active.cancellation.cancel();
                    let _ = self.inner.writer.send(OcrWorkerEvent::Cancelled {
                        request_id: replaced.active.request_id,
                    });
                }
                self.inner.queue_changed.notify_one();
                true
            }
            OcrWorkerCommand::Cancel { request_id } => {
                let pending_cancelled = {
                    let mut queue = lock(&self.inner.queue);
                    let active_cancelled = queue
                        .active
                        .as_ref()
                        .is_some_and(|active| active.request_id == request_id);
                    if let Some(active) = queue
                        .active
                        .as_ref()
                        .filter(|active| active.request_id == request_id)
                    {
                        active.cancellation.cancel();
                    }
                    let pending_cancelled = if queue
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.active.request_id == request_id)
                        && let Some(pending) = queue.pending.take()
                    {
                        pending.active.cancellation.cancel();
                        true
                    } else {
                        false
                    };
                    pending_cancelled && !active_cancelled
                };
                if pending_cancelled {
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

    fn run_queue(&self) {
        loop {
            let job = {
                let mut queue = lock(&self.inner.queue);
                while queue.pending.is_none() && !queue.shutdown {
                    queue = self
                        .inner
                        .queue_changed
                        .wait(queue)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                if queue.shutdown {
                    return;
                }
                let job = queue.pending.take().expect("pending OCR job disappeared");
                queue.active = Some(ActiveRequest {
                    request_id: job.active.request_id,
                    generation: job.active.generation,
                    cancellation: job.active.cancellation.clone(),
                });
                job
            };
            self.process_recognition(
                job.active.request_id,
                job.active.generation,
                job.width,
                job.height,
                job.stride,
                job.rgba_base64,
                job.language_policy,
                job.active.cancellation,
            );
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
        let mut queue = lock(&self.inner.queue);
        if queue.active.as_ref().is_some_and(|active| {
            active.request_id == request_id && active.generation == generation
        }) {
            queue.active = None;
            true
        } else {
            false
        }
    }

    fn shutdown(&self) {
        let cancelled = {
            let mut queue = lock(&self.inner.queue);
            queue.shutdown = true;
            let mut cancelled = Vec::with_capacity(1);
            if let Some(active) = queue.active.as_ref() {
                active.cancellation.cancel();
            }
            if let Some(pending) = queue.pending.take() {
                pending.active.cancellation.cancel();
                if queue
                    .active
                    .as_ref()
                    .is_none_or(|active| active.request_id != pending.active.request_id)
                {
                    cancelled.push(pending.active.request_id);
                }
            }
            cancelled
        };
        for request_id in cancelled {
            let _ = self
                .inner
                .writer
                .send(OcrWorkerEvent::Cancelled { request_id });
        }
        self.inner.queue_changed.notify_all();
        if let Some(handle) = lock(&self.inner.worker_thread).take() {
            let _ = handle.join();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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

#[cfg(feature = "screen-translation")]
fn configured_engine() -> (Option<OcrEngine>, &'static str) {
    let bundle = rust_model_manager::screen_ocr_bundle();
    let executable = env::current_exe().ok();
    let candidates = ocr_bundle_candidates(
        &bundle.id,
        env::var_os("SAYFLEX_OCR_BUNDLE").map(PathBuf::from),
        executable.as_deref(),
        &rust_app_core::product::data_root(),
    );
    let Some(root) = candidates.into_iter().find(|root| {
        let Some(installer_root) = root.parent().map(PathBuf::from) else {
            return false;
        };
        let installer = rust_model_manager::BundleInstaller::new(installer_root);
        installer.bundle_path(&bundle) == *root && installer.is_installed(&bundle).unwrap_or(false)
    }) else {
        return (None, "ppocrv5-cpu-unavailable");
    };
    let paths = PpOcrModelPaths::from_bundle_root(root);
    match PpOcrAdapter::load(&paths) {
        Ok(adapter) => (Some(OcrEngine::new(Arc::new(adapter))), "ppocrv5-cpu"),
        Err(_) => (None, "ppocrv5-cpu-unavailable"),
    }
}

#[cfg(feature = "screen-translation")]
fn ocr_bundle_candidates(
    bundle_id: &str,
    override_root: Option<PathBuf>,
    executable: Option<&std::path::Path>,
    data_root: &std::path::Path,
) -> Vec<PathBuf> {
    let mut candidates = Vec::with_capacity(3);
    if let Some(root) = override_root {
        candidates.push(root);
    }
    if let Some(application_root) = executable.and_then(std::path::Path::parent) {
        candidates.push(application_root.join("ocr-models").join(bundle_id));
    }
    candidates.push(data_root.join("ocr-models").join(bundle_id));
    candidates.dedup();
    candidates
}

#[cfg(feature = "screen-translation")]
fn install_ocr_models() -> Result<()> {
    let bundle = rust_model_manager::screen_ocr_bundle();
    let root = env::var_os("SAYFLEX_OCR_BUNDLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            rust_app_core::product::data_root()
                .join("ocr-models")
                .join(&bundle.id)
        });
    let installer_root = root
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("the OCR bundle path has no parent"))?;
    let installer = rust_model_manager::BundleInstaller::new(installer_root);
    if installer.bundle_path(&bundle) != root {
        return Err(anyhow!(
            "the OCR bundle path does not match its catalog identity"
        ));
    }
    let fetcher = rust_model_manager::HttpFetcher::new("ocr-models")?;
    let mut last_phase = None;
    let mut last_progress_bucket = u64::MAX;
    installer.ensure_installed(&bundle, &fetcher, |progress| {
        let progress_bucket = progress
            .bytes_received
            .saturating_mul(20)
            .checked_div(progress.total_bytes.max(1))
            .unwrap_or(0);
        if last_phase != Some(progress.phase) || last_progress_bucket != progress_bucket {
            eprintln!(
                "ocr_model_install phase={:?} bytes_received={} total_bytes={}",
                progress.phase, progress.bytes_received, progress.total_bytes
            );
            last_phase = Some(progress.phase);
            last_progress_bucket = progress_bucket;
        }
    })?;
    Ok(())
}

#[cfg(not(feature = "screen-translation"))]
fn configured_engine() -> (Option<OcrEngine>, &'static str) {
    let _ = PathBuf::new();
    (None, "disabled")
}

fn main() -> Result<()> {
    rust_platform_windows::configure_safe_dll_search()?;
    #[cfg(feature = "screen-translation")]
    if env::args().any(|argument| argument == "--install-models") {
        install_ocr_models()?;
        return Ok(());
    }
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
        path::{Path, PathBuf},
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use base64::Engine as _;
    use image::{Rgba, RgbaImage};
    use rust_ocr_engine::{CancellationToken, InferenceAdapter, OcrDocument, OcrError};
    use rust_protocol::{
        OcrFailureKind, OcrLanguagePolicy, OcrLine, OcrPixelFormat, OcrWorkerCommand,
        OcrWorkerEvent, PhysicalRect, RequestId,
    };

    use super::{EventWriter, WorkerRuntime, ocr_bundle_candidates};

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
                    quad: None,
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
        started: Arc<(Mutex<bool>, Condvar)>,
    }

    impl InferenceAdapter for BlockingAdapter {
        fn recognize(
            &self,
            _image: &RgbaImage,
            _language_policy: &OcrLanguagePolicy,
            cancellation: &CancellationToken,
        ) -> Result<OcrDocument, OcrError> {
            let (started, started_changed) = &*self.started;
            *started.lock().unwrap() = true;
            started_changed.notify_all();
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

    struct SerialAdapter {
        gate: Arc<(Mutex<usize>, Condvar)>,
        started: Arc<(Mutex<usize>, Condvar)>,
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    impl InferenceAdapter for SerialAdapter {
        fn recognize(
            &self,
            _image: &RgbaImage,
            _language_policy: &OcrLanguagePolicy,
            cancellation: &CancellationToken,
        ) -> Result<OcrDocument, OcrError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            let (started, changed) = &*self.started;
            *started.lock().unwrap() += 1;
            changed.notify_all();

            let (permits, released) = &*self.gate;
            let mut permits = permits.lock().unwrap();
            while *permits == 0 {
                permits = released.wait(permits).unwrap();
            }
            *permits -= 1;
            self.active.fetch_sub(1, Ordering::SeqCst);
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
    fn packaged_ocr_models_are_considered_before_per_user_models() {
        let candidates = ocr_bundle_candidates(
            "bundle-id",
            None,
            Some(Path::new(r"C:\Program Files\SayFlex\SayFlex.Ocr.exe")),
            Path::new(r"C:\Users\me\AppData\Local\SayFlex"),
        );
        assert_eq!(
            candidates,
            [
                PathBuf::from(r"C:\Program Files\SayFlex\ocr-models\bundle-id"),
                PathBuf::from(r"C:\Users\me\AppData\Local\SayFlex\ocr-models\bundle-id"),
            ]
        );
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
    fn cancellation_is_reported_only_after_active_inference_has_returned() {
        let (writer, events) = EventWriter::test_channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let started = Arc::new((Mutex::new(false), Condvar::new()));
        let engine = rust_ocr_engine::OcrEngine::new(Arc::new(BlockingAdapter {
            gate: Arc::clone(&gate),
            started: Arc::clone(&started),
        }));
        let runtime = WorkerRuntime::new(Some(engine), writer);
        let request_id = RequestId::new();
        let pixels = base64::engine::general_purpose::STANDARD.encode([1_u8, 2, 3, 255]);
        runtime.handle(request(request_id, &pixels));
        let (has_started, started_changed) = &*started;
        let guard = has_started.lock().unwrap();
        let (guard, timeout) = started_changed
            .wait_timeout_while(guard, Duration::from_secs(2), |started| !*started)
            .unwrap();
        assert!(!timeout.timed_out());
        drop(guard);
        runtime.handle(OcrWorkerCommand::Cancel { request_id });

        assert!(events.recv_timeout(Duration::from_millis(50)).is_err());
        let (open, changed) = &*gate;
        *open.lock().unwrap() = true;
        changed.notify_all();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(2)).unwrap(),
            OcrWorkerEvent::Cancelled { request_id }
        );
        runtime.shutdown();
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn recognition_is_serialized_and_only_the_newest_pending_request_is_kept() {
        let (writer, events) = EventWriter::test_channel();
        let gate = Arc::new((Mutex::new(0), Condvar::new()));
        let started = Arc::new((Mutex::new(0), Condvar::new()));
        let adapter = Arc::new(SerialAdapter {
            gate: Arc::clone(&gate),
            started: Arc::clone(&started),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        });
        let engine = rust_ocr_engine::OcrEngine::new(adapter.clone());
        let runtime = WorkerRuntime::new(Some(engine), writer);
        let pixels = base64::engine::general_purpose::STANDARD.encode([1_u8, 2, 3, 255]);
        let first = RequestId::new();
        let replaced = RequestId::new();
        let newest = RequestId::new();

        runtime.handle(request(first, &pixels));
        let (started_count, changed) = &*started;
        let guard = started_count.lock().unwrap();
        let (guard, timeout) = changed
            .wait_timeout_while(guard, Duration::from_secs(2), |count| *count < 1)
            .unwrap();
        assert!(!timeout.timed_out());
        drop(guard);

        runtime.handle(request(replaced, &pixels));
        runtime.handle(request(newest, &pixels));
        assert_eq!(
            events.recv_timeout(Duration::from_secs(2)).unwrap(),
            OcrWorkerEvent::Cancelled {
                request_id: replaced
            }
        );

        let (permits, released) = &*gate;
        *permits.lock().unwrap() += 1;
        released.notify_all();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(2)).unwrap(),
            OcrWorkerEvent::Recognized { request_id, .. } if request_id == first
        ));

        let guard = started_count.lock().unwrap();
        let (guard, timeout) = changed
            .wait_timeout_while(guard, Duration::from_secs(2), |count| *count < 2)
            .unwrap();
        assert!(!timeout.timed_out());
        drop(guard);
        *permits.lock().unwrap() += 1;
        released.notify_all();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(2)).unwrap(),
            OcrWorkerEvent::Recognized { request_id, .. } if request_id == newest
        ));
        assert_eq!(adapter.max_active.load(Ordering::SeqCst), 1);
        runtime.shutdown();
    }
}
