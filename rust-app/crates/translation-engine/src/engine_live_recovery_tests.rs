//! Explicit opt-in real-model diagnostics. No fixture text or credentials live here.
//!
//! Ignored tests require SAYFLEX_LIVE_FIXTURE (a TranslateBlocks JSON/JSONL),
//! SAYFLEX_LIVE_MODEL, SAYFLEX_LIVE_RUNTIME and SAYFLEX_LIVE_OUTPUT_DIR.
//! SAYFLEX_LIVE_BLOCK_IDS optionally selects comma-separated exact block IDs while
//! preserving the fixture's complete context. The guided test optionally reads
//! SAYFLEX_LIVE_PRIOR (block_id and translated_text); otherwise it captures a real
//! initial model response before checking the production recovery path.
//! The prompt comparison reads SAYFLEX_LIVE_PROBES: up to three external objects
//! with label, system and user strings and optional enable_thinking (default false);
//! select exactly one original fixture block.
//! Thinking probes accept optional reasoning_budget_tokens in 1..=256 (default128),
//! passed per request without changing the isolated server's defaults.
//! Optional temperature is limited to 0.0..=0.8 (default0.0), with seed default0.
//! Run only after inspecting available RAM and approving a separate CPU process.

#![cfg(windows)]

use std::{collections::HashSet, env, mem::size_of, os::windows::process::CommandExt, sync::Mutex};

use rust_protocol::TranslationWorkerCommand;
use serde_json::{Value, json};

use super::*;

static LIVE_RUN: Mutex<()> = Mutex::new(());
const GIB: u64 = 1024 * 1024 * 1024;

struct Fixture {
    path: PathBuf,
    blocks: Vec<ScreenTextBlock>,
    context: SourceContext,
    source: TranslationLanguage,
    target: TranslationLanguage,
}

struct CpuHarness {
    // Drop the owned process before the temporary, deliberately empty runtime root.
    engine: TranslationEngine,
    _restart_guard: tempfile::TempDir,
    report_path: PathBuf,
    log_path: PathBuf,
    available_ram: u64,
    model_bytes: u64,
}

fn required_path(name: &str) -> Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("set {name} explicitly before running an ignored live diagnostic"))
}

fn load_fixture() -> Result<Fixture> {
    let path = required_path("SAYFLEX_LIVE_FIXTURE")?;
    let text = fs::read_to_string(&path)?;
    let text = text.trim_start_matches('\u{feff}');
    let command = serde_json::from_str::<TranslationWorkerCommand>(text)
        .ok()
        .or_else(|| {
            text.lines().find_map(|line| {
                serde_json::from_str::<TranslationWorkerCommand>(line)
                    .ok()
                    .filter(|command| {
                        matches!(command, TranslationWorkerCommand::TranslateBlocks { .. })
                    })
            })
        })
        .ok_or_else(|| anyhow!("fixture must contain a production TranslateBlocks command"))?;
    let TranslationWorkerCommand::TranslateBlocks {
        mut blocks,
        context,
        source_language,
        target_language,
        ..
    } = command
    else {
        bail!("fixture is not a TranslateBlocks command");
    };
    if let Ok(selected) = env::var("SAYFLEX_LIVE_BLOCK_IDS") {
        let selected = selected
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .collect::<HashSet<_>>();
        if selected.is_empty()
            || selected
                .iter()
                .any(|id| !blocks.iter().any(|block| block.block_id.as_str() == *id))
        {
            bail!("every selected diagnostic block ID must exist in the original fixture");
        }
        blocks.retain(|block| selected.contains(block.block_id.as_str()));
    }
    if blocks.is_empty() {
        bail!("live fixture contains no blocks");
    }
    Ok(Fixture {
        path,
        blocks,
        context,
        source: source_language
            .ok_or_else(|| anyhow!("live fixture must specify source_language"))?,
        target: target_language
            .ok_or_else(|| anyhow!("live fixture must specify target_language"))?,
    })
}

#[repr(C)]
struct MemoryStatus {
    length: u32,
    load: u32,
    total_physical: u64,
    available_physical: u64,
    total_page_file: u64,
    available_page_file: u64,
    total_virtual: u64,
    available_virtual: u64,
    available_extended_virtual: u64,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
}

fn available_physical_memory() -> Result<u64> {
    let mut status = MemoryStatus {
        length: size_of::<MemoryStatus>() as u32,
        load: 0,
        total_physical: 0,
        available_physical: 0,
        total_page_file: 0,
        available_page_file: 0,
        total_virtual: 0,
        available_virtual: 0,
        available_extended_virtual: 0,
    };
    // SAFETY: the documented structure has its exact byte length and writable storage.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(status.available_physical)
}

impl CpuHarness {
    fn start(label: &str) -> Result<Self> {
        let model = required_path("SAYFLEX_LIVE_MODEL")?.canonicalize()?;
        let runtime_root = required_path("SAYFLEX_LIVE_RUNTIME")?.canonicalize()?;
        let executable = runtime_root.join("llama-server.exe");
        if !executable.is_file() {
            bail!("diagnostic runtime does not contain llama-server.exe");
        }
        let model_bytes = fs::metadata(&model)?.len();
        let available_ram = available_physical_memory()?;
        let required_ram = model_bytes.saturating_add(4 * GIB);
        if available_ram < required_ram {
            bail!(
                "CPU diagnostic RAM guard: available={available_ram}, required={required_ram} (model plus 4 GiB reserve)"
            );
        }
        let pack = crate::TranslationCatalog::load()?
            .packs
            .into_iter()
            .find(|pack| {
                matches!(&pack.backend, TranslationBackendDescriptor::LlamaCpp { model_file, .. }
                if model.file_name().is_some_and(|name| name == std::ffi::OsStr::new(model_file)))
            })
            .ok_or_else(|| {
                anyhow!("diagnostic model must match a production catalog GGUF filename")
            })?;
        let TranslationBackendDescriptor::LlamaCpp {
            context_size,
            max_output_tokens,
            ..
        } = &pack.backend
        else {
            unreachable!("catalog selection accepts only LlamaCpp packs");
        };
        let (context_size, max_output_tokens) = (*context_size, *max_output_tokens);
        let output_root = required_path("SAYFLEX_LIVE_OUTPUT_DIR")?;
        fs::create_dir_all(&output_root)?;
        let run_id = Uuid::new_v4().simple().to_string();
        let log_path = output_root.join(format!("{label}-{run_id}.server.log"));
        let report_path = output_root.join(format!("{label}-{run_id}.report.json"));
        let restart_guard = tempfile::Builder::new()
            .prefix("sayflex-no-gpu-restart-")
            .tempdir()?;

        // Build fallible clients before spawning, so an initialization failure cannot
        // orphan a process. These settings match the production structured client.
        let client = blocking_local_client()?;
        let async_client = reqwest::Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(120))
            .build()?;
        let async_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let key = Uuid::new_v4().simple().to_string();
        let log = File::create(&log_path)?;
        let error_log = log.try_clone()?;
        let mut command = Command::new(&executable);
        command.env_clear();
        for name in ["SystemRoot", "WINDIR", "SystemDrive", "COMSPEC"] {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .arg("-m")
            .arg(&model)
            .args(["-c", &context_size.to_string(), "-np", "1"])
            .args(["-ngl", "0", "-dev", "none", "-t", "4", "-tb", "4"])
            .args(["--no-kv-offload", "--no-op-offload", "--prio", "-1"])
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--no-ui",
                "--offline",
            ])
            .current_dir(&runtime_root)
            .env("PATH", &runtime_root)
            .env("TEMP", restart_guard.path())
            .env("TMP", restart_guard.path())
            .env("LLAMA_API_KEY", &key)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(error_log))
            // CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS, before model load.
            .creation_flags(0x0800_0000 | 0x0000_4000);
        let child = command
            .spawn()
            .context("could not start isolated CPU server")?;
        let runtime = LlamaRuntime {
            child,
            client,
            async_client,
            async_runtime,
            endpoint: format!("http://127.0.0.1:{port}"),
            authorization: format!("Bearer {key}"),
            max_output_tokens,
        };
        // LlamaRuntime owns and kills only this newly spawned child on every exit.
        let ready = runtime.wait_until_ready(Instant::now() + SERVER_START_TIMEOUT, &|| true)?;
        let StartAttempt::Ready(runtime) = ready else {
            bail!(
                "isolated CPU server did not become ready; see {}",
                log_path.display()
            );
        };
        let engine = TranslationEngine {
            pack,
            model_root: model
                .parent()
                .ok_or_else(|| anyhow!("model has no parent"))?
                .to_path_buf(),
            // No executable exists here: a dead CPU child must never trigger the
            // production starter, which intentionally selects GPU devices.
            runtime_root: restart_guard.path().to_path_buf(),
            runtime: Some(ResidentRuntime::Llama(*runtime)),
        };
        Ok(Self {
            engine,
            _restart_guard: restart_guard,
            report_path,
            log_path,
            available_ram,
            model_bytes,
        })
    }

    fn write_report(
        &self,
        fixture: &Fixture,
        elapsed: Duration,
        events: &[Value],
        corrections: &[Value],
        result: &Result<Vec<TranslatedScreenBlock>>,
    ) -> Result<()> {
        let report = json!({
            "fixture": fixture.path,
            "block_ids": fixture.blocks.iter().map(|block|block.block_id.as_str()).collect::<Vec<_>>(),
            "model_bytes": self.model_bytes, "available_ram_before_start": self.available_ram,
            "cpu_threads": 4, "gpu_layers": 0, "production_operation_limit_ms": STRUCTURED_OPERATION_TIMEOUT.as_millis(),
            "elapsed_ms": elapsed.as_millis(), "partial_events": events,
            "corrective_requests": corrections,
            "blocks": result.as_ref().ok(), "error": result.as_ref().err().map(|error|format!("{error:#}")),
            "server_log": self.log_path,
        });
        fs::write(&self.report_path, serde_json::to_vec_pretty(&report)?)?;
        eprintln!("live CPU recovery evidence: {}", self.report_path.display());
        Ok(())
    }
}

#[test]
#[ignore = "opt-in real model CPU process; inspect RAM and set SAYFLEX_LIVE_* paths first"]
fn production_fixture_live_cpu() -> Result<()> {
    let _exclusive = LIVE_RUN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = load_fixture()?;
    let mut harness = CpuHarness::start("production-fixture")?;
    let mut events = Vec::new();
    let started = Instant::now();
    let result = harness
        .engine
        .translate_blocks_streaming(
            &fixture.blocks,
            &fixture.context,
            &[],
            Some(fixture.source),
            Some(fixture.target),
            &|| true,
            |block| {
                events.push(json!({ "elapsed_ms": started.elapsed().as_millis(), "block": block }));
                true
            },
        )
        .map(|output| output.blocks);
    harness.write_report(&fixture, started.elapsed(), &events, &[], &result)?;
    let blocks = result?;
    assert_eq!(blocks.len(), fixture.blocks.len());
    assert!(
        blocks
            .iter()
            .all(|block| block.completion == BlockCompletion::Completed),
        "real model left unresolved rows; this is not a successful recovery gate (see report)"
    );
    let published = events
        .iter()
        .filter_map(|event| event["block"]["block_id"].as_str())
        .collect::<HashSet<_>>();
    assert_eq!(published.len(), events.len(), "a block was published twice");
    assert_eq!(
        published.len(),
        blocks.len(),
        "every completed row must have been published"
    );
    Ok(())
}

#[test]
#[ignore = "opt-in real corrective model request; CPU paths required, SAYFLEX_LIVE_PRIOR optional"]
fn guided_recovery_live_cpu() -> Result<()> {
    let _exclusive = LIVE_RUN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut fixture = load_fixture()?;
    let prior = env::var_os("SAYFLEX_LIVE_PRIOR").map(|path| -> Result<String> {
        let prior: StructuredTranslationItem = serde_json::from_slice(&fs::read(path)?)?;
        fixture.blocks.retain(|block| block.block_id == prior.block_id);
        assert_eq!(fixture.blocks.len(), 1, "prior must identify one original block");
        Ok(json!({"translations": [{"block_id": prior.block_id, "translated_text": prior.translated_text}]}).to_string())
    }).transpose()?;
    let mut harness = CpuHarness::start("guided-recovery")?;
    let started = Instant::now();
    let deadline = started + STRUCTURED_OPERATION_TIMEOUT;
    let active = || Instant::now() < deadline;
    let initial = if let Some(prior) = prior {
        prior
    } else {
        let messages = structured_translation_messages(
            &fixture.blocks,
            &fixture.context,
            &[],
            fixture.source,
            fixture.target,
        )?;
        let Some(ResidentRuntime::Llama(runtime)) = harness.engine.runtime.as_mut() else {
            unreachable!()
        };
        runtime.translate_structured(messages, &active)?
    };
    fs::write(harness.report_path.with_extension("initial.json"), &initial)?;
    assert!(
        validate_structured_response(&initial, &fixture.blocks, fixture.target).is_err(),
        "guided test requires an actually invalid prior candidate"
    );
    let corrections_path = harness.report_path.with_extension("corrections.json");
    let Some(ResidentRuntime::Llama(runtime)) = harness.engine.runtime.as_mut() else {
        unreachable!()
    };
    let mut retries = 0;
    let mut events = Vec::new();
    let mut corrections = Vec::new();
    let result = recover_structured_response_streaming(
        &initial,
        &fixture.blocks,
        fixture.target,
        |block, feedback| {
            if !active() {
                bail!("guided recovery exceeded the production operation deadline");
            }
            retries += 1;
            let messages = budgeted_recovery_messages(
                feedback,
                |feedback| {
                    structured_recovery_messages(
                        block,
                        &fixture.context,
                        &[],
                        fixture.source,
                        fixture.target,
                        feedback,
                    )
                },
                |messages| runtime.structured_request_fits(messages, &active),
            )?;
            // Test-only private evidence: save the exact budget-admitted prompt
            // before inference, and raw output before the recovery validator.
            // The production runtime and logs never receive this instrumentation.
            corrections.push(json!({
                "attempt": feedback.attempt,
                "validation_reason": feedback.reason.to_string(),
                "block_id": block.block_id,
                "request_elapsed_ms": started.elapsed().as_millis(),
                "system": &messages.0,
                "user": &messages.1,
                "chat_request": structured_chat_request(
                    messages.clone(), runtime.max_output_tokens, Some(feedback.attempt)
                ),
            }));
            fs::write(&corrections_path, serde_json::to_vec_pretty(&corrections)?)?;
            let response =
                runtime.translate_structured_correction(messages, feedback.attempt, &active);
            let correction = corrections.last_mut().expect("request evidence was saved");
            correction["response_elapsed_ms"] = json!(started.elapsed().as_millis());
            correction["raw_response"] = json!(response.as_ref().ok());
            correction["error"] = json!(response.as_ref().err().map(|error| format!("{error:#}")));
            fs::write(&corrections_path, serde_json::to_vec_pretty(&corrections)?)?;
            let alignment_sent = corrections
                .last()
                .and_then(|entry| entry["user"].as_str())
                .ok_or_else(|| anyhow!("missing admitted corrective request"))?;
            let alignment_sent =
                recovery_request_uses_alignment(&(String::new(), alignment_sent.to_owned()))?;
            response.map(|text| StructuredRecoveryResponse {
                text,
                alignment_sent,
            })
        },
        |block| {
            events.push(json!({ "elapsed_ms": started.elapsed().as_millis(), "block": block }));
            true
        },
    );
    harness.write_report(&fixture, started.elapsed(), &events, &corrections, &result)?;
    let blocks = result?;
    assert!(
        (1..=2 * fixture.blocks.len()).contains(&retries),
        "correction must execute bounded real model requests"
    );
    assert_eq!(blocks.len(), fixture.blocks.len());
    assert!(
        blocks
            .iter()
            .all(|block| block.completion == BlockCompletion::Completed),
        "real corrective recovery failed; inspect report"
    );
    assert_eq!(
        events.len(),
        blocks.len(),
        "each accepted row must be published once"
    );
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CorrectivePromptProbe {
    label: String,
    system: String,
    user: String,
    #[serde(default)]
    enable_thinking: bool,
    #[serde(default)]
    reasoning_budget_tokens: Option<u16>,
    #[serde(default)]
    temperature: f32,
    #[serde(default)]
    seed: u32,
}

fn translate_prompt_probe(
    runtime: &mut LlamaRuntime,
    probe: &CorrectivePromptProbe,
    deadline: Instant,
    evidence: &mut Value,
) -> Result<String> {
    if !runtime.is_running() {
        bail!("isolated prompt probe runtime is not running");
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(TranslationCancelled.into());
    }
    // Use the production request and response types with explicit diagnostic
    // options. No alternate production entry point or runtime hook.
    let request = ChatRequest {
        model: "local",
        messages: [
            ChatMessage {
                role: "system",
                content: probe.system.clone(),
            },
            ChatMessage {
                role: "user",
                content: probe.user.clone(),
            },
        ],
        temperature: probe.temperature,
        max_tokens: runtime.max_output_tokens,
        seed: probe.seed,
        stream: false,
        chat_template_kwargs: ChatTemplateKwargs {
            enable_thinking: probe.enable_thinking,
        },
    };
    let mut request = serde_json::to_value(&request)?;
    if probe.enable_thinking {
        request["reasoning_budget_tokens"] = json!(probe.reasoning_budget_tokens.unwrap_or(128));
    }
    evidence["chat_request"] = request.clone();
    let request = runtime
        .async_client
        .post(format!("{}/v1/chat/completions", runtime.endpoint))
        .header(AUTHORIZATION, &runtime.authorization)
        .header(CONTENT_TYPE, "application/json")
        .timeout(remaining)
        .json(&request);
    let active = || Instant::now() < deadline;
    runtime.async_runtime.block_on(async {
        let response = tokio::select! {
            response = request.send() => response.context("prompt probe request failed")?,
            () = wait_until_cancelled(&active) => return Err(TranslationCancelled.into()),
        };
        let status = response.status();
        evidence["http_status"] = json!(status.as_u16());
        let body = tokio::select! {
            body = response.bytes() => body.context("could not read prompt probe completion")?,
            () = wait_until_cancelled(&active) => return Err(TranslationCancelled.into()),
        };
        evidence["raw_completion_json"] = json!(String::from_utf8_lossy(&body));
        if let Ok(completion) = serde_json::from_slice::<Value>(&body) {
            evidence["reasoning_content"] = completion
                .pointer("/choices/0/message/reasoning_content")
                .or_else(|| completion.pointer("/choices/0/message/reasoning"))
                .cloned()
                .unwrap_or(Value::Null);
            evidence["message_content"] = completion
                .pointer("/choices/0/message/content")
                .cloned()
                .unwrap_or(Value::Null);
        }
        if !status.is_success() {
            bail!("prompt probe returned HTTP {status}; private report contains response");
        }
        let response: ChatResponse = serde_json::from_slice(&body)
            .context("prompt probe returned invalid completion JSON")?;
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("prompt probe returned no result"))?;
        validate_finish_reason(choice.finish_reason.as_deref())?;
        let content = strip_reasoning(&choice.message.content).trim();
        if content.is_empty() {
            bail!("prompt probe returned an empty result");
        }
        if !active() {
            return Err(TranslationCancelled.into());
        }
        Ok(content.to_owned())
    })
}

#[test]
#[ignore = "opt-in CPU prompt comparison; requires SAYFLEX_LIVE_PROBES and one selected fixture block"]
fn corrective_prompt_probes_live_cpu() -> Result<()> {
    let _exclusive = LIVE_RUN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = load_fixture()?;
    if fixture.blocks.len() != 1 {
        bail!("prompt comparison requires exactly one selected original fixture block");
    }
    let probes_path = required_path("SAYFLEX_LIVE_PROBES")?;
    let probes_text = fs::read_to_string(&probes_path)?;
    let probes: Vec<CorrectivePromptProbe> =
        serde_json::from_str(probes_text.trim_start_matches('\u{feff}'))?;
    if !(1..=3).contains(&probes.len()) {
        bail!("prompt comparison accepts one to three external prompts");
    }
    if probes.iter().any(|probe| {
        probe
            .reasoning_budget_tokens
            .is_some_and(|budget| !(1..=256).contains(&budget))
    }) {
        bail!("probe reasoning_budget_tokens must be in 1..=256");
    }
    if probes
        .iter()
        .any(|probe| !probe.temperature.is_finite() || !(0.0..=0.8).contains(&probe.temperature))
    {
        bail!("probe temperature must be finite and in 0.0..=0.8");
    }
    let mut harness = CpuHarness::start("corrective-probes")?;
    let report_path = harness.report_path.clone();
    let started = Instant::now();
    let deadline = started + STRUCTURED_OPERATION_TIMEOUT;
    let active = || Instant::now() < deadline;
    let mut report = json!({
        "diagnostic_only": true,
        "fixture": fixture.path,
        "block_id": fixture.blocks[0].block_id,
        "probe_file": probes_path,
        "model_bytes": harness.model_bytes,
        "available_ram_before_start": harness.available_ram,
        "cpu_threads": 4, "gpu_layers": 0,
        "production_operation_limit_ms": STRUCTURED_OPERATION_TIMEOUT.as_millis(),
        "server_log": harness.log_path,
        "requested_probe_count": probes.len(),
        "valid_response_count": 0,
        "all_requested_probes_finished": false,
        "probes": [],
    });
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    eprintln!("live CPU prompt evidence: {}", report_path.display());
    let Some(ResidentRuntime::Llama(runtime)) = harness.engine.runtime.as_mut() else {
        unreachable!()
    };
    let mut valid_count = 0;
    for (index, probe) in probes.into_iter().enumerate() {
        if !active() {
            report["error"] = json!("shared production operation deadline exceeded");
            report["elapsed_ms"] = json!(started.elapsed().as_millis());
            fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
            bail!("prompt comparison exceeded the shared production operation deadline");
        }
        let messages = (probe.system.clone(), probe.user.clone());
        report["probes"]
            .as_array_mut()
            .expect("probe array")
            .push(json!({
                "label": probe.label,
                "request_elapsed_ms": started.elapsed().as_millis(),
                "system": &messages.0,
                "user": &messages.1,
                "enable_thinking": probe.enable_thinking,
                "temperature": probe.temperature,
                "seed": probe.seed,
                "reasoning_budget_tokens": probe.enable_thinking.then_some(probe.reasoning_budget_tokens.unwrap_or(128)),
                "validation": "not_run",
            }));
        fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
        let response = (|| {
            if !runtime.structured_request_fits(&messages, &active)? {
                bail!("probe exceeds the unchanged production request budget");
            }
            translate_prompt_probe(runtime, &probe, deadline, &mut report["probes"][index])
        })();
        report["probes"][index]["response_elapsed_ms"] = json!(started.elapsed().as_millis());
        report["probes"][index]["raw_response"] = json!(response.as_ref().ok());
        report["probes"][index]["error"] =
            json!(response.as_ref().err().map(|error| format!("{error:#}")));
        // Save exact raw output before validation. Invalid output remains evidence;
        // this diagnostic never publishes or manufactures a completed block.
        fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
        let response = response?;
        match validate_structured_response(&response, &fixture.blocks, fixture.target) {
            Ok(_) => {
                valid_count += 1;
                report["probes"][index]["validation"] = json!("valid");
            }
            Err(error) => {
                report["probes"][index]["validation"] = json!(error.to_string());
            }
        }
        report["valid_response_count"] = json!(valid_count);
        report["elapsed_ms"] = json!(started.elapsed().as_millis());
        fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    }
    report["all_requested_probes_finished"] = json!(true);
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    eprintln!(
        "prompt comparison finished: valid_responses={valid_count}; diagnostic success is not a recovery gate"
    );
    Ok(())
}
