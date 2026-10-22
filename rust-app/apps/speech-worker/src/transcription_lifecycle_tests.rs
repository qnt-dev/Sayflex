use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use rust_cloud_speech::{Chain, HttpExecutor, ProviderEntry, ProviderId, Router};
use rust_protocol::{DictationLanguage, SessionId, SpeechOutputMode, SpeechWorkerEvent};

use super::NetworkJob;
use crate::{audio::RecordedAudio, history_actor::HistoryCommand, inference::InferenceHandle};

fn job() -> (
    NetworkJob,
    mpsc::Receiver<SpeechWorkerEvent>,
    mpsc::Receiver<HistoryCommand>,
) {
    let (events, received_events) = mpsc::channel();
    let (history, received_history) = mpsc::channel();
    let inference = InferenceHandle::start(events.clone(), history.clone()).unwrap();
    let session_id = SessionId::new();
    inference.begin_session(session_id);
    let (results, _) = mpsc::channel();
    (
        NetworkJob {
            router: Router::new(Chain::new(Vec::new())),
            revision: 0,
            secrets: HashMap::new(),
            executor: None,
            recording: RecordedAudio {
                session_id,
                samples: vec![0.0; 160],
                sample_rate_hz: 16_000,
                duration_ms: 10,
                source_start_sample: 0,
                source_total_samples: 160,
            },
            output_mode: SpeechOutputMode::OriginalLanguage,
            audio_finalize_ms: 0,
            release_started: Instant::now(),
            language: DictationLanguage::Auto,
            events,
            history,
            cancelled: inference.cancellations(),
            inference,
            results,
        },
        received_events,
        received_history,
    )
}

// Only transport is synthetic: the real router, publication gate, inference
// owner and HTTP response parser all run against a loopback endpoint.
fn provider_response(
    job: &mut NetworkJob,
    status: u16,
    body: &'static str,
    before_response: impl FnOnce() + Send + 'static,
) -> thread::JoinHandle<()> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut provider = ProviderEntry::new(ProviderId::GROQ, true);
    provider.endpoint = Some(format!("http://{}", listener.local_addr().unwrap()));
    job.router = Router::new(Chain::new(vec![provider]));
    job.secrets
        .insert(ProviderId::GROQ.into(), "fixture-key".into());
    job.executor = Some(HttpExecutor::new(Duration::from_secs(2)).unwrap());
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "the router did not request the fixture"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("fixture listener failed: {error}"),
            }
        };
        // Windows inherits nonblocking mode from the listening socket.
        // The response fixture uses blocking reads with the timeout below.
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let read = socket.read(&mut chunk).unwrap();
            assert_ne!(read, 0);
            request.extend_from_slice(&chunk[..read]);
            let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]).to_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length: ")
                        .map(|value| value.parse().unwrap())
                })
                .unwrap();
            if request.len() >= header_end + 4 + length {
                break;
            }
        }
        before_response();
        write!(socket, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    })
}

#[test]
fn cloud_success_releases_session_so_memory_unload_is_available() {
    let (mut job, events, _) = job();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.json");
    let actor = crate::history_actor::start(path.clone(), job.events.clone()).unwrap();
    job.history = actor.commands();
    let inference = job.inference.clone();
    let server = provider_response(&mut job, 200, r#"{"text":"completed fixture"}"#, || {});
    job.run();
    server.join().unwrap();
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(5)).unwrap(),
        SpeechWorkerEvent::TranscribedRemotely { .. }
    ));
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(5)).unwrap(),
        SpeechWorkerEvent::Completed { .. }
    ));
    actor.shutdown_and_join().unwrap();
    assert_eq!(
        crate::history::HistoryStore::open(path).unwrap().latest(1)[0]
            .transcript
            .text,
        "completed fixture"
    );
    assert!(
        !inference.has_active_session(),
        "a completed cloud dictation must not keep model_busy set"
    );
    assert_eq!(inference.unload_model_if_and_wait("fixture").unwrap(), None);
    inference.shutdown();
}

#[test]
fn cloud_no_speech_releases_session() {
    let (mut job, events, _) = job();
    let inference = job.inference.clone();
    let server = provider_response(&mut job, 200, r#"{"text":""}"#, || {});
    job.run();
    server.join().unwrap();
    assert!(events.try_iter().any(
        |event| matches!(event, SpeechWorkerEvent::Failed { code, .. } if code == "transcription")
    ));
    assert!(!inference.has_active_session());
    inference.shutdown();
}

#[test]
fn cloud_exhaustion_releases_session() {
    let (mut job, events, _) = job();
    let inference = job.inference.clone();
    let server = provider_response(&mut job, 503, r#"{"error":"fixture unavailable"}"#, || {});
    job.run();
    server.join().unwrap();
    assert!(events.try_iter().any(|event| matches!(event, SpeechWorkerEvent::Failed { code, .. } if code == "transcription_failed")));
    assert!(!inference.has_active_session());
    inference.shutdown();
}

#[test]
fn cancellation_before_network_execution_releases_session() {
    let (job, events, _) = job();
    let inference = job.inference.clone();
    job.cancelled.cancel(job.recording.session_id);
    job.run();
    assert!(matches!(
        events.try_recv(),
        Ok(SpeechWorkerEvent::Cancelled { .. })
    ));
    assert!(!inference.has_active_session());
    inference.shutdown();
}

#[test]
fn cancellation_during_network_execution_releases_session_without_history() {
    let (mut job, events, history) = job();
    let inference = job.inference.clone();
    let cancellations = job.cancelled.clone();
    let session_id = job.recording.session_id;
    let server = provider_response(&mut job, 200, r#"{"text":"late fixture"}"#, move || {
        cancellations.cancel(session_id)
    });
    job.run();
    server.join().unwrap();
    assert!(matches!(
        events.try_recv(),
        Ok(SpeechWorkerEvent::Cancelled { .. })
    ));
    assert!(history.try_recv().is_err());
    assert!(!inference.has_active_session());
    inference.shutdown();
}

#[test]
fn cloud_completion_preserves_a_newer_active_session() {
    let (mut job, _, _) = job();
    let inference = job.inference.clone();
    let newer_inference = inference.clone();
    let newer = SessionId::new();
    let server = provider_response(&mut job, 200, r#"{"text":"old fixture"}"#, move || {
        newer_inference.begin_session(newer)
    });
    job.run();
    server.join().unwrap();
    assert!(inference.has_active_session());
    inference.cancel_session(newer);
    assert!(
        !inference.has_active_session(),
        "the active session must still be the newer one"
    );
    inference.shutdown();
}

#[test]
fn missing_network_client_releases_session_when_local_is_disabled() {
    let (job, events, _) = job();
    let inference = job.inference.clone();
    job.run();
    assert!(matches!(
        events.try_recv(),
        Ok(SpeechWorkerEvent::Failed { .. })
    ));
    assert!(!inference.has_active_session());
    inference.shutdown();
}

#[test]
fn local_fallback_preserves_a_newer_active_session() {
    let (mut job, events, _) = job();
    job.router = Router::new(Chain::new(vec![ProviderEntry::new(
        ProviderId::LOCAL,
        true,
    )]));
    let inference = job.inference.clone();
    let older = job.recording.session_id;
    let newer = SessionId::new();
    inference.begin_session(newer);
    job.run();
    assert!(
        inference.has_active_session(),
        "handing an older cloud request to local inference must not release newer capture"
    );
    let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        matches!(event, SpeechWorkerEvent::Failed { session_id: Some(session_id), .. } if session_id == older),
        "the local inference owner must publish the terminal result"
    );
    inference.cancel_session(newer);
    assert!(!inference.has_active_session());
    inference.shutdown();
}
