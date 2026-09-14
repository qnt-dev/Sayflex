//! Isolated preview modes for looking at one window at a time.
//!
//! Each of these starts the application as a single transient window with
//! rehearsed input, registers no global hook and touches no other application,
//! so a visual change can be judged without dictating into a real document.
//! They exist for development and are reachable only from the command line.

use std::{
    cell::{Cell, RefCell},
    env,
    error::Error,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rust_app_core::worker_stream::JsonWriter;
use rust_protocol::{AUDIO_SPECTRUM_BANDS, SessionId, SpeechWorkerCommand, SpeechWorkerEvent};
use slint::{
    ComponentHandle, Image, ModelRc, PhysicalPosition, Rgba8Pixel, SharedPixelBuffer, SharedString,
    Timer, TimerMode, VecModel, winit_030::WinitWindowAccessor,
};

use crate::{
    ActionPalette, SelectionChooser, TranslationPair, TranslationPreview,
    adaptive_text_panel_height, apply_translation_preview_layout,
    configure_indicator_native_window, flow_sphere, native_window_handle,
    overlay_presenter::{OverlayContract, OverlayPresenter},
    precreate_indicator, refresh_cloaked_translation_backdrop,
    settings_document::data_root,
    show_indicator, slint_window_handle,
    speech_backend::{SpeechBackend, SpeechBackendPreference},
    start_indicator_animation, unix_ms,
    visualizer::{
        CircularSpectrumPalette, IndicatorMode, VisualizerStyle, WritingSlateSurface,
        indicator_preview_pulse_matrix_palette_for_argument,
        indicator_preview_slate_surface_for_argument, indicator_preview_style_for_argument,
        new_indicator,
    },
    wire_translation_preview,
    worker_supervisor::{
        CREATE_NO_WINDOW, speech_backend_plan, speech_worker_path, trusted_cuda_runtime_directory,
    },
};

pub(crate) fn indicator_preview_style() -> Option<VisualizerStyle> {
    env::args().find_map(|argument| indicator_preview_style_for_argument(&argument))
}

pub(crate) fn indicator_presentation_probe_cycles() -> Option<u32> {
    env::args().find_map(|argument| {
        if argument == "--indicator-presentation-probe" {
            return Some(30);
        }
        argument
            .strip_prefix("--indicator-presentation-probe=")
            .and_then(|cycles| cycles.parse::<u32>().ok())
            .map(|cycles| cycles.clamp(1, 200))
    })
}

pub(crate) fn translation_presentation_probe_cycles() -> Option<u32> {
    env::args().find_map(|argument| {
        if argument == "--translation-presentation-probe" {
            return Some(30);
        }
        argument
            .strip_prefix("--translation-presentation-probe=")
            .and_then(|cycles| cycles.parse::<u32>().ok())
            .map(|cycles| cycles.clamp(1, 200))
    })
}

pub(crate) fn indicator_preview_slate_surface() -> WritingSlateSurface {
    env::args()
        .find_map(|argument| indicator_preview_slate_surface_for_argument(&argument))
        .unwrap_or_default()
}

pub(crate) fn indicator_preview_palette() -> CircularSpectrumPalette {
    env::args()
        .find_map(|argument| match argument.as_str() {
            "--indicator-palette=sunset" => Some(CircularSpectrumPalette::Sunset),
            "--indicator-palette=arctic" | "--indicator-palette=blue" => {
                Some(CircularSpectrumPalette::Arctic)
            }
            "--indicator-palette=monochrome" | "--indicator-palette=white" => {
                Some(CircularSpectrumPalette::Monochrome)
            }
            _ => None,
        })
        .unwrap_or_default()
}

pub(crate) fn indicator_preview_flow_sphere_palette() -> flow_sphere::FlowSpherePalette {
    env::args()
        .find_map(|argument| match argument.as_str() {
            "--flow-sphere-palette=reference" | "--flow-sphere-palette=original" => {
                Some(flow_sphere::FlowSpherePalette::Reference)
            }
            "--flow-sphere-palette=black-yellow" | "--flow-sphere-palette=yellow" => {
                Some(flow_sphere::FlowSpherePalette::BlackYellow)
            }
            _ => None,
        })
        .unwrap_or_default()
}

pub(crate) fn indicator_preview_pulse_matrix_palette() -> crate::pulse_matrix::PulseMatrixPalette {
    env::args()
        .find_map(|argument| indicator_preview_pulse_matrix_palette_for_argument(&argument))
        .unwrap_or_default()
}

pub(crate) fn translation_preview_mode() -> Option<bool> {
    env::args().find_map(|argument| match argument.as_str() {
        "--translation-preview" => Some(false),
        "--translation-preview=long" => Some(true),
        _ => None,
    })
}

pub(crate) fn run_indicator_preview(
    style: VisualizerStyle,
    live_microphone: bool,
    writing_slate_surface: WritingSlateSurface,
) -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.IndicatorPreview")?;
    let (indicator, animation) = new_indicator()?;
    {
        let mut animation = animation.borrow_mut();
        animation.set_style(style);
        animation.set_flow_sphere_palette(indicator_preview_flow_sphere_palette());
        animation.set_pulse_matrix_palette(indicator_preview_pulse_matrix_palette());
        animation.begin_listening();
    }
    indicator.set_visualizer_style(style.index());
    indicator.set_indicator_title("SayFlex Recording Indicator".into());
    indicator.set_writing_slate_surface(writing_slate_surface.index());
    let blue_glass = writing_slate_surface == WritingSlateSurface::BlueGlass;
    indicator.set_circular_palette(indicator_preview_palette().index());
    indicator.set_prepared(true);
    indicator.show()?;
    let indicator_weak = indicator.as_weak();
    slint::spawn_local(async move {
        if let Some(indicator) = indicator_weak.upgrade() {
            let handle = indicator
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle);
            let configured = handle.is_some_and(|handle| {
                if blue_glass {
                    rust_platform_windows::configure_blue_glass_overlay_window(handle, None).is_ok()
                } else {
                    configure_indicator_native_window(handle, None, style.uses_transparent_window())
                }
            });
            if !configured {
                eprintln!("could not configure recording indicator preview");
            } else if blue_glass && let Some(handle) = handle {
                match rust_platform_windows::capture_blurred_overlay_backdrop(handle, 1) {
                    Ok(backdrop)
                        if backdrop.rgba.len()
                            == backdrop.width as usize * backdrop.height as usize * 4 =>
                    {
                        let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                            &backdrop.rgba,
                            backdrop.width,
                            backdrop.height,
                        );
                        indicator.set_writing_slate_glass_backdrop(Image::from_rgba8(pixels));
                    }
                    Ok(_) => eprintln!("captured recording-indicator backdrop has invalid length"),
                    Err(error) => {
                        eprintln!("could not capture recording-indicator backdrop: {error}")
                    }
                }
            }
        }
    })?;

    let live_signal = Arc::new(Mutex::new(None::<(f32, Vec<f32>)>));
    let live_audio_worker = if live_microphone {
        let worker_job = rust_platform_windows::WorkerJob::new()
            .map_err(|error| format!("could not create preview worker job: {error}"))?;
        let backend = speech_backend_plan(SpeechBackendPreference::Auto)?.active();
        let path = speech_worker_path(backend)?;
        let mut command = Command::new(&path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if backend == SpeechBackend::Cuda {
            let runtime = trusted_cuda_runtime_directory()
                .ok_or("the CUDA preview worker is missing its verified runtime")?;
            command.env("PATH", runtime);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        worker_job
            .assign(&child)
            .map_err(|error| format!("could not contain preview audio worker: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or("preview audio worker stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("preview audio worker stdout is unavailable")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("preview audio worker stderr is unavailable")?;
        let writer = JsonWriter::start(stdin, "preview-audio-worker")?;
        let session_id = SessionId::new();
        writer.send(&SpeechWorkerCommand::SetFastReleasePreviews { enabled: false })?;
        writer.send(&SpeechWorkerCommand::Begin {
            session_id,
            started_unix_ms: unix_ms(),
        })?;

        let signal_for_reader = Arc::clone(&live_signal);
        std::thread::Builder::new()
            .name("indicator-preview-audio".to_owned())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let Ok(event) = serde_json::from_str::<SpeechWorkerEvent>(&line) else {
                        continue;
                    };
                    if let SpeechWorkerEvent::AudioLevel {
                        session_id: event_session,
                        peak,
                        spectrum,
                    } = event
                        && event_session == session_id
                        && let Ok(mut latest) = signal_for_reader.lock()
                    {
                        *latest = Some((peak, spectrum));
                    }
                }
            })?;
        std::thread::Builder::new()
            .name("indicator-preview-audio-diagnostics".to_owned())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    eprintln!("preview audio worker: {line}");
                }
            })?;
        Some((worker_job, child, writer, session_id))
    } else {
        None
    };

    let timer = Timer::default();
    let indicator = indicator.as_weak();
    let animation_for_timer = Rc::clone(&animation);
    let started = Instant::now();
    let segment = Rc::new(Cell::new(IndicatorMode::Listening));
    let segment_for_timer = Rc::clone(&segment);
    let live_signal_for_timer = Arc::clone(&live_signal);
    timer.start(TimerMode::Repeated, Duration::from_millis(12), move || {
        let elapsed = started.elapsed().as_secs_f32();
        let (cycle_length, listening_until, processing_until) =
            if style == VisualizerStyle::WritingSlate {
                (17.0, 14.0, 16.0)
            } else {
                (10.0, 7.0, 9.0)
            };
        let cycle_position = elapsed % cycle_length;
        let next_segment = if cycle_position < listening_until {
            IndicatorMode::Listening
        } else if cycle_position < processing_until {
            IndicatorMode::Processing
        } else {
            IndicatorMode::Success
        };
        if segment_for_timer.get() != next_segment {
            segment_for_timer.set(next_segment);
            match next_segment {
                IndicatorMode::Listening => animation_for_timer.borrow_mut().begin_listening(),
                IndicatorMode::Processing => animation_for_timer.borrow_mut().begin_processing(),
                IndicatorMode::Success => animation_for_timer.borrow_mut().begin_success(),
                IndicatorMode::Hidden => {}
            }
        }
        if next_segment == IndicatorMode::Listening {
            if live_microphone {
                if let Ok(mut latest) = live_signal_for_timer.try_lock()
                    && let Some((peak, spectrum)) = latest.take()
                {
                    animation_for_timer
                        .borrow_mut()
                        .set_microphone_signal(peak, &spectrum);
                }
            } else {
                let simulated_level = (0.08 + ((elapsed * 2.7).sin() + 1.0) * 0.18).clamp(0.0, 1.0);
                let simulated_spectrum = (0..AUDIO_SPECTRUM_BANDS)
                    .map(|index| {
                        let position = index as f32 / AUDIO_SPECTRUM_BANDS as f32;
                        let low_voice = (1.0 - (position - 0.24).abs() * 3.1).clamp(0.0, 1.0);
                        let formant = (1.0 - (position - 0.52).abs() * 5.4).clamp(0.0, 1.0);
                        let motion = ((elapsed * 3.2 + index as f32 * 0.47).sin() + 1.0) * 0.5;
                        ((low_voice * 0.72 + formant * 0.38)
                            * (0.52 + motion * 0.48)
                            * simulated_level
                            * 2.9)
                            .clamp(0.0, 1.0)
                    })
                    .collect::<Vec<_>>();
                animation_for_timer
                    .borrow_mut()
                    .set_microphone_signal(simulated_level, &simulated_spectrum);
            }
        }
        if let Some(indicator) = indicator.upgrade() {
            animation_for_timer
                .borrow_mut()
                .tick(std::slice::from_ref(&indicator));
        }
    });
    slint::run_event_loop()?;
    timer.stop();
    if let Some((_worker_job, mut child, writer, session_id)) = live_audio_worker {
        let _ = writer.send(&SpeechWorkerCommand::Cancel { session_id });
        let _ = writer.send(&SpeechWorkerCommand::Shutdown);
        drop(writer);
        let _ = child.wait();
    }
    Ok(())
}

pub(crate) fn run_indicator_presentation_probe(cycles: u32) -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.OverlayProbe")?;
    let _ = rust_app_core::diagnostics::initialize(
        &data_root().join("logs"),
        "recording-overlay-probe",
    );
    let (indicator, animation) = new_indicator()?;
    let presenter = OverlayPresenter::install_always_renderable(
        indicator.window(),
        "recording_overlay_probe",
        OverlayContract::ClickThrough,
        None,
    )?;
    indicator.set_visualizer_style(VisualizerStyle::WritingSlate.index());
    indicator.set_writing_slate_surface(WritingSlateSurface::BlueGlass.index());
    {
        let mut animation = animation.borrow_mut();
        animation.set_style(VisualizerStyle::WritingSlate);
        animation.begin_listening();
    }
    precreate_indicator(&indicator, &presenter, true);

    let animation_timer = start_indicator_animation(vec![indicator.as_weak()], &animation);
    let cycle_timer = Timer::default();
    let step = Rc::new(Cell::new(0_u32));
    let step_for_timer = Rc::clone(&step);
    let indicator_weak = indicator.as_weak();
    let presenter_for_timer = presenter.clone();
    let animation_for_timer = Rc::clone(&animation);
    cycle_timer.start(TimerMode::Repeated, Duration::from_millis(90), move || {
        let current = step_for_timer.get();
        if current >= cycles.saturating_mul(2).saturating_add(4) {
            if let Some(indicator) = indicator_weak.upgrade() {
                indicator.set_prepared(false);
                presenter_for_timer.retire(indicator.window(), "probe_complete");
            }
            let _ = slint::quit_event_loop();
            return;
        }
        step_for_timer.set(current.saturating_add(1));

        // Leave four event-loop turns for native creation and presenter
        // attachment, then exercise full frame-gated present/retire cycles.
        if current < 4 {
            return;
        }
        if current.is_multiple_of(2) {
            animation_for_timer.borrow_mut().begin_listening();
            if let Some(indicator) = indicator_weak.upgrade() {
                show_indicator(&indicator, &presenter_for_timer, true, None);
            }
        } else {
            animation_for_timer.borrow_mut().stop();
            if let Some(indicator) = indicator_weak.upgrade() {
                indicator.set_prepared(false);
                presenter_for_timer.retire(indicator.window(), "probe_cycle");
            }
        }
    });

    slint::run_event_loop()?;
    cycle_timer.stop();
    animation_timer.stop();
    let _ = rust_app_core::diagnostics::flush();
    Ok(())
}

/// Answers one question, with a real window and a real compositor: does
/// Windows draw a window that is visible but standing off the desktop, and does
/// it refuse to draw one that is cloaked?
///
/// The whole presentation contract rests on that answer and it was never
/// measured. The frame gate was written against a cloaked window and, in the
/// live application, did not get a single frame in four consecutive
/// translations — every one of them was revealed by the watchdog.
pub(crate) fn run_staging_probe() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire(r"Local\SayFlex.StagingProbe")?;
    // So the measurement survives the terminal it was printed in.
    let _ = rust_app_core::diagnostics::initialize(&data_root().join("logs"), "staging-probe");
    let preview = TranslationPreview::new()?;
    preview.set_prepared(false);
    preview
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    preview.show()?;

    let frames = Rc::new(Cell::new(0u32));
    let counted = Rc::clone(&frames);
    preview
        .window()
        .set_rendering_notifier(move |state, _| {
            if matches!(state, slint::RenderingState::AfterRendering) {
                counted.set(counted.get() + 1);
            }
        })
        .map_err(|error| format!("could not observe rendering: {error:?}"))?;

    // What went wrong, if anything did. A probe that prints a number it could
    // not measure is worse than one that fails.
    let trouble: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let step = Rc::new(Cell::new(0u32));
    let staged_before = Rc::new(Cell::new(0u32));
    let staged = Rc::new(Cell::new(0u32));
    let cloaked_before = Rc::new(Cell::new(0u32));
    let handle = Rc::new(Cell::new(0u64));

    let timer = slint::Timer::default();
    let preview_weak = preview.as_weak();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), {
        let trouble = Rc::clone(&trouble);
        let frames = Rc::clone(&frames);
        let step = Rc::clone(&step);
        let staged_before = Rc::clone(&staged_before);
        let staged = Rc::clone(&staged);
        let cloaked_before = Rc::clone(&cloaked_before);
        let handle = Rc::clone(&handle);
        move || {
            let Some(preview) = preview_weak.upgrade() else {
                return;
            };
            let now = step.get();
            step.set(now + 1);
            match now {
                0 => {
                    let Some(hwnd) = crate::slint_window_handle(preview.window()) else {
                        fail(&trouble, "the window has no native handle");
                        return;
                    };
                    handle.set(hwnd);
                    // This applies the overlay contract *and cloaks the window*.
                    // The first attempt at this probe stopped here and called
                    // the next interval "not cloaked", which measured the
                    // cloaked case twice and proved nothing. Every call is
                    // checked now, because a probe that swallows its own errors
                    // reports a number it did not measure.
                    if let Err(error) = rust_platform_windows::prepare_translation_window(hwnd) {
                        fail(&trouble, &format!("prepare failed: {error}"));
                        return;
                    }
                    if let Err(error) = rust_platform_windows::reveal_overlay_window(hwnd) {
                        fail(&trouble, &format!("reveal failed: {error}"));
                    }
                }
                2 => {
                    // Off the desktop, visible, and demonstrably not cloaked.
                    match rust_platform_windows::overlay_window_is_cloaked(handle.get()) {
                        Ok(false) => {}
                        Ok(true) => {
                            fail(
                                &trouble,
                                "the window was still cloaked in the visible interval",
                            );
                            return;
                        }
                        Err(error) => {
                            fail(&trouble, &format!("cloak state unreadable: {error}"));
                            return;
                        }
                    }
                    preview.set_translated_text("staged".into());
                    preview.set_prepared(true);
                    staged_before.set(frames.get());
                    preview.window().request_redraw();
                }
                14 => {
                    staged.set(frames.get() - staged_before.get());
                    if let Err(error) = rust_platform_windows::cloak_overlay_window(handle.get()) {
                        fail(&trouble, &format!("cloak failed: {error}"));
                        return;
                    }
                    match rust_platform_windows::overlay_window_is_cloaked(handle.get()) {
                        Ok(true) => {}
                        Ok(false) => {
                            fail(
                                &trouble,
                                "the window was not cloaked in the cloaked interval",
                            );
                            return;
                        }
                        Err(error) => {
                            fail(&trouble, &format!("cloak state unreadable: {error}"));
                            return;
                        }
                    }
                    preview.set_translated_text("cloaked".into());
                    cloaked_before.set(frames.get());
                    preview.window().request_redraw();
                }
                26 => {
                    let cloaked = frames.get() - cloaked_before.get();
                    let _ = rust_platform_windows::reveal_overlay_window(handle.get());
                    let line = format!(
                        "staging_probe visible_cloaked=false visible_frames={} \
                         cloaked_cloaked=true cloaked_frames={cloaked}",
                        staged.get()
                    );
                    println!("{line}");
                    // Written where it can be read afterwards, so the result is
                    // evidence rather than something that scrolled past.
                    rust_app_core::diagnostics::log_line(&line);
                    if staged.get() == 0 || cloaked == 0 {
                        fail(
                            &trouble,
                            "a measurement of zero frames says nothing about either case",
                        );
                        return;
                    }
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        }
    });

    slint::run_event_loop()?;
    // The log is written on its own thread; without this the process exits
    // before the measurement reaches the file, which is the one place it is
    // any use afterwards.
    let _ = rust_app_core::diagnostics::flush();
    if let Some(reason) = trouble.borrow().clone() {
        return Err(format!("staging_probe failed: {reason}").into());
    }
    Ok(())
}

/// Records why the probe cannot answer, and stops it.
fn fail(trouble: &Rc<RefCell<Option<String>>>, reason: &str) {
    *trouble.borrow_mut() = Some(reason.to_owned());
    let _ = slint::quit_event_loop();
}

pub(crate) fn run_translation_presentation_probe(cycles: u32) -> Result<(), Box<dyn Error>> {
    let _single_instance = rust_platform_windows::SingleInstanceGuard::acquire(
        "Local\\SayFlex.TranslationOverlayProbe",
    )?;
    let _ = rust_app_core::diagnostics::initialize(
        &data_root().join("logs"),
        "translation-overlay-probe",
    );
    let preview = TranslationPreview::new()?;
    let presenter = OverlayPresenter::install_content_gated(
        preview.window(),
        "translation_preview_probe",
        OverlayContract::Interactive,
        None,
    )?;
    preview.set_prepared(false);
    preview
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    preview.show()?;
    {
        let preview = preview.as_weak();
        let presenter = presenter.clone();
        slint::spawn_local(async move {
            if let Some(preview) = preview.upgrade() {
                let result = preview
                    .window()
                    .winit_window()
                    .await
                    .ok()
                    .as_deref()
                    .and_then(native_window_handle)
                    .ok_or_else(|| "translation probe HWND is unavailable".to_owned())
                    .and_then(|handle| {
                        rust_platform_windows::prepare_translation_window(handle)?;
                        presenter.attach(handle)
                    });
                if let Err(error) = result {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview_probe transition=failed stage=attach error={error}"
                    ));
                }
            }
        })?;
    }

    let cycle_timer = Timer::default();
    let step = Rc::new(Cell::new(0_u32));
    let step_for_timer = Rc::clone(&step);
    let preview_weak = preview.as_weak();
    let presenter_for_timer = presenter.clone();
    cycle_timer.start(TimerMode::Repeated, Duration::from_millis(220), move || {
        let current = step_for_timer.get();
        if current >= cycles.saturating_mul(2).saturating_add(4) {
            if let Some(preview) = preview_weak.upgrade() {
                preview.set_prepared(false);
                presenter_for_timer.retire(preview.window(), "probe_complete");
            }
            let _ = slint::quit_event_loop();
            return;
        }
        step_for_timer.set(current.saturating_add(1));
        if current < 4 {
            return;
        }
        let Some(preview) = preview_weak.upgrade() else {
            return;
        };
        if current.is_multiple_of(2) {
            let text = if (current / 2).is_multiple_of(2) {
                "Frame-gated glass preview"
            } else {
                "The translation overlay is rendered completely before DWM reveals its native window."
            };
            preview.set_translated_text(text.into());
            apply_translation_preview_layout(&preview, "", text);
            preview.set_prepared(false);
            // The contract this probe exists to exercise: a presenter that has
            // not been told there is content will not accept a frame. Without
            // this the probe could never show its window, which would have made
            // it useless exactly when it was needed.
            presenter.content_changed(preview.window(), !text.is_empty());
            let ticket = match presenter_for_timer.begin_prepare() {
                Ok(ticket) => ticket,
                Err(error) => {
                    rust_app_core::diagnostics::log_line(&format!(
                        "translation_preview_probe transition=failed stage=begin_prepare \
error={error}"
                    ));
                    return;
                }
            };
            let Some(handle) = slint_window_handle(preview.window()) else {
                presenter_for_timer.abort_prepare(
                    preview.window(),
                    ticket,
                    "configure",
                    "native HWND is unavailable",
                );
                return;
            };
            if let Err(error) = rust_platform_windows::configure_translation_window(handle) {
                presenter_for_timer.abort_prepare(
                    preview.window(),
                    ticket,
                    "configure",
                    &error,
                );
                return;
            }
            refresh_cloaked_translation_backdrop(&preview, handle);
            preview.set_prepared(true);
            if let Err(error) = presenter_for_timer.await_frame(preview.window(), ticket) {
                preview.set_prepared(false);
                presenter_for_timer.abort_prepare(
                    preview.window(),
                    ticket,
                    "await_frame",
                    &error,
                );
            }
        } else {
            preview.set_prepared(false);
            presenter_for_timer.retire(preview.window(), "probe_cycle");
        }
    });

    slint::run_event_loop()?;
    cycle_timer.stop();
    let _ = rust_app_core::diagnostics::flush();
    Ok(())
}

pub(crate) fn run_action_palette_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.ActionPreview")?;
    let palette = ActionPalette::new()?;
    let source =
        "A compact command surface should fit the selected text and preserve editor focus.";
    palette.set_source_text(source.into());
    palette.set_source_panel_height(adaptive_text_panel_height(source, 70, 2, 4, 15.0, 16.0));
    palette.set_translation_enabled(true);
    palette.set_layout_enabled(true);
    palette.set_prepared(true);
    palette.on_translate(|| {
        let _ = slint::quit_event_loop();
    });
    palette.on_correct_layout(|| {
        let _ = slint::quit_event_loop();
    });
    palette.on_cancel(|| {
        let _ = slint::quit_event_loop();
    });
    palette.show()?;
    let palette_weak = palette.as_weak();
    slint::spawn_local(async move {
        if let Some(palette) = palette_weak.upgrade() {
            let handle = palette
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle);
            if let Err(error) = handle
                .ok_or_else(|| "native action window is unavailable".to_owned())
                .and_then(rust_platform_windows::configure_action_window)
            {
                eprintln!("could not configure action preview: {error}");
            }
        }
    })?;
    slint::run_event_loop()?;
    Ok(())
}

pub(crate) fn run_translation_preview(long_text: bool) -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.TranslationPreview")?;
    let preview = TranslationPreview::new()?;
    let presenter = OverlayPresenter::install_content_gated(
        preview.window(),
        "translation_preview_probe",
        OverlayContract::Interactive,
        Some(Arc::new(rust_platform_windows::activate_popup_dismiss)),
    )?;
    let translated = if long_text {
        "Длинный перевод должен оставаться удобным для чтения. Карточка сохраняет компактную форму, а результат прокручивается внутри неё. Это позволяет быстро проверить смысл, не терять выбранное место в исходном приложении и не открывать отдельное большое рабочее окно. Прокрутка появляется только тогда, когда текст действительно не помещается. ".repeat(12)
    } else {
        "Мужчина-пассажир звонит в колокольчик проводника из купе мистера Рэтчетта. Проводник подходит к двери. Пассажир отвечает по-французски."
            .to_owned()
    };
    let source = if long_text {
        "A long translation should remain easy to read. The card keeps its compact shape while the result scrolls inside it. This makes it possible to verify the meaning without losing the selected place in the source application."
    } else {
        "A male passenger rings the conductor's bell from Mr. Ratchett's compartment. The conductor comes to the door. The passenger answers in French."
    };
    let dual_pairs = if long_text {
        vec![TranslationPair {
            source: source.into(),
            translation: translated.clone().into(),
        }]
    } else {
        vec![
            TranslationPair {
                source:
                    "A male passenger rings the conductor's bell from Mr. Ratchett's compartment."
                        .into(),
                translation:
                    "Мужчина-пассажир звонит в колокольчик проводника из купе мистера Рэтчетта."
                        .into(),
            },
            TranslationPair {
                source: "The conductor comes to the door.".into(),
                translation: "Проводник подходит к двери.".into(),
            },
            TranslationPair {
                source: "The passenger answers in French.".into(),
                translation: "Пассажир отвечает по-французски.".into(),
            },
        ]
    };
    preview.set_source_text(source.into());
    preview.set_dual_pairs(ModelRc::new(VecModel::from(dual_pairs)));
    preview.set_translated_text(translated.clone().into());
    // After the content, like every other caller: the card is measured with
    // whatever is on it.
    apply_translation_preview_layout(&preview, source, &translated);
    preview.set_card_width(480.0);
    preview.set_prepared(false);
    wire_translation_preview(&preview, &presenter);
    preview
        .window()
        .set_position(PhysicalPosition::new(-32_000, -32_000));
    preview.show()?;
    let preview_weak = preview.as_weak();
    let presenter_for_window = presenter.clone();
    slint::spawn_local(async move {
        if let Some(preview) = preview_weak.upgrade() {
            let handle = preview
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle);
            let prepared = handle
                .ok_or_else(|| "native translation window is unavailable".to_owned())
                .and_then(rust_platform_windows::prepare_translation_window)
                .and_then(|()| presenter_for_window.attach(handle.unwrap_or_default()));
            if let Err(error) = prepared {
                eprintln!("could not configure translation preview: {error}");
            } else if let Some(handle) = handle {
                let ticket = match presenter_for_window.begin_prepare() {
                    Ok(ticket) => ticket,
                    Err(error) => {
                        eprintln!("could not prepare translation preview: {error}");
                        return;
                    }
                };
                if let Err(error) = rust_platform_windows::configure_translation_window(handle) {
                    presenter_for_window.abort_prepare(
                        preview.window(),
                        ticket,
                        "configure",
                        &error,
                    );
                    return;
                }
                refresh_cloaked_translation_backdrop(&preview, handle);
                preview.set_prepared(true);
                if let Err(error) = presenter_for_window.await_frame(preview.window(), ticket) {
                    preview.set_prepared(false);
                    presenter_for_window.abort_prepare(
                        preview.window(),
                        ticket,
                        "await_frame",
                        &error,
                    );
                }
            }
        }
    })?;
    // The design probe intentionally has no global click-away monitor. It must
    // remain visible while the user compares ORIG and DUAL, even after clicking
    // another window. Production keeps its normal popup lifecycle.
    slint::run_event_loop()?;
    Ok(())
}

pub(crate) fn run_layout_preview() -> Result<(), Box<dyn Error>> {
    let _single_instance =
        rust_platform_windows::SingleInstanceGuard::acquire("Local\\SayFlex.LayoutPreview")?;
    let chooser = SelectionChooser::new()?;
    let source = "ghbdtn vbh";
    chooser.set_source_text(source.into());
    chooser.set_source_panel_height(adaptive_text_panel_height(source, 82, 2, 5, 15.0, 16.0));
    chooser.set_candidate_labels(ModelRc::new(VecModel::from(vec![SharedString::from(
        "English keys → Cyrillic layout",
    )])));
    chooser.set_candidate_texts(ModelRc::new(VecModel::from(vec![SharedString::from(
        "привет мир",
    )])));
    chooser.set_prepared(true);
    chooser.on_choose_candidate(|_| {
        let _ = slint::quit_event_loop();
    });
    chooser.on_cancel(|| {
        let _ = slint::quit_event_loop();
    });
    chooser.show()?;
    let chooser_weak = chooser.as_weak();
    slint::spawn_local(async move {
        if let Some(chooser) = chooser_weak.upgrade() {
            let handle = chooser
                .window()
                .winit_window()
                .await
                .ok()
                .as_deref()
                .and_then(native_window_handle);
            if let Err(error) = handle
                .ok_or_else(|| "native selection window is unavailable".to_owned())
                .and_then(rust_platform_windows::configure_action_window)
            {
                eprintln!("could not configure layout preview: {error}");
            }
        }
    })?;
    slint::run_event_loop()?;
    Ok(())
}
