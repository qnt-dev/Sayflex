use std::{env, path::PathBuf, time::Instant};

use anyhow::{Context, Result, anyhow, bail};
use rust_model_manager::{BundleInstaller, HttpFetcher};
use rust_platform_windows::ProcessResourceSampler;
use rust_protocol::{
    BlockCompletion, GlossaryEntry, GlossaryEntryId, GlossaryOrigin, PhysicalRect, ScreenBlockId,
    ScreenTextBlock, SourceContext, TranslationLanguage, TranslationTier,
};
use rust_translation_engine::{
    TranslationBackendDescriptor, TranslationCancelled, TranslationCatalog, TranslationEngine,
    TranslationPackDescriptor,
};
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::Threading::{GetCurrentProcess, GetProcessTimes},
};

const DEFAULT_PACK_ID: &str = "firefox-ru-en-v3.1";
const HY_MT2_CANDIDATE_ID: &str = "hy-mt2-1.8b-q4-candidate";

#[derive(Debug, Clone, Copy)]
struct CorpusCase {
    id: &'static str,
    source: &'static str,
    source_language: TranslationLanguage,
    target_language: TranslationLanguage,
}

const CORPUS: &[CorpusCase] = &[
    CorpusCase {
        id: "en_ui",
        source: "Translation completed locally.",
        source_language: TranslationLanguage::English,
        target_language: TranslationLanguage::Russian,
    },
    CorpusCase {
        id: "en_sentence",
        source: "SayFlex keeps your text on this computer and does not send it to a service.",
        source_language: TranslationLanguage::English,
        target_language: TranslationLanguage::Russian,
    },
    CorpusCase {
        id: "en_names_numbers_url",
        source: "Alex sent 24 files to https://example.com/docs at 18:30 on 21 September 2026.",
        source_language: TranslationLanguage::English,
        target_language: TranslationLanguage::Russian,
    },
    CorpusCase {
        id: "en_paragraphs",
        source: "The first paragraph checks punctuation and names.\n\nThe second paragraph must arrive without being cut off.",
        source_language: TranslationLanguage::English,
        target_language: TranslationLanguage::Russian,
    },
    CorpusCase {
        id: "ru_ui",
        source: "Перевод выполнен локально.",
        source_language: TranslationLanguage::Russian,
        target_language: TranslationLanguage::English,
    },
    CorpusCase {
        id: "ru_sentence",
        source: "SayFlex хранит ваш текст на этом компьютере и не отправляет его в сервис.",
        source_language: TranslationLanguage::Russian,
        target_language: TranslationLanguage::English,
    },
    CorpusCase {
        id: "ru_names_numbers_url",
        source: "Алексей отправил 24 файла на https://example.com/docs в 18:30 21 сентября 2026 года.",
        source_language: TranslationLanguage::Russian,
        target_language: TranslationLanguage::English,
    },
    CorpusCase {
        id: "ru_paragraphs",
        source: "Первый абзац проверяет пунктуацию и имена.\n\nВторой абзац должен прийти без обрезки.",
        source_language: TranslationLanguage::Russian,
        target_language: TranslationLanguage::English,
    },
];

#[derive(Debug)]
struct Args {
    pack_root: PathBuf,
    runtime_root: PathBuf,
    pack_id: String,
    hy_mt2_model: Option<PathBuf>,
    install: bool,
    include_text: bool,
}

struct SelectedPack {
    descriptor: TranslationPackDescriptor,
    model_root: PathBuf,
    runtime_root: PathBuf,
    download_bytes: u64,
    installed_bytes: u64,
}

#[derive(Debug)]
struct CaseResult {
    case: CorpusCase,
    output: String,
    elapsed_ms: u64,
    first_result_ms: u64,
}

#[derive(Debug)]
struct StructuredResult {
    elapsed_ms: u64,
    translated_text: String,
    completion: BlockCompletion,
    glossary_adhered: bool,
}

#[derive(Debug)]
struct Report {
    pack_id: String,
    pack_root: PathBuf,
    download_bytes: u64,
    installed_bytes: u64,
    cold_load_ms: u64,
    warm_total_ms: u64,
    cpu_ms: u64,
    working_set_before: u64,
    working_set_after: u64,
    gpu_percent: Option<f32>,
    vram_bytes: Option<u64>,
    validation_error: Option<String>,
    cancellation_passed: bool,
    structured: StructuredResult,
    results: Vec<CaseResult>,
}

fn main() -> Result<()> {
    let args = parse_args(env::args().skip(1))?;
    let catalog = TranslationCatalog::load()?;
    let selected = select_pack(&args, &catalog)?;

    let pid = std::process::id();
    let mut resources = ProcessResourceSampler::new();
    let before = resources
        .sample_groups(&[pid])
        .map_err(|error| anyhow!(error))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("resource sampler returned no process group"))?;
    let cpu_before = process_cpu_ticks()?;

    let mut engine = TranslationEngine::new(
        selected.descriptor.clone(),
        selected.model_root.clone(),
        selected.runtime_root,
    );
    let cold_started = Instant::now();
    engine.warm().context("warm compact translation pack")?;
    let cold_load_ms = elapsed_ms(cold_started);

    let warm_started = Instant::now();
    let mut results = Vec::with_capacity(CORPUS.len());
    for case in CORPUS {
        let started = Instant::now();
        let output = engine
            .translate_streaming(
                case.source,
                Some(case.source_language),
                Some(case.target_language),
                || true,
                |_, _| true,
            )
            .with_context(|| format!("translate corpus case {}", case.id))?;
        results.push(CaseResult {
            case: *case,
            output: output.text,
            elapsed_ms: elapsed_ms(started),
            first_result_ms: output.first_token_duration_ms,
        });
    }
    let warm_total_ms = elapsed_ms(warm_started);
    let cancellation_passed = cancellation_probe(&mut engine);
    let structured = structured_probe(&mut engine)?;
    let validation_error = validate_report(&results, cancellation_passed, &structured)
        .err()
        .map(|error| format!("{error:#}"));

    let cpu_ms = process_cpu_ticks()?
        .saturating_sub(cpu_before)
        .saturating_div(10_000);
    let after = resources
        .sample_groups(&[pid])
        .map_err(|error| anyhow!(error))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("resource sampler returned no process group"))?;
    let report = Report {
        pack_id: selected.descriptor.id,
        pack_root: selected.model_root,
        download_bytes: selected.download_bytes,
        installed_bytes: selected.installed_bytes,
        cold_load_ms,
        warm_total_ms,
        cpu_ms,
        working_set_before: before.ram_bytes,
        working_set_after: after.ram_bytes,
        gpu_percent: after.gpu_percent,
        vram_bytes: after.vram_bytes,
        validation_error,
        cancellation_passed,
        structured,
        results,
    };
    print!("{}", render_report(&report, args.include_text));
    if let Some(error) = &report.validation_error {
        bail!("benchmark validation failed: {error}");
    }
    Ok(())
}

fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut pack_root = rust_app_core::product::data_root().join("translation-models");
    let mut runtime_root = rust_app_core::product::data_root()
        .join("runtimes")
        .join("llama-b10164-vulkan");
    let mut pack_id = DEFAULT_PACK_ID.to_owned();
    let mut hy_mt2_model = None;
    let mut install = false;
    let mut include_text = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--install" => install = true,
            "--include-text" => include_text = true,
            "--pack-id" => {
                pack_id = arguments
                    .next()
                    .ok_or_else(|| anyhow!("--pack-id requires an id"))?;
            }
            "--pack-root" => {
                pack_root = arguments
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow!("--pack-root requires a path"))?;
            }
            "--runtime-root" => {
                runtime_root = arguments
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow!("--runtime-root requires a path"))?;
            }
            "--hy-mt2-model" => {
                hy_mt2_model = Some(
                    arguments
                        .next()
                        .map(PathBuf::from)
                        .ok_or_else(|| anyhow!("--hy-mt2-model requires a GGUF path"))?,
                );
            }
            _ => bail!("unknown argument: {argument}"),
        }
    }
    Ok(Args {
        pack_root,
        runtime_root,
        pack_id,
        hy_mt2_model,
        install,
        include_text,
    })
}

fn select_pack(args: &Args, catalog: &TranslationCatalog) -> Result<SelectedPack> {
    if let Some(model_path) = &args.hy_mt2_model {
        if args.install {
            bail!("--install cannot be combined with --hy-mt2-model");
        }
        let metadata = model_path.metadata().with_context(|| {
            format!("read candidate model metadata at {}", model_path.display())
        })?;
        let model_file = model_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("candidate model file name is not valid UTF-8"))?
            .to_owned();
        let model_root = model_path
            .parent()
            .ok_or_else(|| anyhow!("candidate model path has no parent directory"))?
            .to_owned();
        require_llama_runtime(&args.runtime_root)?;
        return Ok(SelectedPack {
            descriptor: TranslationPackDescriptor {
                id: HY_MT2_CANDIDATE_ID.to_owned(),
                display_name: "HY-MT2 1.8B Q4 candidate".to_owned(),
                description: "External benchmark candidate; never added to the product catalog automatically."
                    .to_owned(),
                tier: TranslationTier::Tiny,
                license: "Apache-2.0".to_owned(),
                recommended_locales: vec!["ru".to_owned()],
                backend: TranslationBackendDescriptor::LlamaCpp {
                    model_file,
                    context_size: 4_096,
                    max_output_tokens: 2_048,
                },
                artifacts: Vec::new(),
                languages: vec![TranslationLanguage::English, TranslationLanguage::Russian],
            },
            model_root,
            runtime_root: args.runtime_root.clone(),
            download_bytes: metadata.len(),
            installed_bytes: metadata.len(),
        });
    }

    let pack = catalog.by_id(&args.pack_id).cloned().ok_or_else(|| {
        anyhow!(
            "translation pack {} is absent from the built-in catalog",
            args.pack_id
        )
    })?;
    let bundle = pack.as_bundle();
    let installer = BundleInstaller::new(args.pack_root.clone());
    if args.install {
        let fetcher = HttpFetcher::new("translation-benchmark")?;
        let mut last_percent = None;
        installer
            .ensure_installed(&bundle, &fetcher, |progress| {
                if let Some(percent) = progress
                    .bytes_received
                    .saturating_mul(100)
                    .checked_div(progress.total_bytes)
                    && last_percent != Some(percent)
                {
                    eprint!("\rInstalling {}: {percent:>3}%", pack.id);
                    last_percent = Some(percent);
                }
            })
            .with_context(|| format!("install translation pack {}", pack.id))?;
        eprintln!();
    }
    if !installer.is_installed(&bundle)? {
        bail!(
            "translation pack {} is not installed at {}; rerun with --install",
            pack.id,
            args.pack_root.display()
        );
    }
    if pack.backend.requires_llama_runtime() {
        require_llama_runtime(&args.runtime_root)?;
    }
    Ok(SelectedPack {
        descriptor: pack,
        model_root: installer.bundle_path(&bundle),
        runtime_root: args.runtime_root.clone(),
        download_bytes: bundle.total_bytes(),
        installed_bytes: bundle.installed_bytes(),
    })
}

fn require_llama_runtime(runtime_root: &std::path::Path) -> Result<()> {
    let executable = runtime_root.join("llama-server.exe");
    if !executable.is_file() {
        bail!(
            "llama.cpp runtime is absent at {}; pass --runtime-root",
            runtime_root.display()
        );
    }
    Ok(())
}

fn validate_complete(results: &[CaseResult]) -> Result<()> {
    let directions = [
        (TranslationLanguage::English, TranslationLanguage::Russian),
        (TranslationLanguage::Russian, TranslationLanguage::English),
    ];
    for (source, target) in directions {
        let directional = results
            .iter()
            .filter(|result| {
                result.case.source_language == source && result.case.target_language == target
            })
            .collect::<Vec<_>>();
        if directional.is_empty() {
            bail!("benchmark has no completed {source:?} to {target:?} direction");
        }
        for result in directional {
            if result.output.trim().is_empty() {
                bail!("{} returned an empty translation", result.case.id);
            }
            let source_paragraphs = result.case.source.split("\n\n").count();
            let output_paragraphs = result.output.split("\n\n").count();
            if source_paragraphs != output_paragraphs {
                bail!(
                    "{} returned {output_paragraphs} of {source_paragraphs} paragraphs",
                    result.case.id
                );
            }
            for protected in protected_values(result.case.id) {
                if !result.output.contains(protected) {
                    bail!("{} did not preserve {protected}", result.case.id);
                }
            }
        }
    }
    Ok(())
}

fn protected_values(case_id: &str) -> &'static [&'static str] {
    match case_id {
        "en_names_numbers_url" | "ru_names_numbers_url" => {
            &["24", "https://example.com/docs", "18:30", "2026"]
        }
        _ => &[],
    }
}

fn cancellation_probe(engine: &mut TranslationEngine) -> bool {
    let started = Instant::now();
    engine
        .translate_streaming(
            "This deliberately long translation request must be cancelled before it can publish \
             a result. The benchmark checks that cancellation remains responsive while the local \
             runtime is active.",
            Some(TranslationLanguage::English),
            Some(TranslationLanguage::Russian),
            || started.elapsed() < std::time::Duration::from_millis(1),
            |_, _| true,
        )
        .is_err_and(|error| error.downcast_ref::<TranslationCancelled>().is_some())
}

fn structured_probe(engine: &mut TranslationEngine) -> Result<StructuredResult> {
    let block = ScreenTextBlock {
        block_id: ScreenBlockId::from("glossary-probe"),
        source_text: "The cooldown is reduced by 2 seconds.".to_owned(),
        bounds: PhysicalRect {
            x: 0,
            y: 0,
            width: 480,
            height: 48,
        },
        confidence_milli: 1_000,
    };
    let context = SourceContext {
        application_id: Some("dota-2".to_owned()),
        executable_name: Some("dota2.exe".to_owned()),
        window_title: Some("Dota 2".to_owned()),
        domain: Some("Dota 2".to_owned()),
        ..SourceContext::default()
    };
    let glossary = GlossaryEntry {
        entry_id: GlossaryEntryId::from("cooldown-ru"),
        source_phrase: "cooldown".to_owned(),
        approved_translation: "время перезарядки".to_owned(),
        source_language: TranslationLanguage::English,
        target_language: TranslationLanguage::Russian,
        examples: vec!["Reduces the cooldown by 2 seconds.".to_owned()],
        context: context.clone(),
        origin: GlossaryOrigin::Confirmed,
        created_unix_ms: 0,
        updated_unix_ms: 0,
    };
    let started = Instant::now();
    let output = engine.translate_blocks(
        &[block],
        &context,
        &[glossary],
        Some(TranslationLanguage::English),
        Some(TranslationLanguage::Russian),
        &|| true,
    )?;
    let translated = output
        .blocks
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("structured glossary probe returned no block"))?;
    let normalized = translated.translated_text.to_lowercase();
    Ok(StructuredResult {
        elapsed_ms: elapsed_ms(started),
        glossary_adhered: normalized.contains("время перезарядки"),
        translated_text: translated.translated_text,
        completion: translated.completion,
    })
}

fn validate_report(
    results: &[CaseResult],
    cancellation_passed: bool,
    structured: &StructuredResult,
) -> Result<()> {
    validate_complete(results)?;
    if !cancellation_passed {
        bail!("in-flight cancellation was not reported as cancelled");
    }
    if structured.completion != BlockCompletion::Completed {
        bail!("structured glossary probe was not completed");
    }
    if !structured.glossary_adhered {
        bail!("structured glossary probe ignored the approved translation");
    }
    Ok(())
}

fn render_report(report: &Report, include_text: bool) -> String {
    let ram_delta = report
        .working_set_after
        .saturating_sub(report.working_set_before);
    let mut output = format!(
        "pack={}\npack_root={}\ndownload_bytes={}\ninstalled_bytes={}\ncold_load_ms={}\nwarm_cases={}\nwarm_total_ms={}\nprocess_cpu_ms={}\nworking_set_before_bytes={}\nworking_set_after_bytes={}\nworking_set_delta_bytes={}\ngpu_percent={}\nvram_bytes={}\ncancellation={}\nstructured_elapsed_ms={}\nstructured_completion={:?}\nglossary_adhered={}\nvalidation={}\n",
        report.pack_id,
        report.pack_root.display(),
        report.download_bytes,
        report.installed_bytes,
        report.cold_load_ms,
        report.results.len(),
        report.warm_total_ms,
        report.cpu_ms,
        report.working_set_before,
        report.working_set_after,
        ram_delta,
        optional_number(report.gpu_percent.map(f64::from)),
        optional_number(report.vram_bytes.map(|value| value as f64)),
        if report.cancellation_passed {
            "passed"
        } else {
            "failed"
        },
        report.structured.elapsed_ms,
        report.structured.completion,
        report.structured.glossary_adhered,
        report.validation_error.as_deref().unwrap_or("passed"),
    );
    if include_text {
        output.push_str(&format!(
            "structured_translation={:?}\n",
            report.structured.translated_text
        ));
    }
    for result in &report.results {
        output.push_str(&format!(
            "case={} direction={:?}->{:?} elapsed_ms={} first_result_ms={} output_chars={}\n",
            result.case.id,
            result.case.source_language,
            result.case.target_language,
            result.elapsed_ms,
            result.first_result_ms,
            result.output.chars().count(),
        ));
        if include_text {
            output.push_str(&format!(
                "source={:?}\ntranslation={:?}\n",
                result.case.source, result.output
            ));
        }
    }
    output
}

fn optional_number(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "not_available".to_owned())
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn process_cpu_ticks() -> Result<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: GetCurrentProcess returns a pseudo handle valid in this process;
    // all four FILETIME pointers are initialized and writable for the call.
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        bail!("could not read benchmark process CPU time");
    }
    Ok(filetime_ticks(kernel).saturating_add(filetime_ticks(user)))
}

fn filetime_ticks(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(case: CorpusCase, output: &str) -> CaseResult {
        CaseResult {
            case,
            output: output.to_owned(),
            elapsed_ms: 12,
            first_result_ms: 8,
        }
    }

    #[test]
    fn default_report_never_prints_corpus_text() {
        let report = Report {
            pack_id: DEFAULT_PACK_ID.to_owned(),
            pack_root: PathBuf::from("models"),
            download_bytes: 1,
            installed_bytes: 2,
            cold_load_ms: 3,
            warm_total_ms: 4,
            cpu_ms: 5,
            working_set_before: 6,
            working_set_after: 7,
            gpu_percent: Some(0.0),
            vram_bytes: Some(0),
            validation_error: None,
            cancellation_passed: true,
            structured: StructuredResult {
                elapsed_ms: 8,
                translated_text: "Время перезарядки уменьшено на 2 секунды.".to_owned(),
                completion: BlockCompletion::Completed,
                glossary_adhered: true,
            },
            results: vec![result(CORPUS[0], "Перевод выполнен локально.")],
        };
        let rendered = render_report(&report, false);
        assert!(!rendered.contains(CORPUS[0].source));
        assert!(!rendered.contains("Перевод выполнен локально."));
        assert!(rendered.contains("case=en_ui"));
        assert!(rendered.contains("pack=firefox-ru-en-v3.1"));
    }

    #[test]
    fn incomplete_or_one_way_results_are_rejected() {
        assert!(validate_complete(&[result(CORPUS[0], "Готово.")]).is_err());
        let mut both = vec![
            result(CORPUS[0], "Готово."),
            result(CORPUS[4], "Completed."),
        ];
        assert!(validate_complete(&both).is_ok());
        both[1].output.clear();
        assert!(validate_complete(&both).is_err());
    }

    #[test]
    fn a_multiline_result_must_keep_every_paragraph() {
        let mut both = vec![
            result(CORPUS[3], "Первый абзац. Второй абзац."),
            result(CORPUS[4], "Completed."),
        ];
        assert!(validate_complete(&both).is_err());
        both[0].output = "Первый абзац.\n\nВторой абзац.".to_owned();
        assert!(validate_complete(&both).is_ok());
    }

    #[test]
    fn protected_values_must_survive_translation() {
        let mut both = vec![
            result(CORPUS[2], "Алекс отправил файлы."),
            result(CORPUS[4], "Completed."),
        ];
        assert!(validate_complete(&both).is_err());
        both[0].output =
            "Алекс отправил 24 файла на https://example.com/docs в 18:30 21 сентября 2026 года."
                .to_owned();
        assert!(validate_complete(&both).is_ok());
    }

    #[test]
    fn candidate_mode_is_explicit_and_never_enables_install() {
        let parsed = parse_args([
            "--hy-mt2-model".to_owned(),
            "candidate.gguf".to_owned(),
            "--runtime-root".to_owned(),
            "runtime".to_owned(),
        ])
        .unwrap();
        assert_eq!(parsed.hy_mt2_model, Some(PathBuf::from("candidate.gguf")));
        assert_eq!(parsed.runtime_root, PathBuf::from("runtime"));
        assert!(!parsed.install);
    }
}
