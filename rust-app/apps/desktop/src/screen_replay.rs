//! Explicit inspect-only replay of a recorded crop through the production
//! OCR and translation pipeline. The native selection hook is not bypassed in
//! ordinary builds or requests.

use std::{fs::File, io::BufReader, path::PathBuf, time::Instant};

use rust_platform_windows::screen_capture::{CapturedOutput, DesktopCapture, Rotation};
use rust_protocol::{PhysicalRect, RequestId, TranslationFailureKind};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReplayStep {
    Normal,
    SelectionTimeout,
    WorkerFailure,
    CancelAfterDispatch,
}

pub(crate) const STEPS: [ReplayStep; 9] = [
    ReplayStep::Normal,
    ReplayStep::Normal,
    ReplayStep::Normal,
    ReplayStep::SelectionTimeout,
    ReplayStep::Normal,
    ReplayStep::WorkerFailure,
    ReplayStep::Normal,
    ReplayStep::CancelAfterDispatch,
    ReplayStep::Normal,
];
const NORMAL_REPEAT_STEPS: [ReplayStep; 2] = [ReplayStep::Normal, ReplayStep::Normal];

#[derive(Clone, Copy, Debug)]
pub(crate) enum ReplayPhase {
    Ready,
    Awaiting(Instant),
    RestoredAfterTimeout(Instant),
    HoldingResult { since: Instant, handle: u64 },
    Closing(Instant),
    CancelClosing(Instant),
}

pub(crate) struct ScreenReplayProbe {
    pub(crate) capture: DesktopCapture,
    pub(crate) rect: PhysicalRect,
    pub(crate) restored_request_id: Option<RequestId>,
    pub(crate) output: PathBuf,
    pub(crate) started: Instant,
    pub(crate) index: usize,
    pub(crate) normal_repeat_only: bool,
    pub(crate) request_id: Option<RequestId>,
    pub(crate) phase: ReplayPhase,
    pub(crate) completed: bool,
    pub(crate) translation_dispatched: bool,
    pub(crate) cancel_ack: bool,
    pub(crate) failure_kind: Option<TranslationFailureKind>,
    pub(crate) loaded_model_id: Option<String>,
    pub(crate) cancelled_request_ids: Vec<RequestId>,
    pub(crate) failure: Option<String>,
    pub(crate) lines: Vec<String>,
    pub(crate) final_result: Option<Result<(), String>>,
}

#[derive(Deserialize)]
struct CropMetadata {
    request_id: RequestId,
    crop_x: i32,
    crop_y: i32,
    width: u32,
    height: u32,
}

impl ScreenReplayProbe {
    pub(crate) fn load_from_environment() -> Result<Self, String> {
        let png = std::env::var_os("SAYFLEX_SCREEN_REPLAY_PNG")
            .map(PathBuf::from)
            .ok_or("SAYFLEX_SCREEN_REPLAY_PNG is required")?;
        let metadata = std::env::var_os("SAYFLEX_SCREEN_REPLAY_METADATA")
            .map(PathBuf::from)
            .ok_or("SAYFLEX_SCREEN_REPLAY_METADATA is required")?;
        let output = std::env::var_os("SAYFLEX_SCREEN_REPLAY_OUTPUT")
            .map(PathBuf::from)
            .ok_or("SAYFLEX_SCREEN_REPLAY_OUTPUT is required")?;
        std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;

        let metadata: CropMetadata =
            serde_json::from_reader(File::open(&metadata).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        let decoder = png::Decoder::new(BufReader::new(
            File::open(&png).map_err(|error| error.to_string())?,
        ));
        let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
        let mut pixels = vec![0; reader.output_buffer_size().ok_or("PNG buffer too large")?];
        let info = reader
            .next_frame(&mut pixels)
            .map_err(|error| error.to_string())?;
        if info.color_type != png::ColorType::Rgba
            || info.bit_depth != png::BitDepth::Eight
            || info.width != metadata.width
            || info.height != metadata.height
        {
            return Err("replay PNG must be RGBA8 and match its recorded crop metadata".into());
        }
        pixels.truncate(info.buffer_size());
        let rect = PhysicalRect {
            x: metadata.crop_x,
            y: metadata.crop_y,
            width: metadata.width,
            height: metadata.height,
        };
        let stride = rect
            .width
            .checked_mul(4)
            .ok_or("replay crop stride overflow")?;
        let output_frame = CapturedOutput::try_new(rect, Rotation::Identity, 1_000, stride, pixels)
            .map_err(|error| error.to_string())?;
        let capture =
            DesktopCapture::try_new(1, vec![output_frame]).map_err(|error| error.to_string())?;
        Ok(Self {
            capture,
            rect,
            restored_request_id: None,
            output,
            started: Instant::now(),
            index: 0,
            normal_repeat_only: std::env::var_os("SAYFLEX_SCREEN_REPLAY_NORMAL_ONLY").as_deref()
                == Some(std::ffi::OsStr::new("1")),
            request_id: None,
            phase: ReplayPhase::Ready,
            completed: false,
            translation_dispatched: false,
            cancel_ack: false,
            failure_kind: None,
            loaded_model_id: None,
            cancelled_request_ids: Vec::new(),
            failure: None,
            lines: vec![format!(
                "inspect replay recorded_request={} crop={}x{} at {},{}; native selection and DXGI capture not exercised",
                metadata.request_id, rect.width, rect.height, rect.x, rect.y
            )],
            final_result: None,
        })
    }

    pub(crate) fn step(&self) -> Option<ReplayStep> {
        if self.normal_repeat_only {
            NORMAL_REPEAT_STEPS.get(self.index).copied()
        } else {
            STEPS.get(self.index).copied()
        }
    }

    pub(crate) fn record(&mut self, message: impl Into<String>) {
        let message = message.into();
        rust_app_core::diagnostics::log_line(&format!("screen_replay {message}"));
        self.lines.push(message);
    }

    pub(crate) fn begin_cancel(&mut self, request_id: RequestId) {
        if !self.cancelled_request_ids.contains(&request_id) {
            self.cancelled_request_ids.push(request_id);
        }
    }

    pub(crate) fn observe_raw_screen_completion(&mut self, request_id: RequestId) {
        if self.cancelled_request_ids.contains(&request_id) {
            self.finish(Err(format!(
                "cancelled request={request_id} received late ScreenBlocksCompleted"
            )));
        }
    }

    pub(crate) fn finish(&mut self, result: Result<(), String>) {
        if self.final_result.is_some() {
            return;
        }
        self.record(format!("finished result={result:?}"));
        let receipt = self.output.join("replay-receipt.txt");
        if let Err(error) = std::fs::write(&receipt, self.lines.join("\n") + "\n") {
            self.final_result = Some(Err(format!("could not write replay receipt: {error}")));
        } else {
            self.final_result = Some(result);
        }
        let _ = slint::quit_event_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_completion_between_cancel_dispatch_and_ack_fails_probe() {
        let request_id = RequestId::new();
        let bounds = PhysicalRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let capture = DesktopCapture::try_new(
            1,
            vec![
                CapturedOutput::try_new(bounds, Rotation::Identity, 1_000, 4, vec![0, 0, 0, 255])
                    .unwrap(),
            ],
        )
        .unwrap();
        let output = std::env::temp_dir().join(format!("sayflex-cancel-race-{request_id}"));
        std::fs::create_dir(&output).unwrap();
        let mut probe = ScreenReplayProbe {
            capture,
            rect: bounds,
            restored_request_id: None,
            output: output.clone(),
            started: Instant::now(),
            index: 7,
            normal_repeat_only: false,
            request_id: Some(request_id),
            phase: ReplayPhase::CancelClosing(Instant::now()),
            completed: false,
            translation_dispatched: true,
            cancel_ack: false,
            failure_kind: None,
            loaded_model_id: None,
            cancelled_request_ids: Vec::new(),
            failure: None,
            lines: Vec::new(),
            final_result: None,
        };
        probe.begin_cancel(request_id);
        probe.observe_raw_screen_completion(request_id);
        assert!(matches!(probe.final_result, Some(Err(_))));
        assert!(!probe.cancel_ack);
        std::fs::remove_file(output.join("replay-receipt.txt")).unwrap();
        std::fs::remove_dir(output).unwrap();
    }
}
