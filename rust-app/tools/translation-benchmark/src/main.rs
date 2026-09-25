use std::{env, path::PathBuf, time::Instant};

use anyhow::{Context, Result, anyhow, bail};
use rust_model_manager::{BundleInstaller, HttpFetcher};
use rust_platform_windows::ProcessResourceSampler;
use rust_protocol::{TranslationLanguage, TranslationTier};
use rust_translation_engine::{TranslationCatalog, TranslationEngine};
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::Threading::{GetCurrentProcess, GetProcessTimes},
};

const PACK_ID: &str = "firefox-ru-en-v3.1";

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
    install: bool,
    include_text: bool,
}

#[derive(Debug)]
struct CaseResult {
    case: CorpusCase,
    output: String,
    elapsed_ms: u64,
    first_result_ms: u64,
}

#[derive(Debug)]
struct Report {
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
    results: Vec<CaseResult>,
}

fn main() -> Result<()> {
    let args = parse_args(env::args().skip(1))?;
    let catalog = TranslationCatalog::load()?;
    let pack = catalog
        .by_tier(TranslationTier::Compact)
        .filter(|candidate| candidate.id == PACK_ID)
        .cloned()
        .ok_or_else(|| anyhow!("compact translation pack is absent from the built-in catalog"))?;
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
                    eprint!("\rInstalling compact translation pack: {percent:>3}%");
                    last_percent = Some(percent);
                }
            })
            .context("install compact translation pack")?;
        eprintln!();
    }
    if !installer.is_installed(&bundle)? {
        bail!(
            "compact translation pack is not installed at {}; rerun with --install",
            args.pack_root.display()
        );
    }

    let pid = std::process::id();
    let mut resources = ProcessResourceSampler::new();
    let before = resources
        .sample_groups(&[pid])
        .map_err(|error| anyhow!(error))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("resource sampler returned no process group"))?;
    let cpu_before = process_cpu_ticks()?;

    let mut engine = TranslationEngine::new(pack, installer.bundle_path(&bundle), PathBuf::new());
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
    validate_complete(&results)?;

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
        pack_root: args.pack_root,
        download_bytes: bundle.total_bytes(),
        installed_bytes: bundle.installed_bytes(),
        cold_load_ms,
        warm_total_ms,
        cpu_ms,
        working_set_before: before.ram_bytes,
        working_set_after: after.ram_bytes,
        gpu_percent: after.gpu_percent,
        vram_bytes: after.vram_bytes,
        results,
    };
    print!("{}", render_report(&report, args.include_text));
    Ok(())
}

fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut pack_root = rust_app_core::product::data_root().join("translation-models");
    let mut install = false;
    let mut include_text = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--install" => install = true,
            "--include-text" => include_text = true,
            "--pack-root" => {
                pack_root = arguments
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow!("--pack-root requires a path"))?;
            }
            _ => bail!("unknown argument: {argument}"),
        }
    }
    Ok(Args {
        pack_root,
        install,
        include_text,
    })
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
        }
    }
    Ok(())
}

fn render_report(report: &Report, include_text: bool) -> String {
    let ram_delta = report
        .working_set_after
        .saturating_sub(report.working_set_before);
    let mut output = format!(
        "pack={PACK_ID}\npack_root={}\ndownload_bytes={}\ninstalled_bytes={}\ncold_load_ms={}\nwarm_cases={}\nwarm_total_ms={}\nprocess_cpu_ms={}\nworking_set_before_bytes={}\nworking_set_after_bytes={}\nworking_set_delta_bytes={}\ngpu_percent={}\nvram_bytes={}\n",
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
    );
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
            results: vec![result(CORPUS[0], "Перевод выполнен локально.")],
        };
        let rendered = render_report(&report, false);
        assert!(!rendered.contains(CORPUS[0].source));
        assert!(!rendered.contains("Перевод выполнен локально."));
        assert!(rendered.contains("case=en_ui"));
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
}
