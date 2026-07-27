use std::{
    mem,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, RingBuffer};
use rust_protocol::{SessionId, SpeechWorkerEvent};

const AUDIO_RING_SECONDS: usize = 8;
const CAPTURE_RESERVE_SECONDS: usize = 15;
const MAX_CAPTURE_SECONDS: usize = 5 * 60;
const LEVEL_INTERVAL: Duration = Duration::from_millis(32);

#[derive(Debug, Clone)]
pub struct RecordedAudio {
    pub session_id: SessionId,
    pub samples: Vec<f32>,
    pub sample_rate_hz: u32,
    /// Absolute source position represented by `samples[0]`.
    pub source_start_sample: usize,
    /// Absolute source position immediately after the final sample available
    /// when this recording or preview snapshot was created.
    pub source_total_samples: usize,
    /// Duration of the complete recording at snapshot time, not merely the
    /// duration of `samples`.
    pub duration_ms: u64,
}

impl RecordedAudio {
    #[must_use]
    pub fn segment_duration_ms(&self) -> u64 {
        self.samples.len() as u64 * 1000 / u64::from(self.sample_rate_hz.max(1))
    }

    #[must_use]
    pub fn segment_from_absolute(&self, source_start_sample: usize) -> Option<Self> {
        if source_start_sample < self.source_start_sample
            || source_start_sample > self.source_total_samples
        {
            return None;
        }
        let local_start = source_start_sample - self.source_start_sample;
        Some(Self {
            session_id: self.session_id,
            samples: self.samples.get(local_start..)?.to_vec(),
            sample_rate_hz: self.sample_rate_hz,
            source_start_sample,
            source_total_samples: self.source_total_samples,
            duration_ms: self.duration_ms,
        })
    }
}

#[derive(Clone)]
pub struct AudioSnapshotSource {
    commands: Sender<CaptureCommand>,
}

impl AudioSnapshotSource {
    pub fn snapshot_from(
        &self,
        session_id: SessionId,
        source_start_sample: usize,
    ) -> Option<RecordedAudio> {
        let (reply, response) = mpsc::sync_channel(1);
        self.commands
            .send(CaptureCommand::Snapshot {
                session_id,
                source_start_sample,
                reply,
            })
            .ok()?;
        response
            .recv_timeout(Duration::from_millis(80))
            .ok()
            .flatten()
    }
}

struct ActiveCapture {
    session_id: SessionId,
    samples: Vec<f32>,
    last_level_report: Instant,
    peak_since_report: f32,
    limit_reported: bool,
}

enum CaptureCommand {
    Begin {
        session_id: SessionId,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    End {
        session_id: SessionId,
        reply: mpsc::SyncSender<Result<RecordedAudio, String>>,
    },
    Cancel {
        session_id: SessionId,
        reply: mpsc::SyncSender<bool>,
    },
    Snapshot {
        session_id: SessionId,
        source_start_sample: usize,
        reply: mpsc::SyncSender<Option<RecordedAudio>>,
    },
    Shutdown,
}

pub struct AudioRecorder {
    stream: Stream,
    commands: Sender<CaptureCommand>,
}

impl AudioRecorder {
    pub fn open(events: Sender<SpeechWorkerEvent>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no default microphone is available")?;
        let supported = device
            .default_input_config()
            .context("could not read the default microphone format")?;
        let sample_rate_hz = supported.sample_rate();
        let channels = usize::from(supported.channels().max(1));
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();

        let ring_capacity = sample_rate_hz as usize * AUDIO_RING_SECONDS;
        let (producer, consumer) = RingBuffer::<f32>::new(ring_capacity);
        let dropped_samples = Arc::new(AtomicU64::new(0));
        let (commands, command_receiver) = mpsc::channel();
        let consumer_events = events.clone();
        let consumer_drops = Arc::clone(&dropped_samples);
        thread::Builder::new()
            .name("speech-audio-consumer".to_owned())
            .spawn(move || {
                capture_loop(
                    consumer,
                    command_receiver,
                    consumer_events,
                    consumer_drops,
                    sample_rate_hz,
                );
            })?;

        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(
                &device,
                &config,
                channels,
                producer,
                dropped_samples,
                events,
            )?,
            SampleFormat::I16 => build_stream::<i16>(
                &device,
                &config,
                channels,
                producer,
                dropped_samples,
                events,
            )?,
            SampleFormat::I32 => build_stream::<i32>(
                &device,
                &config,
                channels,
                producer,
                dropped_samples,
                events,
            )?,
            SampleFormat::U16 => build_stream::<u16>(
                &device,
                &config,
                channels,
                producer,
                dropped_samples,
                events,
            )?,
            other => bail!("unsupported microphone sample format: {other}"),
        };
        Ok(Self { stream, commands })
    }

    pub fn snapshot_source(&self) -> AudioSnapshotSource {
        AudioSnapshotSource {
            commands: self.commands.clone(),
        }
    }

    pub fn begin(&self, session_id: SessionId) -> Result<()> {
        let (reply, response) = mpsc::sync_channel(1);
        self.commands
            .send(CaptureCommand::Begin { session_id, reply })
            .context("audio capture worker stopped")?;
        response
            .recv()
            .context("audio capture worker did not acknowledge begin")?
            .map_err(anyhow::Error::msg)?;
        if let Err(error) = self.stream.play() {
            let _ = self.cancel(session_id);
            return Err(error).context("could not start microphone capture");
        }
        Ok(())
    }

    pub fn end(&self, session_id: SessionId) -> Result<RecordedAudio> {
        self.stream
            .pause()
            .context("could not stop microphone capture")?;
        let (reply, response) = mpsc::sync_channel(1);
        self.commands
            .send(CaptureCommand::End { session_id, reply })
            .context("audio capture worker stopped")?;
        response
            .recv()
            .context("audio capture worker did not finalize audio")?
            .map_err(anyhow::Error::msg)
    }

    pub fn cancel(&self, session_id: SessionId) -> bool {
        let _ = self.stream.pause();
        let (reply, response) = mpsc::sync_channel(1);
        if self
            .commands
            .send(CaptureCommand::Cancel { session_id, reply })
            .is_err()
        {
            return false;
        }
        response.recv().unwrap_or(false)
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        let _ = self.stream.pause();
        let _ = self.commands.send(CaptureCommand::Shutdown);
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    mut producer: Producer<f32>,
    dropped_samples: Arc<AtomicU64>,
    events: Sender<SpeechWorkerEvent>,
) -> Result<Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let error_events = events;
    device
        .build_input_stream(
            *config,
            move |input: &[T], _| {
                // This callback is real-time sensitive: no locks, allocation, filesystem, or IPC.
                // Stereo downmixing is bounded arithmetic and the preallocated SPSC queue is
                // wait-free. The consumer owns all growing buffers and UI event publication.
                for frame in input.chunks(channels) {
                    let mono = frame.iter().copied().map(f32::from_sample).sum::<f32>()
                        / frame.len().max(1) as f32;
                    if producer.push(mono.clamp(-1.0, 1.0)).is_err() {
                        dropped_samples.fetch_add(1, Ordering::Relaxed);
                    }
                }
            },
            move |error| {
                let _ = error_events.send(SpeechWorkerEvent::Failed {
                    session_id: None,
                    code: "microphone_stream".to_owned(),
                    message: error.to_string(),
                    retryable: true,
                });
            },
            None,
        )
        .context("could not create microphone stream")
}

fn capture_loop(
    mut consumer: Consumer<f32>,
    commands: Receiver<CaptureCommand>,
    events: Sender<SpeechWorkerEvent>,
    dropped_samples: Arc<AtomicU64>,
    sample_rate_hz: u32,
) {
    let mut active: Option<ActiveCapture> = None;
    loop {
        drain_audio(&mut consumer, active.as_mut(), &events, sample_rate_hz);
        let command = if active.is_some() {
            commands.recv_timeout(Duration::from_millis(2))
        } else {
            commands
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        match command {
            Ok(CaptureCommand::Begin { session_id, reply }) => {
                drain_discard(&mut consumer);
                dropped_samples.store(0, Ordering::Release);
                if active.is_some() {
                    let _ = reply.send(Err("another recording is already active".to_owned()));
                    continue;
                }
                active = Some(ActiveCapture {
                    session_id,
                    samples: Vec::with_capacity(sample_rate_hz as usize * CAPTURE_RESERVE_SECONDS),
                    last_level_report: Instant::now() - LEVEL_INTERVAL,
                    peak_since_report: 0.0,
                    limit_reported: false,
                });
                let _ = reply.send(Ok(()));
            }
            Ok(CaptureCommand::End { session_id, reply }) => {
                drain_audio(&mut consumer, active.as_mut(), &events, sample_rate_hz);
                let result = finish_capture(&mut active, session_id, sample_rate_hz);
                let dropped = dropped_samples.swap(0, Ordering::AcqRel);
                if dropped > 0 {
                    eprintln!(
                        "audio ring overflowed; dropped {dropped} samples for session {session_id}"
                    );
                }
                let _ = reply.send(result);
            }
            Ok(CaptureCommand::Cancel { session_id, reply }) => {
                let cancelled = active
                    .as_ref()
                    .is_some_and(|capture| capture.session_id == session_id);
                if cancelled {
                    active = None;
                    drain_discard(&mut consumer);
                }
                let _ = reply.send(cancelled);
            }
            Ok(CaptureCommand::Snapshot {
                session_id,
                source_start_sample,
                reply,
            }) => {
                drain_audio(&mut consumer, active.as_mut(), &events, sample_rate_hz);
                let snapshot = active
                    .as_ref()
                    .filter(|capture| capture.session_id == session_id)
                    .and_then(|capture| {
                        snapshot_audio(capture, sample_rate_hz, source_start_sample)
                    });
                let _ = reply.send(snapshot);
            }
            Ok(CaptureCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn drain_audio(
    consumer: &mut Consumer<f32>,
    active: Option<&mut ActiveCapture>,
    events: &Sender<SpeechWorkerEvent>,
    sample_rate_hz: u32,
) {
    let Some(active) = active else {
        drain_discard(consumer);
        return;
    };
    let max_samples = sample_rate_hz as usize * MAX_CAPTURE_SECONDS;
    while let Ok(sample) = consumer.pop() {
        active.peak_since_report = active.peak_since_report.max(sample.abs());
        if active.samples.len() < max_samples {
            active.samples.push(sample);
        } else if !active.limit_reported {
            active.limit_reported = true;
            let _ = events.send(SpeechWorkerEvent::Failed {
                session_id: Some(active.session_id),
                code: "recording_limit".to_owned(),
                message: "The five-minute recording limit was reached.".to_owned(),
                retryable: false,
            });
        }
    }
    if active.last_level_report.elapsed() >= LEVEL_INTERVAL {
        active.last_level_report = Instant::now();
        let peak = mem::take(&mut active.peak_since_report).clamp(0.0, 1.0);
        let _ = events.send(SpeechWorkerEvent::AudioLevel {
            session_id: active.session_id,
            peak,
        });
    }
}

fn drain_discard(consumer: &mut Consumer<f32>) {
    while consumer.pop().is_ok() {}
}

fn finish_capture(
    active: &mut Option<ActiveCapture>,
    session_id: SessionId,
    sample_rate_hz: u32,
) -> Result<RecordedAudio, String> {
    let capture = active
        .take()
        .ok_or_else(|| "no recording is active".to_owned())?;
    if capture.session_id != session_id {
        *active = Some(capture);
        return Err("recording session identity does not match".to_owned());
    }
    let duration_ms = capture.samples.len() as u64 * 1000 / u64::from(sample_rate_hz.max(1));
    Ok(RecordedAudio {
        session_id: capture.session_id,
        source_start_sample: 0,
        source_total_samples: capture.samples.len(),
        samples: capture.samples,
        sample_rate_hz,
        duration_ms,
    })
}

fn snapshot_audio(
    capture: &ActiveCapture,
    sample_rate_hz: u32,
    source_start_sample: usize,
) -> Option<RecordedAudio> {
    if source_start_sample > capture.samples.len() {
        return None;
    }
    let source_total_samples = capture.samples.len();
    let duration_ms = source_total_samples as u64 * 1000 / u64::from(sample_rate_hz.max(1));
    Some(RecordedAudio {
        session_id: capture.session_id,
        samples: capture.samples[source_start_sample..].to_vec(),
        sample_rate_hz,
        source_start_sample,
        source_total_samples,
        duration_ms,
    })
}

pub fn resample_bandlimited(input: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if input.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        return input.to_vec();
    }

    const HALF_TAPS: isize = 16;
    let divisor = greatest_common_divisor(source_rate, target_rate);
    let input_step = u64::from(source_rate / divisor);
    let phase_count = usize::try_from(target_rate / divisor).unwrap_or(1).max(1);
    let cutoff = (f64::from(target_rate) / f64::from(source_rate)).min(1.0) * 0.94;
    let kernels = (0..phase_count)
        .map(|phase| {
            let fraction = phase as f64 / phase_count as f64;
            let mut kernel = (-HALF_TAPS..=HALF_TAPS)
                .map(|tap| {
                    let distance = tap as f64 - fraction;
                    let normalized = distance / (HALF_TAPS as f64 + 1.0);
                    let window = 0.42
                        + 0.5 * (std::f64::consts::PI * normalized).cos()
                        + 0.08 * (2.0 * std::f64::consts::PI * normalized).cos();
                    (cutoff * sinc(cutoff * distance) * window) as f32
                })
                .collect::<Vec<_>>();
            let sum = kernel.iter().sum::<f32>();
            if sum.abs() > f32::EPSILON {
                for coefficient in &mut kernel {
                    *coefficient /= sum;
                }
            }
            kernel
        })
        .collect::<Vec<_>>();

    let output_len =
        ((input.len() as u64 * u64::from(target_rate)) / u64::from(source_rate)) as usize;
    let mut output = Vec::with_capacity(output_len);
    let phase_denominator = phase_count as u64;
    for output_index in 0..output_len {
        let position_numerator = output_index as u64 * input_step;
        let center = (position_numerator / phase_denominator) as isize;
        let phase = (position_numerator % phase_denominator) as usize;
        let mut value = 0.0_f32;
        let mut used_weight = 0.0_f32;
        for (offset, coefficient) in (-HALF_TAPS..=HALF_TAPS).zip(&kernels[phase]) {
            let source_index = center + offset;
            if let Some(sample) = usize::try_from(source_index)
                .ok()
                .and_then(|index| input.get(index))
            {
                value += sample * coefficient;
                used_weight += coefficient;
            }
        }
        output.push(if used_weight.abs() > 1.0e-6 {
            value / used_weight
        } else {
            0.0
        });
    }
    output
}

fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left.max(1)
}

fn sinc(value: f64) -> f64 {
    if value.abs() < 1.0e-12 {
        1.0
    } else {
        let radians = std::f64::consts::PI * value;
        radians.sin() / radians
    }
}

#[cfg(test)]
mod tests {
    use rust_protocol::SessionId;

    use super::{RecordedAudio, resample_bandlimited};

    #[test]
    fn resampler_preserves_duration() {
        let source = vec![0.0_f32; 48_000];
        assert_eq!(resample_bandlimited(&source, 48_000, 16_000).len(), 16_000);
    }

    #[test]
    fn downsampling_rejects_energy_above_the_target_nyquist_frequency() {
        let sine = |frequency: f32| {
            (0..48_000)
                .map(|index| {
                    (2.0 * std::f32::consts::PI * frequency * index as f32 / 48_000.0).sin()
                })
                .collect::<Vec<_>>()
        };
        let low = resample_bandlimited(&sine(1_000.0), 48_000, 16_000);
        let aliased = resample_bandlimited(&sine(12_000.0), 48_000, 16_000);
        let rms = |samples: &[f32]| {
            (samples[64..samples.len() - 64]
                .iter()
                .map(|sample| sample * sample)
                .sum::<f32>()
                / (samples.len() - 128) as f32)
                .sqrt()
        };
        assert!(rms(&low) > 0.65);
        assert!(rms(&aliased) < 0.02);
    }

    #[test]
    fn absolute_segments_preserve_total_timeline_and_copy_only_the_tail() {
        let recording = RecordedAudio {
            session_id: SessionId::new(),
            samples: (0..16_000).map(|sample| sample as f32).collect(),
            sample_rate_hz: 16_000,
            source_start_sample: 32_000,
            source_total_samples: 48_000,
            duration_ms: 3_000,
        };

        let tail = recording.segment_from_absolute(40_000).unwrap();
        assert_eq!(tail.source_start_sample, 40_000);
        assert_eq!(tail.source_total_samples, 48_000);
        assert_eq!(tail.duration_ms, 3_000);
        assert_eq!(tail.segment_duration_ms(), 500);
        assert_eq!(tail.samples.first(), Some(&8_000.0));
        assert!(recording.segment_from_absolute(31_999).is_none());
        assert!(recording.segment_from_absolute(48_001).is_none());
    }
}
