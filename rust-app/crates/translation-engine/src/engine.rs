use std::{
    borrow::Cow,
    fs::{self, File, OpenOptions},
    io,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{
    StatusCode,
    blocking::Client,
    header::{AUTHORIZATION, CONTENT_TYPE},
};
use rust_protocol::{TranslationLanguage, TranslationTier};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{TranslationBackendDescriptor, TranslationPackDescriptor};

const MAX_INPUT_CHARACTERS: usize = 32_000;
const MAX_SOURCE_TOKENS_PER_REQUEST: usize = 1_200;
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(90);
const SERVER_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SERVER_LOG_LIMIT_BYTES: u64 = 2 * 1024 * 1024;

pub struct TranslationEngine {
    pack: TranslationPackDescriptor,
    model_root: PathBuf,
    runtime_root: PathBuf,
    runtime: Option<LlamaRuntime>,
}

pub struct TranslationOutput {
    pub text: String,
    pub source_language: TranslationLanguage,
    pub target_language: TranslationLanguage,
    pub first_token_duration_ms: u64,
    pub backend_request_count: u32,
}

#[derive(Debug, thiserror::Error)]
#[error("translation was cancelled")]
pub struct TranslationCancelled;

impl TranslationEngine {
    pub fn new(
        pack: TranslationPackDescriptor,
        model_root: PathBuf,
        runtime_root: PathBuf,
    ) -> Self {
        Self {
            pack,
            model_root,
            runtime_root,
            runtime: None,
        }
    }

    #[must_use]
    pub const fn tier(&self) -> TranslationTier {
        self.pack.tier
    }

    pub fn warm(&mut self) -> Result<()> {
        self.ensure_runtime()?;
        // Prime both prompt directions and GPU execution graphs. This happens only
        // when the user opts into the warm/fast mode, never in the input hook.
        for (text, source, target) in [
            (
                "Hello",
                TranslationLanguage::English,
                TranslationLanguage::Russian,
            ),
            (
                "Привет",
                TranslationLanguage::Russian,
                TranslationLanguage::English,
            ),
        ] {
            self.runtime
                .as_mut()
                .context("translation runtime failed to initialize")?
                .translate_unit(text, source, target)?;
        }
        Ok(())
    }

    pub fn translate(
        &mut self,
        text: &str,
        source_language: Option<TranslationLanguage>,
        target_language: Option<TranslationLanguage>,
    ) -> Result<(String, TranslationLanguage, TranslationLanguage)> {
        let translated =
            self.translate_streaming(text, source_language, target_language, || true, |_, _| true)?;
        Ok((
            translated.text,
            translated.source_language,
            translated.target_language,
        ))
    }

    pub fn translate_streaming(
        &mut self,
        text: &str,
        source_language: Option<TranslationLanguage>,
        target_language: Option<TranslationLanguage>,
        should_continue: impl Fn() -> bool,
        mut on_partial: impl FnMut(&str, u64) -> bool,
    ) -> Result<TranslationOutput> {
        if text.trim().is_empty() {
            bail!("text is empty");
        }
        if text.chars().count() > MAX_INPUT_CHARACTERS {
            bail!("text exceeds the {MAX_INPUT_CHARACTERS}-character local translation limit");
        }
        let source = source_language.unwrap_or_else(|| detect_language(text));
        let target = target_language.unwrap_or_else(|| source.opposite());
        if source == target {
            bail!("source and target languages are the same");
        }
        if !self.pack.supports(source, target) {
            bail!("installed pack does not support this translation direction");
        }
        // UI Automation providers commonly expose Windows paragraph breaks as
        // bare CR characters. Normalize the model input so the line-preserving
        // instruction operates on one unambiguous representation.
        let model_input = normalize_line_endings(text);

        self.ensure_runtime()?;
        let first_attempt = translate_planned(
            self.runtime
                .as_mut()
                .context("translation runtime failed to initialize")?,
            &model_input,
            source,
            target,
            &should_continue,
            &mut on_partial,
        );
        let translated = match first_attempt {
            Ok(translated) => translated,
            Err(first_error) => {
                if first_error.downcast_ref::<TranslationCancelled>().is_some() {
                    return Err(first_error);
                }
                // A crashed backend is restarted once in-process. The caller still
                // receives a normal error if the deterministic retry also fails.
                self.runtime = None;
                self.ensure_runtime()
                    .context("translation backend restart failed")?;
                translate_planned(
                    self.runtime
                        .as_mut()
                        .context("translation runtime failed to restart")?,
                    &model_input,
                    source,
                    target,
                    &should_continue,
                    &mut on_partial,
                )
                .with_context(|| format!("translation retry failed after: {first_error:#}"))?
            }
        };
        let normalized = normalize_single_word_repetition(text, &translated.text);
        Ok(TranslationOutput {
            text: normalized,
            source_language: source,
            target_language: target,
            first_token_duration_ms: translated.first_token_duration_ms,
            backend_request_count: translated.backend_request_count,
        })
    }

    fn ensure_runtime(&mut self) -> Result<&mut LlamaRuntime> {
        if self.runtime.as_mut().is_some_and(LlamaRuntime::is_running) {
            return self
                .runtime
                .as_mut()
                .ok_or_else(|| anyhow!("translation runtime disappeared"));
        }
        self.runtime = None;
        let TranslationBackendDescriptor::LlamaCpp {
            model_file,
            context_size,
            max_output_tokens,
        } = &self.pack.backend;
        let model_path = self.model_root.join(model_file);
        if !model_path.is_file() {
            bail!("translation model is missing: {}", model_path.display());
        }
        self.runtime = Some(LlamaRuntime::start(
            &self.runtime_root,
            &model_path,
            *context_size,
            *max_output_tokens,
        )?);
        self.runtime
            .as_mut()
            .ok_or_else(|| anyhow!("translation runtime disappeared"))
    }
}

#[must_use]
pub fn detect_language(text: &str) -> TranslationLanguage {
    let mut cyrillic = 0_usize;
    let mut latin = 0_usize;
    for character in text.chars() {
        if ('\u{0400}'..='\u{052f}').contains(&character) {
            cyrillic += 1;
        } else if character.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    if cyrillic > 0 && cyrillic.saturating_mul(2) >= latin {
        TranslationLanguage::Russian
    } else {
        TranslationLanguage::English
    }
}

struct LlamaRuntime {
    child: Child,
    client: Client,
    async_client: reqwest::Client,
    async_runtime: tokio::runtime::Runtime,
    endpoint: String,
    authorization: String,
    max_output_tokens: u32,
}

impl LlamaRuntime {
    fn start(
        runtime_root: &Path,
        model_path: &Path,
        context_size: u32,
        max_output_tokens: u32,
    ) -> Result<Self> {
        let executable = runtime_root.join("llama-server.exe");
        if !executable.is_file() {
            bail!(
                "llama.cpp translation runtime is missing: {}",
                executable.display()
            );
        }
        let device = preferred_vulkan_device(&executable, runtime_root);
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .context("could not reserve a local translation port")?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let api_key = Uuid::new_v4().simple().to_string();
        let authorization = format!("Bearer {api_key}");
        let log = open_server_log(runtime_root)?;
        let error_log = log.try_clone()?;
        let mut command = Command::new(&executable);
        command
            .arg("-m")
            .arg(model_path)
            .args(["-c", &context_size.to_string()])
            .args(["-np", "1"])
            .args(["-ngl", "all"])
            .args(["--host", "127.0.0.1"])
            .args(["--port", &port.to_string()])
            .args(["--api-key", &api_key])
            .args(["--no-ui", "--offline"])
            .current_dir(runtime_root)
            .env("PATH", runtime_root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(error_log));
        if let Some(device) = device {
            command.args(["-dev", &device]);
        }
        configure_hidden_process(&mut command);
        let child = command
            .spawn()
            .context("could not launch the local llama.cpp translation runtime")?;

        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(120))
            .build()
            .context("could not create the local translation client")?;
        let async_client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(120))
            .build()
            .context("could not create the cancellable local translation client")?;
        let async_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("could not create the local translation async runtime")?;
        let endpoint = format!("http://127.0.0.1:{port}");
        let mut runtime = Self {
            child,
            client,
            async_client,
            async_runtime,
            endpoint,
            authorization,
            max_output_tokens,
        };
        runtime.wait_until_ready()?;
        Ok(runtime)
    }

    fn wait_until_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + SERVER_START_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                bail!("llama.cpp translation runtime exited during startup: {status}");
            }
            match self
                .client
                .get(format!("{}/health", self.endpoint))
                .header(AUTHORIZATION, &self.authorization)
                .send()
            {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response)
                    if response.status() == StatusCode::SERVICE_UNAVAILABLE
                        || response.status() == StatusCode::TOO_MANY_REQUESTS => {}
                Ok(_) | Err(_) => {}
            }
            thread::sleep(SERVER_POLL_INTERVAL);
        }
        bail!("llama.cpp translation runtime did not become ready in time")
    }

    fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    fn translate_unit(
        &mut self,
        text: &str,
        source: TranslationLanguage,
        target: TranslationLanguage,
    ) -> Result<String> {
        if !self.is_running() {
            bail!("llama.cpp translation runtime is not running");
        }
        let request = ChatRequest {
            model: "local",
            messages: [
                ChatMessage {
                    role: "system",
                    content: translation_instruction(source, target),
                },
                ChatMessage {
                    role: "user",
                    content: text.to_owned(),
                },
            ],
            temperature: 0.0,
            max_tokens: self.max_output_tokens,
            seed: 0,
            stream: false,
            chat_template_kwargs: ChatTemplateKwargs {
                enable_thinking: false,
            },
        };
        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.endpoint))
            .header(AUTHORIZATION, &self.authorization)
            .header(CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .context("local translation request failed")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            bail!("local translation runtime returned {status}: {body}");
        }
        let response: ChatResponse = response
            .json()
            .context("local translation runtime returned invalid JSON")?;
        let content = response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .ok_or_else(|| anyhow!("local translation runtime returned no result"))?;
        let content = strip_reasoning(&content);
        if content.is_empty() {
            bail!("local translation runtime returned an empty result");
        }
        Ok(content.to_owned())
    }

    fn token_count(&self, text: &str) -> Result<usize> {
        let response = self
            .client
            .post(format!("{}/tokenize", self.endpoint))
            .header(AUTHORIZATION, &self.authorization)
            .header(CONTENT_TYPE, "application/json")
            .json(&TokenizeRequest {
                content: text,
                add_special: false,
                parse_special: false,
            })
            .send()
            .context("local tokenizer request failed")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            bail!("local tokenizer returned {status}: {body}");
        }
        let response: TokenizeResponse = response
            .json()
            .context("local tokenizer returned invalid JSON")?;
        Ok(response.tokens.len())
    }

    fn translate_unit_streaming(
        &mut self,
        text: &str,
        source: TranslationLanguage,
        target: TranslationLanguage,
        should_continue: &impl Fn() -> bool,
        mut on_partial: impl FnMut(&str, u64) -> bool,
    ) -> Result<StreamedUnit> {
        if !self.is_running() {
            bail!("llama.cpp translation runtime is not running");
        }
        let started = Instant::now();
        let request = ChatRequest {
            model: "local",
            messages: [
                ChatMessage {
                    role: "system",
                    content: translation_instruction(source, target),
                },
                ChatMessage {
                    role: "user",
                    content: text.to_owned(),
                },
            ],
            temperature: 0.0,
            max_tokens: self.max_output_tokens,
            seed: 0,
            stream: true,
            chat_template_kwargs: ChatTemplateKwargs {
                enable_thinking: false,
            },
        };
        let request = self
            .async_client
            .post(format!("{}/v1/chat/completions", self.endpoint))
            .header(AUTHORIZATION, &self.authorization)
            .header(CONTENT_TYPE, "application/json")
            .json(&request);
        let content = self.async_runtime.block_on(async {
            let mut response = tokio::select! {
                response = request.send() => response
                    .context("local streaming translation request failed")?,
                () = wait_until_cancelled(should_continue) => {
                    return Err(TranslationCancelled.into());
                }
            };
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                bail!("local streaming translation runtime returned {status}: {body}");
            }

            let mut content = String::new();
            let mut pending = Vec::new();
            let mut done = false;
            while !done {
                let chunk = tokio::select! {
                    chunk = response.chunk() => chunk
                        .context("could not read the local translation stream")?,
                    () = wait_until_cancelled(should_continue) => {
                        return Err(TranslationCancelled.into());
                    }
                };
                let Some(chunk) = chunk else {
                    break;
                };
                pending.extend_from_slice(&chunk);
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let line = pending.drain(..=end).collect::<Vec<_>>();
                    let line = std::str::from_utf8(&line[..line.len().saturating_sub(1)])
                        .context("local translation stream returned invalid UTF-8")?;
                    if process_stream_line(line, &mut content, started, &mut on_partial)? {
                        done = true;
                        break;
                    }
                }
            }
            if !done && !pending.is_empty() {
                let line = std::str::from_utf8(&pending)
                    .context("local translation stream returned invalid trailing UTF-8")?;
                let _ = process_stream_line(line, &mut content, started, &mut on_partial)?;
            }
            Ok::<String, anyhow::Error>(content)
        })?;

        let content = strip_reasoning(&content).trim().to_owned();
        if content.is_empty() {
            bail!("local translation runtime returned an empty streamed result");
        }
        Ok(StreamedUnit { content })
    }
}

impl Drop for LlamaRuntime {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[derive(Serialize)]
struct ChatRequest {
    model: &'static str,
    messages: [ChatMessage; 2],
    temperature: f32,
    max_tokens: u32,
    seed: u32,
    stream: bool,
    chat_template_kwargs: ChatTemplateKwargs,
}

#[derive(Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Serialize)]
struct ChatTemplateKwargs {
    enable_thinking: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

#[derive(Serialize)]
struct TokenizeRequest<'a> {
    content: &'a str,
    add_special: bool,
    parse_special: bool,
}

#[derive(Deserialize)]
struct TokenizeResponse {
    tokens: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct ChatStreamResponse {
    #[serde(default)]
    choices: Vec<ChatStreamChoice>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ChatStreamChoice {
    delta: ChatStreamDelta,
}

#[derive(Deserialize)]
struct ChatStreamDelta {
    content: Option<String>,
}

struct StreamedUnit {
    content: String,
}

async fn wait_until_cancelled(should_continue: &impl Fn() -> bool) {
    while should_continue() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn process_stream_line(
    line: &str,
    content: &mut String,
    started: Instant,
    on_partial: &mut impl FnMut(&str, u64) -> bool,
) -> Result<bool> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return Ok(false);
    }
    let Some(payload) = line.strip_prefix("data:") else {
        return Ok(false);
    };
    let payload = payload.trim();
    if payload == "[DONE]" {
        return Ok(true);
    }
    let chunk: ChatStreamResponse =
        serde_json::from_str(payload).context("local translation stream returned invalid JSON")?;
    if let Some(error) = chunk.error {
        bail!("local translation stream failed: {error}");
    }
    for choice in chunk.choices {
        let Some(delta) = choice.delta.content else {
            continue;
        };
        if delta.is_empty() {
            continue;
        }
        content.push_str(&delta);
        let elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        let visible = strip_reasoning(content);
        if !visible.is_empty() && !on_partial(visible, elapsed_ms) {
            return Err(TranslationCancelled.into());
        }
    }
    Ok(false)
}

fn preferred_vulkan_device(executable: &Path, runtime_root: &Path) -> Option<String> {
    let mut command = Command::new(executable);
    command
        .arg("--list-devices")
        .current_dir(runtime_root)
        .env("PATH", runtime_root);
    configure_hidden_process(&mut command);
    let output = command.output().ok()?;
    let listing = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let devices = listing
        .lines()
        .filter_map(|line| {
            let (id, name) = line.trim().split_once(':')?;
            id.starts_with("Vulkan")
                .then(|| (id.to_owned(), name.trim().to_owned()))
        })
        .collect::<Vec<_>>();
    devices
        .iter()
        .find(|(_, name)| {
            let name = name.to_ascii_lowercase();
            !name.contains("intel(r) uhd")
                && !name.contains("intel uhd")
                && !name.contains("intel(r) iris")
                && !name.contains("intel iris")
        })
        .or_else(|| devices.first())
        .map(|(id, _)| id.clone())
}

fn open_server_log(runtime_root: &Path) -> io::Result<File> {
    let log_directory = runtime_root.join("logs");
    fs::create_dir_all(&log_directory)?;
    let current = log_directory.join("translation-server.log");
    if current
        .metadata()
        .is_ok_and(|metadata| metadata.len() >= SERVER_LOG_LIMIT_BYTES)
    {
        let previous = log_directory.join("translation-server.previous.log");
        if previous.exists() {
            fs::remove_file(&previous)?;
        }
        fs::rename(&current, previous)?;
    }
    OpenOptions::new().create(true).append(true).open(current)
}

#[cfg(windows)]
fn configure_hidden_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_hidden_process(_command: &mut Command) {}

fn language_name(language: TranslationLanguage) -> &'static str {
    match language {
        TranslationLanguage::English => "English",
        TranslationLanguage::Russian => "Russian",
    }
}

fn translation_instruction(source: TranslationLanguage, target: TranslationLanguage) -> String {
    format!(
        "Translate the user text naturally and faithfully from {} into {}. Preserve the exact \
         meaning, tone, terminology, paragraph breaks, and line breaks; do not omit or add \
         information. Return only the translation.",
        language_name(source),
        language_name(target)
    )
}

fn strip_reasoning(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed
        .rfind("</think>")
        .map_or(trimmed, |end| trimmed[end + "</think>".len()..].trim())
}

struct PlannedTranslation {
    text: String,
    first_token_duration_ms: u64,
    backend_request_count: u32,
}

fn translate_planned(
    runtime: &mut LlamaRuntime,
    text: &str,
    source: TranslationLanguage,
    target: TranslationLanguage,
    should_continue: &impl Fn() -> bool,
    on_partial: &mut impl FnMut(&str, u64) -> bool,
) -> Result<PlannedTranslation> {
    let started = Instant::now();
    let chunks = plan_translation_chunks(runtime, text, should_continue)?;
    let mut output = String::with_capacity(text.len());
    let mut first_token_duration_ms = None;
    let mut backend_request_count = 0_u32;
    for chunk in chunks {
        if !should_continue() {
            return Err(TranslationCancelled.into());
        }
        let leading_bytes = chunk.len() - chunk.trim_start().len();
        let trailing_bytes = chunk.len() - chunk.trim_end().len();
        let body_end = chunk.len().saturating_sub(trailing_bytes);
        let leading = &chunk[..leading_bytes];
        let body = &chunk[leading_bytes..body_end];
        let trailing = &chunk[body_end..];
        output.push_str(leading);
        if body.is_empty() {
            output.push_str(trailing);
            continue;
        }

        backend_request_count = backend_request_count.saturating_add(1);
        let completed_prefix = output.clone();
        let streamed = runtime.translate_unit_streaming(
            body,
            source,
            target,
            should_continue,
            |partial, _unit_elapsed_ms| {
                let elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
                first_token_duration_ms.get_or_insert(elapsed_ms);
                let mut visible = String::with_capacity(completed_prefix.len() + partial.len());
                visible.push_str(&completed_prefix);
                visible.push_str(partial);
                on_partial(&visible, elapsed_ms)
            },
        )?;
        first_token_duration_ms
            .get_or_insert_with(|| started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
        output.push_str(streamed.content.trim());
        output.push_str(trailing);
    }
    if output.trim().is_empty() {
        bail!("local translation runtime returned an empty result");
    }
    Ok(PlannedTranslation {
        text: output,
        first_token_duration_ms: first_token_duration_ms.unwrap_or(0),
        backend_request_count,
    })
}

fn plan_translation_chunks(
    runtime: &LlamaRuntime,
    text: &str,
    should_continue: &impl Fn() -> bool,
) -> Result<Vec<String>> {
    if !should_continue() {
        return Err(TranslationCancelled.into());
    }
    let total_tokens = runtime.token_count(text)?;
    if !should_continue() {
        return Err(TranslationCancelled.into());
    }
    if total_tokens <= MAX_SOURCE_TOKENS_PER_REQUEST {
        return Ok(vec![text.to_owned()]);
    }

    let total_characters = text.chars().count().max(1);
    let estimated_characters = total_characters
        .saturating_mul(MAX_SOURCE_TOKENS_PER_REQUEST)
        .checked_div(total_tokens.max(1))
        .unwrap_or(total_characters);
    let mut character_budget = estimated_characters.saturating_mul(9) / 10;
    character_budget = character_budget.clamp(256, total_characters);
    let mut chunks = split_at_natural_boundaries(text, character_budget);

    let mut index = 0;
    while index < chunks.len() {
        if !should_continue() {
            return Err(TranslationCancelled.into());
        }
        let tokens = runtime.token_count(&chunks[index])?;
        if tokens <= MAX_SOURCE_TOKENS_PER_REQUEST {
            index += 1;
            continue;
        }
        let characters = chunks[index].chars().count();
        let reduced_budget = characters
            .saturating_mul(MAX_SOURCE_TOKENS_PER_REQUEST)
            .checked_div(tokens.max(1))
            .unwrap_or(characters)
            .saturating_mul(9)
            / 10;
        let replacement =
            split_at_natural_boundaries(&chunks[index], reduced_budget.clamp(64, characters));
        if replacement.len() <= 1 {
            bail!(
                "translation chunk cannot be reduced below the {MAX_SOURCE_TOKENS_PER_REQUEST}-token budget"
            );
        }
        chunks.splice(index..=index, replacement);
    }
    Ok(chunks)
}

fn split_at_natural_boundaries(text: &str, max_characters: usize) -> Vec<String> {
    if text.chars().count() <= max_characters {
        return vec![text.to_owned()];
    }
    let mut chunks = Vec::new();
    let mut remainder = text;
    while remainder.chars().count() > max_characters {
        let hard_end = remainder
            .char_indices()
            .nth(max_characters)
            .map_or(remainder.len(), |(index, _)| index);
        let search = &remainder[..hard_end];
        let minimum = search
            .char_indices()
            .nth(max_characters / 2)
            .map_or(0, |(index, _)| index);
        let boundary = [
            search.rfind("\n\n").map(|index| index + 2),
            search.rfind('\n').map(|index| index + 1),
            search.rfind(". ").map(|index| index + 2),
            search.rfind("! ").map(|index| index + 2),
            search.rfind("? ").map(|index| index + 2),
            search.rfind(char::is_whitespace).map(|index| index + 1),
        ]
        .into_iter()
        .flatten()
        .find(|boundary| *boundary >= minimum)
        .unwrap_or(hard_end);
        if boundary == 0 {
            break;
        }
        chunks.push(remainder[..boundary].to_owned());
        remainder = &remainder[boundary..];
    }
    if !remainder.is_empty() {
        chunks.push(remainder.to_owned());
    }
    chunks
}

fn normalize_single_word_repetition(source: &str, translated: &str) -> String {
    let source_words = lexical_spans(source);
    if source_words.len() != 1 {
        return translated.to_owned();
    }
    let translated_words = lexical_spans(translated);
    if translated_words.len() != 2 {
        return translated.to_owned();
    }
    let first = &translated[translated_words[0].0..translated_words[0].1];
    let second = &translated[translated_words[1].0..translated_words[1].1];
    if first.to_lowercase() != second.to_lowercase() {
        return translated.to_owned();
    }

    let mut normalized = String::with_capacity(translated.len());
    normalized.push_str(&translated[..translated_words[0].0]);
    normalized.push_str(first);
    normalized.push_str(&translated[translated_words[1].1..]);
    normalized
}

fn normalize_line_endings(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

fn lexical_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() {
            start.get_or_insert(index);
        } else if let Some(word_start) = start.take() {
            spans.push((word_start, index));
        }
    }
    if let Some(word_start) = start {
        spans.push((word_start, text.len()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use rust_protocol::TranslationLanguage;

    use super::{
        detect_language, normalize_line_endings, normalize_single_word_repetition,
        split_at_natural_boundaries, strip_reasoning,
    };

    #[test]
    fn language_detection_is_deterministic_for_the_supported_pair() {
        assert_eq!(
            detect_language("Это русский текст"),
            TranslationLanguage::Russian
        );
        assert_eq!(
            detect_language("This is English text"),
            TranslationLanguage::English
        );
        assert_eq!(
            detect_language("Версия Rust 1.88"),
            TranslationLanguage::Russian
        );
    }

    #[test]
    fn long_text_is_split_without_dropping_words() {
        let input = "one two three four five";
        let chunks = split_at_natural_boundaries(input, 8);
        assert_eq!(chunks.concat(), input);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 8));
    }

    #[test]
    fn natural_split_prefers_existing_line_boundaries() {
        let input = "first line\nsecond line\nthird line";
        let chunks = split_at_natural_boundaries(input, 18);
        assert_eq!(chunks.concat(), input);
        assert_eq!(chunks[0], "first line\n");
    }

    #[test]
    fn windows_provider_line_endings_are_normalized_for_the_model() {
        assert_eq!(
            normalize_line_endings("first\rsecond\r\nthird"),
            "first\nsecond\nthird"
        );
    }

    #[test]
    fn repeated_single_word_translation_is_collapsed_without_losing_terminal_punctuation() {
        assert_eq!(
            normalize_single_word_repetition("hello", "Привет, привет."),
            "Привет."
        );
        assert_eq!(
            normalize_single_word_repetition("TEXT", "TEXT TEXT"),
            "TEXT"
        );
        assert_eq!(
            normalize_single_word_repetition("текст", "text-text"),
            "text"
        );
    }

    #[test]
    fn repetition_normalization_never_changes_multiword_or_distinct_results() {
        assert_eq!(
            normalize_single_word_repetition("hello world", "Привет, привет."),
            "Привет, привет."
        );
        assert_eq!(
            normalize_single_word_repetition("hello", "Добрый день"),
            "Добрый день"
        );
        assert_eq!(
            normalize_single_word_repetition("hello", "Привет, привет, привет."),
            "Привет, привет, привет."
        );
    }

    #[test]
    fn reasoning_is_never_exposed_as_translation() {
        assert_eq!(strip_reasoning("<think>hidden</think>\nПеревод"), "Перевод");
        assert_eq!(strip_reasoning("Перевод"), "Перевод");
    }
}
