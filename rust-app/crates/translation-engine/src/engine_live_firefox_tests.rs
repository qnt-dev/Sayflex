//! Opt-in comparison of the already installed Firefox compact pack.
//! No source text is embedded here and no model is selected in the application.
//!
//! Required: SAYFLEX_LIVE_FIXTURE (TranslateBlocks JSON/JSONL),
//! SAYFLEX_LIVE_FIREFOX_MODEL (installed pack directory),
//! SAYFLEX_LIVE_OUTPUT_DIR (existing directory beneath TEMP).
//! Optional: SAYFLEX_LIVE_BLOCK_IDS (comma-separated exact fixture IDs).
//! SAYFLEX_LIVE_COMPACT_BLOCKS=1 uses the production compact block path and
//! records its completed/failed results instead of the stricter comparison.
//! SAYFLEX_LIVE_TOKEN_DETAILS=1 inspects direct single-window source/output
//! token IDs and pieces; it does not assert translation quality.
//! Only the test process loads the CPU models. It starts no server and downloads nothing.

#![cfg(windows)]

use std::{collections::HashSet, env, mem::size_of};

use rust_protocol::{RequestId, TranslationWorkerCommand};
use serde_json::json;

use super::*;

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
    // SAFETY: the Windows structure has its documented layout and writable storage.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(status.available_physical)
}

fn required_path(name: &str) -> Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("set {name} explicitly for this ignored diagnostic"))
}

#[test]
#[ignore = "opt-in installed Firefox CPU comparison; external fixture, model path and TEMP output required"]
fn firefox_fixture_live_cpu() -> Result<()> {
    let fixture_path = required_path("SAYFLEX_LIVE_FIXTURE")?.canonicalize()?;
    let fixture_text = fs::read_to_string(&fixture_path)?;
    let fixture_text = fixture_text.trim_start_matches('\u{feff}');
    let command = serde_json::from_str::<TranslationWorkerCommand>(fixture_text)
        .ok()
        .or_else(|| {
            fixture_text.lines().find_map(|line| {
                serde_json::from_str::<TranslationWorkerCommand>(line)
                    .ok()
                    .filter(|command| {
                        matches!(command, TranslationWorkerCommand::TranslateBlocks { .. })
                    })
            })
        })
        .ok_or_else(|| anyhow!("fixture must contain a production TranslateBlocks command"))?;
    command
        .validate_limits()
        .map_err(|error| anyhow!("invalid screen fixture: {error:?}"))?;
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
    let source = source_language.ok_or_else(|| anyhow!("fixture must specify source_language"))?;
    let target = target_language.ok_or_else(|| anyhow!("fixture must specify target_language"))?;
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
            bail!("every selected block ID must exist in the original fixture");
        }
        blocks.retain(|block| selected.contains(block.block_id.as_str()));
    }
    if blocks.is_empty() {
        bail!("fixture contains no selected source rows");
    }

    let catalog = crate::TranslationCatalog::load()?;
    let pack = catalog
        .by_id("firefox-ru-en-v3.1")
        .ok_or_else(|| anyhow!("installed Firefox EN/RU pack is absent from the catalog"))?
        .clone();
    if !matches!(pack.backend, TranslationBackendDescriptor::Firefox { .. })
        || !pack.supports(source, target)
    {
        bail!("diagnostic accepts only the Firefox EN/RU CPU pack and its supported direction");
    }
    let model_root = required_path("SAYFLEX_LIVE_FIREFOX_MODEL")?.canonicalize()?;
    let mut model_bytes = 0_u64;
    for artifact in &pack.artifacts {
        let path = model_root.join(&artifact.relative_path);
        let metadata = fs::metadata(&path).with_context(|| {
            format!("installed compact artifact is missing: {}", path.display())
        })?;
        if !metadata.is_file() {
            bail!("installed compact artifact is not a file");
        }
        model_bytes = model_bytes.saturating_add(metadata.len());
    }
    let available_ram = available_physical_memory()?;
    // Conservative opt-in guard, not an estimate of the decoder's peak allocation.
    if available_ram < 2 * 1024 * 1024 * 1024 + model_bytes {
        bail!("compact diagnostic requires at least 2 GiB spare RAM beyond model file size");
    }
    let output_root = required_path("SAYFLEX_LIVE_OUTPUT_DIR")?.canonicalize()?;
    let temporary_root = env::temp_dir().canonicalize()?;
    if !output_root.starts_with(&temporary_root) {
        bail!("private diagnostic output must stay beneath TEMP");
    }
    let report_path = output_root.join(format!(
        "firefox-comparison-{}.report.json",
        RequestId::new()
    ));
    let unused_runtime_root = tempfile::tempdir()?;
    let mut engine = TranslationEngine::new(
        pack.clone(),
        model_root.clone(),
        unused_runtime_root.path().to_owned(),
    );
    let started = Instant::now();
    let deadline = started + STRUCTURED_OPERATION_TIMEOUT;
    let active = || Instant::now() < deadline;
    let mut report = json!({
        "diagnostic_only": true,
        "fixture": fixture_path,
        "catalog_id": pack.id,
        "model_root": model_root,
        "model_bytes": model_bytes,
        "available_ram_before_load": available_ram,
        "gpu_layers": 0,
        "gemm_backend": fxtranslate::gemm::backend(),
        "server_process_started": false,
        "context_consumed": false,
        "glossary_consumed": false,
        "fixture_context": context,
        "source_language": source,
        "target_language": target,
        "selected_block_ids": blocks.iter().map(|block| block.block_id.as_str()).collect::<Vec<_>>(),
        "operation_limit_ms": STRUCTURED_OPERATION_TIMEOUT.as_millis(),
        "deadline_interrupts_inside_synchronous_decode": false,
        "all_selected_rows_finished": false,
        "structurally_valid_rows": 0,
        "rows": [],
    });
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    eprintln!("Firefox CPU comparison evidence: {}", report_path.display());
    if env::var("SAYFLEX_LIVE_TOKEN_DETAILS").as_deref() == Ok("1") {
        let TranslationBackendDescriptor::Firefox { directions } = &pack.backend else {
            unreachable!("checked Firefox backend");
        };
        let direction = directions
            .iter()
            .find(|direction| direction.source == source && direction.target == target)
            .ok_or_else(|| anyhow!("missing diagnostic direction"))?;
        let source_path = model_root.join(&direction.source_vocab_file);
        let target_path = model_root.join(&direction.target_vocab_file);
        let source_vocab = fxtranslate::spm::SpmVocab::load(&source_path)?;
        let target_vocab = fxtranslate::spm::SpmVocab::load(&target_path)?;
        let direct = fxtranslate::engine::Engine::load_mmapped(
            model_root.join(&direction.model_file),
            &source_path,
            &target_path,
        )
        .map_err(|error| anyhow!(error))?
        .with_shortlist(fxtranslate::shortlist::Shortlist::load(
            model_root.join(&direction.shortlist_file),
        )?);
        report["diagnostic_mode"] = json!("direct_token_details");
        report["direct_model_load_ms"] = json!(started.elapsed().as_millis());
        report
            .as_object_mut()
            .expect("report object")
            .remove("structurally_valid_rows");
        for block in &blocks {
            if !active() {
                bail!("token diagnostic exceeded deadline between rows");
            }
            let row_started = started.elapsed().as_millis();
            let source_ids = direct.src_ids(&block.source_text);
            if source_ids.len() > fxtranslate::segment::CONTEXT_WINDOW {
                bail!("direct token diagnostic requires each source to fit one model window");
            }
            let output_ids = direct.greedy(&source_ids);
            let row = json!({
                "block_id": block.block_id,
                "source_text": block.source_text,
                "request_elapsed_ms": row_started,
                "response_elapsed_ms": started.elapsed().as_millis(),
                "source_ids": source_ids,
                "source_pieces": source_ids.iter().map(|id| source_vocab.piece(*id)).collect::<Vec<_>>(),
                "source_roundtrip": source_vocab.decode(&source_ids),
                "output_ids": output_ids,
                "output_pieces": output_ids.iter().map(|id| target_vocab.piece(*id)).collect::<Vec<_>>(),
                "raw_translation": target_vocab.decode(&output_ids),
                "validation": "not_run_direct_token_diagnostic",
            });
            report["rows"].as_array_mut().expect("row array").push(row);
            report["elapsed_ms"] = json!(started.elapsed().as_millis());
            fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
        }
        report["all_selected_rows_finished"] = json!(true);
        fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
        return Ok(());
    }
    if env::var("SAYFLEX_LIVE_COMPACT_BLOCKS").as_deref() == Ok("1") {
        report["diagnostic_mode"] = json!("production_compact_blocks");
        report
            .as_object_mut()
            .expect("report object")
            .remove("structurally_valid_rows");
        let mut published = Vec::new();
        let output = engine.translate_blocks_streaming_until(
            &blocks,
            &context,
            &[],
            Some(source),
            Some(target),
            deadline,
            &active,
            |block| {
                published.push(block.clone());
                active()
            },
        );
        report["elapsed_ms"] = json!(started.elapsed().as_millis());
        report["published_blocks"] = json!(published);
        match output {
            Ok(output) => {
                let completed = output
                    .blocks
                    .iter()
                    .filter(|block| {
                        block.completion == BlockCompletion::Completed
                            && block.failure_message.is_none()
                    })
                    .count();
                report["completed_rows"] = json!(completed);
                report["failed_rows"] = json!(output.blocks.len() - completed);
                report["capabilities"] = json!(output.capabilities);
                report["blocks"] = json!(output.blocks);
                report["all_selected_rows_finished"] = json!(true);
                fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
                eprintln!(
                    "Firefox production compact evidence: completed_rows={completed}; failed_rows={}; completion is not a translation-quality gate",
                    blocks.len() - completed
                );
                return Ok(());
            }
            Err(error) => {
                report["error"] = json!(format!("{error:#}"));
                fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
                return Err(error);
            }
        }
    }
    let mut valid_count = 0;
    for block in &blocks {
        if !active() {
            report["error"] = json!("shared operation deadline exceeded between rows");
            report["elapsed_ms"] = json!(started.elapsed().as_millis());
            fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
            bail!("compact diagnostic exceeded the operation deadline");
        }
        let row_started = started.elapsed().as_millis();
        let translated = engine.translate_streaming(
            &block.source_text,
            Some(source),
            Some(target),
            active,
            |_, _| active(),
        );
        let row = json!({
            "block_id": block.block_id,
            "source_text": block.source_text,
            "request_elapsed_ms": row_started,
            "response_elapsed_ms": started.elapsed().as_millis(),
            "raw_translation": translated.as_ref().ok().map(|output| output.text.as_str()),
            "error": translated.as_ref().err().map(|error| format!("{error:#}")),
            "validation": "not_run",
        });
        let index = report["rows"].as_array().expect("row array").len();
        report["rows"].as_array_mut().expect("row array").push(row);
        // Preserve raw output before applying the full structured validator.
        fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
        let translated = translated?;
        let bound_response = serde_json::to_string(&json!({
            "translations": [{"block_id": block.block_id, "translated_text": translated.text}],
        }))?;
        match validate_structured_response(&bound_response, std::slice::from_ref(block), target) {
            Ok(_) => {
                valid_count += 1;
                report["rows"][index]["validation"] = json!("valid");
            }
            Err(error) => {
                report["rows"][index]["validation"] = json!(error.to_string());
            }
        }
        report["structurally_valid_rows"] = json!(valid_count);
        report["elapsed_ms"] = json!(started.elapsed().as_millis());
        fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    }
    report["all_selected_rows_finished"] = json!(true);
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    eprintln!(
        "Firefox comparison finished: structurally_valid_rows={valid_count}; diagnostic success is not a translation-quality gate"
    );
    Ok(())
}
