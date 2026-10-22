//! Optional NVIDIA setup: consent and installation are separate from activation.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use rust_model_manager::{
    CUDA_DOWNLOAD_BYTES, CUDA_LICENSE_URL, CancelToken, CudaRuntimeManager, DownloadProgress,
    HttpFetcher, InstallPhase,
};
use slint::{ComponentHandle, Timer, TimerMode};

use crate::{
    AppState, SettingsWindow,
    localization::{UiText, active_locale, render, text},
    speech_backend::SpeechBackend,
};
use rust_app_core::product::data_root;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupPhase {
    Checking,
    Available,
    Downloading,
    Verifying,
    Installing,
    Cancelling,
    Installed,
    Cancelled,
    Failed,
}

#[derive(Debug)]
struct SetupState {
    phase: SetupPhase,
    received: u64,
    total: u64,
    error: String,
    gpu: Option<Result<Option<String>, String>>,
}

impl Default for SetupState {
    fn default() -> Self {
        Self {
            phase: SetupPhase::Checking,
            received: 0,
            total: 0,
            error: String::new(),
            gpu: None,
        }
    }
}

impl SetupState {
    fn probe_finished(&mut self, installed: bool) {
        if self.phase == SetupPhase::Checking {
            self.phase = if installed {
                SetupPhase::Installed
            } else {
                SetupPhase::Available
            };
        }
    }

    fn can_download(&self) -> bool {
        matches!(
            self.phase,
            SetupPhase::Available | SetupPhase::Cancelled | SetupPhase::Failed
        )
    }

    fn consent_to_download(&mut self) -> bool {
        if !self.can_download() {
            return false;
        }
        self.phase = SetupPhase::Downloading;
        self.received = 0;
        self.total = 0;
        self.error.clear();
        true
    }

    fn can_cancel(&self) -> bool {
        matches!(
            self.phase,
            SetupPhase::Downloading | SetupPhase::Verifying | SetupPhase::Installing
        )
    }

    fn cancel(&mut self) -> bool {
        if !self.can_cancel() {
            return false;
        }
        self.phase = SetupPhase::Cancelling;
        true
    }

    fn finished(&mut self, result: Result<(), String>) {
        match result {
            // A completed atomic install wins a last-moment cancellation.
            Ok(()) => {
                self.phase = SetupPhase::Installed;
                self.error.clear();
            }
            Err(_) if self.phase == SetupPhase::Cancelling => {
                self.phase = SetupPhase::Cancelled;
                self.error.clear();
            }
            Err(error) => {
                self.phase = SetupPhase::Failed;
                self.error = error;
            }
        }
    }

    fn can_activate(&self, cuda_selected: bool) -> bool {
        self.phase == SetupPhase::Installed && !cuda_selected
    }
}

fn cuda_is_active(cuda_backend: bool, speech_ready: bool) -> bool {
    cuda_backend && speech_ready
}

enum SetupEvent {
    Probed {
        installed: bool,
        gpu: Result<Option<String>, String>,
    },
    Progress(DownloadProgress),
    Finished(Result<(), String>),
}

/// Keep this alive alongside the settings window. All state and Slint access
/// stays on the UI thread; the worker owns only paths, a fetcher and a sender.
pub(crate) struct CudaSetupController {
    _timer: Timer,
    cancellation: Rc<RefCell<Option<CancelToken>>>,
}

impl Drop for CudaSetupController {
    fn drop(&mut self) {
        if let Some(token) = self.cancellation.borrow().as_ref() {
            token.cancel();
        }
    }
}

pub(crate) fn wire_cuda_setup(
    settings: &SettingsWindow,
    app: Rc<RefCell<AppState>>,
    activate: impl Fn() -> Result<(), String> + 'static,
) -> CudaSetupController {
    let setup = Rc::new(RefCell::new(SetupState::default()));
    let changed = Rc::new(Cell::new(false));
    let cancellation = Rc::new(RefCell::new(None::<CancelToken>));
    let (sender, receiver) = mpsc::channel();
    let root = data_root().join("runtimes");

    // Installed means both DLLs passed integrity checking, which must never
    // hash hundreds of megabytes on the UI thread.
    let probe_sender = sender.clone();
    let probe_root = root.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("cuda-probe".into())
        .spawn(move || {
            let installed = CudaRuntimeManager::new(probe_root).is_installed();
            let gpu = rust_platform_windows::nvidia_adapter_name();
            let _ = probe_sender.send(SetupEvent::Probed { installed, gpu });
        })
    {
        setup.borrow_mut().finished(Err(error.to_string()));
    }

    {
        let setup = Rc::clone(&setup);
        let cancellation = Rc::clone(&cancellation);
        let sender = sender.clone();
        let changed = Rc::clone(&changed);
        settings.on_download_cuda_runtime(move || {
            if !setup.borrow_mut().consent_to_download() {
                return;
            }
            changed.set(true);
            // Constructing the client does no network I/O. Acquire its exact
            // token before spawning, so immediate Cancel/Drop cannot miss it.
            let fetcher = match HttpFetcher::https_only("cuda-runtime") {
                Ok(fetcher) => fetcher,
                Err(error) => {
                    setup.borrow_mut().finished(Err(error.to_string()));
                    return;
                }
            };
            let token = fetcher.cancellation();
            *cancellation.borrow_mut() = Some(token.clone());
            let sender = sender.clone();
            let root = root.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("cuda-download".into())
                .spawn(move || {
                    let mut last_phase = None;
                    let mut last_sent = Instant::now();
                    let result = CudaRuntimeManager::new(root)
                        .ensure_installed(&fetcher, &token, |progress| {
                            // HTTP can deliver thousands of tiny chunks per
                            // second. Keep the UI queue bounded in practice.
                            if last_phase != Some(progress.phase)
                                || last_sent.elapsed() >= Duration::from_millis(100)
                                || progress.bytes_received == progress.total_bytes
                            {
                                last_phase = Some(progress.phase);
                                last_sent = Instant::now();
                                let _ = sender.send(SetupEvent::Progress(progress));
                            }
                        })
                        .map(|_| ())
                        .map_err(|error| error.to_string());
                    let _ = sender.send(SetupEvent::Finished(result));
                })
            {
                cancellation.borrow_mut().take();
                setup.borrow_mut().finished(Err(error.to_string()));
            }
        });
    }
    {
        let setup = Rc::clone(&setup);
        let cancellation = Rc::clone(&cancellation);
        let changed = Rc::clone(&changed);
        settings.on_cancel_cuda_runtime(move || {
            if setup.borrow_mut().cancel()
                && let Some(token) = cancellation.borrow().as_ref()
            {
                token.cancel();
                changed.set(true);
            }
        });
    }
    {
        let setup = Rc::clone(&setup);
        let app = Rc::clone(&app);
        let changed = Rc::clone(&changed);
        settings.on_activate_cuda_runtime(move || {
            let selected = app.borrow().speech_backend_active == Some(SpeechBackend::Cuda);
            if !setup.borrow().can_activate(selected) {
                return;
            }
            // The supplied integration checks recording/download activity
            // before changing workers. Installation itself never switches one.
            setup.borrow_mut().error = activate().err().unwrap_or_default();
            changed.set(true);
        });
    }
    {
        let setup = Rc::clone(&setup);
        let changed = Rc::clone(&changed);
        settings.on_open_cuda_license(move || {
            #[cfg(windows)]
            if let Err(error) = rust_platform_windows::open_https_url(CUDA_LICENSE_URL) {
                setup.borrow_mut().error = error.to_string();
                changed.set(true);
            }
        });
    }

    refresh_ui(settings, &app.borrow(), &setup.borrow());
    let timer = Timer::default();
    let weak = settings.as_weak();
    let timer_cancellation = Rc::clone(&cancellation);
    let mut last_runtime = {
        let app = app.borrow();
        (app.speech_backend_active, app.speech_ready, app.ui_locale)
    };
    timer.start(TimerMode::Repeated, Duration::from_millis(150), move || {
        // Bound draining to keep even a burst of progress events responsive.
        for event in receiver.try_iter().take(128) {
            changed.set(true);
            let mut setup = setup.borrow_mut();
            match event {
                SetupEvent::Probed { installed, gpu } => {
                    setup.gpu = Some(gpu);
                    setup.probe_finished(installed);
                }
                SetupEvent::Progress(progress) if setup.can_cancel() => {
                    setup.received = progress.bytes_received;
                    setup.total = progress.total_bytes;
                    setup.phase = match progress.phase {
                        InstallPhase::Downloading => SetupPhase::Downloading,
                        InstallPhase::Verifying => SetupPhase::Verifying,
                        InstallPhase::Installing | InstallPhase::Ready => SetupPhase::Installing,
                    };
                }
                SetupEvent::Progress(_) => {}
                SetupEvent::Finished(result) => {
                    timer_cancellation.borrow_mut().take();
                    setup.finished(result);
                }
            }
        }
        let app = app.borrow();
        let runtime = (app.speech_backend_active, app.speech_ready, app.ui_locale);
        if changed.replace(false) || runtime != last_runtime {
            last_runtime = runtime;
            if let Some(settings) = weak.upgrade() {
                refresh_ui(&settings, &app, &setup.borrow());
            }
        }
    });
    CudaSetupController {
        _timer: timer,
        cancellation,
    }
}

fn refresh_ui(settings: &SettingsWindow, app: &AppState, setup: &SetupState) {
    let locale = active_locale(app.ui_locale);
    let active = cuda_is_active(
        app.speech_backend_active == Some(SpeechBackend::Cuda),
        app.speech_ready,
    );
    let total = if setup.total == 0 {
        CUDA_DOWNLOAD_BYTES
    } else {
        setup.total
    };
    let message = if active && setup.phase == SetupPhase::Installed {
        UiText::CudaActive
    } else if app.speech_backend_active == Some(SpeechBackend::Cuda)
        && setup.phase == SetupPhase::Installed
    {
        UiText::CudaWaitingForModel
    } else {
        match setup.phase {
            SetupPhase::Checking => UiText::CudaCheckingInstallation,
            SetupPhase::Available => UiText::CudaNotInstalled,
            SetupPhase::Downloading => UiText::CudaDownloading,
            SetupPhase::Verifying => UiText::CudaVerifying,
            SetupPhase::Installing => UiText::CudaInstalling,
            SetupPhase::Cancelling => UiText::CudaCancelling,
            SetupPhase::Installed => UiText::CudaInstalled,
            SetupPhase::Cancelled => UiText::CudaCancelled,
            SetupPhase::Failed => UiText::CudaFailed,
        }
    };
    settings.set_cuda_setup_status(
        render(
            locale,
            message,
            &[
                (
                    "received",
                    &format!("{:.0}", setup.received as f64 / 1_000_000.0),
                ),
                ("total", &format!("{:.0}", total as f64 / 1_000_000.0)),
            ],
        )
        .into(),
    );
    let mut description = render(
        locale,
        UiText::CudaDownloadSize,
        &[(
            "size",
            &format!("{:.0}", CUDA_DOWNLOAD_BYTES as f64 / 1_000_000.0),
        )],
    );
    if let Some(gpu) = &setup.gpu {
        description.push('\n');
        description.push_str(&match gpu {
            Ok(Some(name)) => render(locale, UiText::CudaGpuDetected, &[("gpu", name)]),
            Ok(None) => text(locale, UiText::CudaNoGpuDetected).to_owned(),
            Err(_) => text(locale, UiText::CudaGpuUnknown).to_owned(),
        });
    }
    settings.set_cuda_setup_description(description.into());
    settings.set_cuda_setup_error(
        if setup.error.is_empty() {
            String::new()
        } else {
            render(locale, UiText::CudaActionFailed, &[("error", &setup.error)])
        }
        .into(),
    );
    settings.set_cuda_download_label(
        text(
            locale,
            if matches!(setup.phase, SetupPhase::Failed | SetupPhase::Cancelled) {
                UiText::CudaRetryDownload
            } else {
                UiText::CudaAcceptDownload
            },
        )
        .into(),
    );
    settings.set_cuda_setup_progress(
        (setup.received as f64 / total.max(1) as f64).clamp(0.0, 1.0) as f32,
    );
    settings.set_cuda_setup_busy(setup.can_cancel() || setup.phase == SetupPhase::Cancelling);
    settings.set_cuda_can_download(setup.can_download());
    settings.set_cuda_can_cancel(setup.can_cancel());
    settings.set_cuda_can_activate(
        setup.can_activate(app.speech_backend_active == Some(SpeechBackend::Cuda)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use i_slint_backend_testing::ElementHandle;

    fn init_render_backend() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("software".into()),
            },
        )))
        .unwrap();
    }

    #[test]
    fn dropping_the_controller_cancels_its_in_flight_download() {
        i_slint_backend_testing::init_no_event_loop();
        let token = CancelToken::default();
        let controller = CudaSetupController {
            _timer: Timer::default(),
            cancellation: Rc::new(RefCell::new(Some(token.clone()))),
        };
        assert!(!token.is_cancelled());
        drop(controller);
        assert!(token.is_cancelled());
    }

    #[test]
    fn nvidia_card_exposes_license_and_requires_the_explicit_download_action() {
        init_render_backend();
        let settings = SettingsWindow::new().unwrap();
        settings.set_active_page(1);
        settings.set_cuda_can_download(true);
        settings.set_cuda_download_label("Accept and download".into());
        let consent_count = Rc::new(Cell::new(0));
        let counted = Rc::clone(&consent_count);
        settings.on_download_cuda_runtime(move || counted.set(counted.get() + 1));
        settings.show().unwrap();
        settings.window().take_snapshot().unwrap();

        assert_eq!(
            consent_count.get(),
            0,
            "opening settings must not grant consent"
        );
        assert!(
            ElementHandle::find_by_accessible_label(&settings, "NVIDIA license")
                .next()
                .is_some()
        );
        assert!(
            ElementHandle::find_by_accessible_label(&settings, "Activate NVIDIA")
                .next()
                .is_none()
        );
        let button = ElementHandle::find_by_accessible_label(&settings, "Accept and download")
            .next()
            .unwrap();
        button.invoke_accessible_default_action();
        i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
        assert_eq!(consent_count.get(), 1);
        settings.hide().unwrap();
    }

    #[test]
    fn russian_nvidia_consent_fits_the_default_settings_window() {
        init_render_backend();
        let seed = SettingsWindow::new().unwrap();
        slint::select_bundled_translation("ru").unwrap();
        let settings = SettingsWindow::new().unwrap();
        settings.set_active_page(1);
        settings.set_cuda_can_download(true);
        settings.set_cuda_download_label("Принять и скачать".into());
        settings.set_cuda_setup_status("Компоненты NVIDIA не установлены.".into());
        settings.set_cuda_setup_description("Дополнительная загрузка: 394 МБ с сайта NVIDIA.\nОбнаружена видеокарта: NVIDIA GeForce RTX 3080 Ti. Совместимость проверяется при активации.".into());
        settings.show().unwrap();
        settings
            .window()
            .set_size(slint::LogicalSize::new(1000.0, 720.0));
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(500));
        let shot = settings.window().take_snapshot().unwrap();
        if let Ok(path) = std::env::var("SAYFLEX_CUDA_UI_SNAPSHOT") {
            let mut encoder = png::Encoder::new(
                std::io::BufWriter::new(std::fs::File::create(path).unwrap()),
                shot.width(),
                shot.height(),
            );
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(shot.as_bytes())
                .unwrap();
        }
        for label in ["Принять и скачать", "Лицензия NVIDIA"] {
            let action = ElementHandle::find_by_accessible_label(&settings, label)
                .next()
                .unwrap();
            let at = action.absolute_position();
            let size = action.size();
            assert!(
                at.x >= 0.0 && at.x + size.width <= 1000.0,
                "{label}: horizontal clipping at {at:?}, {size:?}"
            );
            assert!(
                at.y >= 74.0 && at.y + size.height < 630.0,
                "{label}: below default workspace at {at:?}, {size:?}"
            );
        }
        settings.hide().unwrap();
        drop(seed);
    }

    #[test]
    fn checking_and_discovery_do_not_start_a_download_without_consent() {
        let mut state = SetupState::default();
        assert_eq!(state.phase, SetupPhase::Checking);
        assert!(!state.consent_to_download());
        state.probe_finished(false);
        assert_eq!(state.phase, SetupPhase::Available);
        assert!(state.consent_to_download());
        assert_eq!(state.phase, SetupPhase::Downloading);
        assert!(!state.consent_to_download());
        state.probe_finished(true);
        assert_eq!(state.phase, SetupPhase::Downloading);
    }

    #[test]
    fn cancellation_waits_for_worker_and_allows_an_explicit_retry() {
        let mut state = SetupState::default();
        state.probe_finished(false);
        assert!(state.consent_to_download());
        assert!(state.cancel());
        assert_eq!(state.phase, SetupPhase::Cancelling);
        assert!(!state.consent_to_download());
        assert!(!state.can_activate(false));
        state.finished(Err("cancelled".into()));
        assert_eq!(state.phase, SetupPhase::Cancelled);
        assert!(state.error.is_empty());
        assert!(state.consent_to_download());
    }

    #[test]
    fn installation_does_not_claim_a_ready_cuda_worker() {
        let mut state = SetupState::default();
        state.probe_finished(false);
        assert!(state.consent_to_download());
        state.finished(Ok(()));
        assert_eq!(state.phase, SetupPhase::Installed);
        assert!(state.can_activate(false));
        assert!(!cuda_is_active(true, false));
        assert!(!cuda_is_active(false, true));
        assert!(cuda_is_active(true, true));
        assert!(!state.can_activate(true));
    }

    #[test]
    fn a_failed_download_can_be_retried_without_reporting_installed() {
        let mut state = SetupState::default();
        state.probe_finished(false);
        assert!(state.consent_to_download());
        state.finished(Err("offline".into()));
        assert_eq!(state.phase, SetupPhase::Failed);
        assert_eq!(state.error, "offline");
        assert!(!state.can_activate(false));
        assert!(state.consent_to_download());
        assert!(state.error.is_empty());
    }
}
