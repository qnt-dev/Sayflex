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
const CAPTURE_RESERVE_SECONDS: usize = 60;
const LEVEL_INTERVAL: Duration = Duration::from_millis(32);

#[derive(Debug, Clone)]
pub struct RecordedAudio {
    pub session_id: SessionId,
    pub samples: Vec<f32>,
    pub sample_rate_hz: u32,
    pub duration_ms: u64,
}

#[derive(Clone)]
pub struct AudioSnapshotSource {
    commands: Sender<CaptureCommand>,
}

impl AudioSnapshotSource {
    pub fn snapshot(&self, session_id: SessionId) -> Option<RecordedAudio> {
        let (reply, response) = mpsc::sync_channel(1);
        self.commands
            .send(CaptureCommand::Snapshot { session_id, reply })
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
        drain_audio(&mut consumer, active.as_mut(), &events);
        match commands.recv_timeout(Duration::from_millis(2)) {
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
                });
                let _ = reply.send(Ok(()));
            }
            Ok(CaptureCommand::End { session_id, reply }) => {
                drain_audio(&mut consumer, active.as_mut(), &events);
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
            Ok(CaptureCommand::Snapshot { session_id, reply }) => {
                drain_audio(&mut consumer, active.as_mut(), &events);
                let snapshot = active
                    .as_ref()
                    .filter(|capture| capture.session_id == session_id)
                    .map(|capture| snapshot_audio(capture, sample_rate_hz));
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
) {
    let Some(active) = active else {
        drain_discard(consumer);
        return;
    };
    while let Ok(sample) = consumer.pop() {
        active.peak_since_report = active.peak_since_report.max(sample.abs());
        active.samples.push(sample);
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
        samples: capture.samples,
        sample_rate_hz,
        duration_ms,
    })
}

fn snapshot_audio(capture: &ActiveCapture, sample_rate_hz: u32) -> RecordedAudio {
    let duration_ms = capture.samples.len() as u64 * 1000 / u64::from(sample_rate_hz.max(1));
    RecordedAudio {
        session_id: capture.session_id,
        samples: capture.samples.clone(),
        sample_rate_hz,
        duration_ms,
    }
}

pub fn resample_linear(input: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if input.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        return input.to_vec();
    }

    let output_len =
        ((input.len() as u64 * u64::from(target_rate)) / u64::from(source_rate)) as usize;
    let mut output = Vec::with_capacity(output_len);
    let source_per_output = source_rate as f64 / target_rate as f64;
    for output_index in 0..output_len {
        let position = output_index as f64 * source_per_output;
        let left = position.floor() as usize;
        let right = (left + 1).min(input.len() - 1);
        let fraction = (position - left as f64) as f32;
        output.push(input[left] + ((input[right] - input[left]) * fraction));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::resample_linear;

    #[test]
    fn resampler_preserves_duration() {
        let source = vec![0.0_f32; 48_000];
        assert_eq!(resample_linear(&source, 48_000, 16_000).len(), 16_000);
    }
}
