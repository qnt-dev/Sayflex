use std::{
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
const CHUNK_CHARACTERS: usize = 2_000;
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(90);
const SERVER_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SERVER_LOG_LIMIT_BYTES: u64 = 2 * 1024 * 1024;

pub struct TranslationEngine {
    pack: TranslationPackDescriptor,
    model_root: PathBuf,
    runtime_root: PathBuf,
    runtime: Option<LlamaRuntime>,
}

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

        self.ensure_runtime()?;
        let first_attempt = translate_preserving_lines(
            self.runtime
                .as_mut()
                .context("translation runtime failed to initialize")?,
            text,
            source,
            target,
        );
        let translated = match first_attempt {
            Ok(translated) => translated,
            Err(first_error) => {
                // A crashed backend is restarted once in-process. The caller still
                // receives a normal error if the deterministic retry also fails.
                self.runtime = None;
                self.ensure_runtime()
                    .context("translation backend restart failed")?;
                translate_preserving_lines(
                    self.runtime
                        .as_mut()
                        .context("translation runtime failed to restart")?,
                    text,
                    source,
                    target,
                )
                .with_context(|| format!("translation retry failed after: {first_error:#}"))?
            }
        };
        let translated = normalize_single_word_repetition(text, &translated);
        Ok((translated, source, target))
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
        let endpoint = format!("http://127.0.0.1:{port}");
        let mut runtime = Self {
            child,
            client,
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
                    content: format!(
                        "Translate the user text naturally and faithfully from {} into {}. \
                         Preserve the exact meaning, tone, and terminology; do not omit or add \
                         information. Return only the translation.",
                        language_name(source),
                        language_name(target)
                    ),
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

fn strip_reasoning(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed
        .rfind("</think>")
        .map_or(trimmed, |end| trimmed[end + "</think>".len()..].trim())
}

fn translate_preserving_lines(
    runtime: &mut LlamaRuntime,
    text: &str,
    source: TranslationLanguage,
    target: TranslationLanguage,
) -> Result<String> {
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (content, newline) = line.strip_suffix('\n').map_or((line, ""), |content| {
            (content.strip_suffix('\r').unwrap_or(content), "\n")
        });
        if content.trim().is_empty() {
            output.push_str(content);
            output.push_str(newline);
            continue;
        }
        let leading = content.len() - content.trim_start().len();
        let trailing = content.len() - content.trim_end().len();
        output.push_str(&content[..leading]);
        let body_end = content.len().saturating_sub(trailing);
        let body = &content[leading..body_end];
        let chunks = split_chunks(body, CHUNK_CHARACTERS);
        for (index, chunk) in chunks.iter().enumerate() {
            if index > 0 {
                output.push(' ');
            }
            output.push_str(runtime.translate_unit(chunk.trim(), source, target)?.trim());
        }
        if trailing > 0 {
            output.push_str(&content[body_end..]);
        }
        output.push_str(newline);
    }
    Ok(output)
}

fn split_chunks(text: &str, max_characters: usize) -> Vec<String> {
    if text.chars().count() <= max_characters {
        return vec![text.to_owned()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let additional = word.chars().count() + usize::from(!current.is_empty());
        if !current.is_empty() && current.chars().count() + additional > max_characters {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        chunks.push(current);
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

    use super::{detect_language, normalize_single_word_repetition, split_chunks, strip_reasoning};

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
        let chunks = split_chunks(input, 8);
        assert_eq!(chunks.join(" "), input);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 8));
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
