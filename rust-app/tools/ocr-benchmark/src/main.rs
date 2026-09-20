mod metrics;

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use image::RgbImage;
use metrics::{normalized_cer, paragraph_normalize};
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PPOCR_SOURCE: &str =
    "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv5";
const TESSERACT_SOURCE: &str = "https://github.com/tesseract-ocr/tesseract";
const WINDOWS_OCR_SOURCE: &str = "Windows.Media.Ocr (installed Windows language packs)";

#[derive(Debug, Parser)]
#[command(about = "Reproducible SayFlex screen OCR candidate benchmark")]
struct Args {
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    models: PathBuf,
    #[arg(long)]
    model_artifacts: PathBuf,
    #[arg(long)]
    output_jsonl: PathBuf,
    #[arg(long, default_value_t = 30)]
    iterations: u32,
    #[arg(long)]
    tesseract_exe: Option<PathBuf>,
    #[arg(long)]
    tessdata: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct FixtureManifest {
    schema_version: u32,
    fixtures: Vec<Fixture>,
}

#[derive(Debug, Clone, Deserialize)]
struct Fixture {
    file: String,
    language: String,
    category: String,
    expected: String,
}

#[derive(Debug, Deserialize)]
struct ModelArtifactManifest {
    schema_version: u32,
    artifacts: Vec<ModelArtifact>,
}

#[derive(Debug, Deserialize)]
struct ModelArtifact {
    name: String,
    sha256: String,
    url: String,
}

#[derive(Debug, Serialize)]
struct BenchmarkRow<'a> {
    engine: &'a str,
    fixture: &'a str,
    language: &'a str,
    category: &'a str,
    iteration: u32,
    expected_text: &'a str,
    recognized_text: String,
    normalized_char_distance: Option<f64>,
    exact_match: Option<bool>,
    elapsed_ms: Option<f64>,
    available: bool,
    failure: Option<String>,
    license: &'a str,
    source: &'a str,
}

#[derive(Debug)]
struct Recognition {
    text: String,
    elapsed_ms: f64,
}

trait Candidate {
    fn name(&self) -> &'static str;
    fn license(&self) -> &'static str;
    fn source(&self) -> &'static str;
    fn recognize(&self, fixture: &Fixture, image_path: &Path) -> Result<Recognition>;
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.iterations == 0 {
        bail!("--iterations must be greater than zero");
    }

    let manifest_bytes = fs::read(&args.manifest)
        .with_context(|| format!("read manifest {}", args.manifest.display()))?;
    let manifest: FixtureManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("parse manifest {}", args.manifest.display()))?;
    if manifest.schema_version != 1 {
        bail!("unsupported manifest schema {}", manifest.schema_version);
    }
    let fixture_root = args
        .manifest
        .parent()
        .ok_or_else(|| anyhow!("manifest has no parent directory"))?;

    if let Some(parent) = args.output_jsonl.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }
    let mut writer = BufWriter::new(
        File::create(&args.output_jsonl)
            .with_context(|| format!("create {}", args.output_jsonl.display()))?,
    );

    let ppocr = ensure_model_artifacts(&args.model_artifacts, &args.models)
        .and_then(|()| PpOcrCandidate::new(&args.models));
    let candidates: Vec<Box<dyn Candidate>> = vec![
        candidate_or_unavailable("ppocr-v5-onnx-cpu", "Apache-2.0", PPOCR_SOURCE, ppocr),
        candidate_or_unavailable(
            "tesseract-5",
            "Apache-2.0",
            TESSERACT_SOURCE,
            TesseractCandidate::new(args.tesseract_exe, args.tessdata),
        ),
        candidate_or_unavailable(
            "windows-media-ocr",
            "Windows component; benchmark-only comparator",
            WINDOWS_OCR_SOURCE,
            WindowsOcrCandidate::new(),
        ),
    ];

    // Warm every available language-family pipeline without polluting measured rows.
    for candidate in &candidates {
        for fixture in &manifest.fixtures {
            let path = fixture_root.join(&fixture.file);
            let _ = candidate.recognize(fixture, &path);
        }
    }

    for iteration in 0..args.iterations {
        for candidate in &candidates {
            for fixture in &manifest.fixtures {
                let image_path = fixture_root.join(&fixture.file);
                let row = match candidate.recognize(fixture, &image_path) {
                    Ok(recognition) => {
                        let actual = paragraph_normalize(&recognition.text);
                        let expected = paragraph_normalize(&fixture.expected);
                        BenchmarkRow {
                            engine: candidate.name(),
                            fixture: &fixture.file,
                            language: &fixture.language,
                            category: &fixture.category,
                            iteration,
                            expected_text: &fixture.expected,
                            normalized_char_distance: Some(normalized_cer(&expected, &actual)),
                            exact_match: Some(actual == expected),
                            recognized_text: actual,
                            elapsed_ms: Some(recognition.elapsed_ms),
                            available: true,
                            failure: None,
                            license: candidate.license(),
                            source: candidate.source(),
                        }
                    }
                    Err(error) => BenchmarkRow {
                        engine: candidate.name(),
                        fixture: &fixture.file,
                        language: &fixture.language,
                        category: &fixture.category,
                        iteration,
                        expected_text: &fixture.expected,
                        recognized_text: String::new(),
                        normalized_char_distance: None,
                        exact_match: None,
                        elapsed_ms: None,
                        available: false,
                        failure: Some(format!("{error:#}")),
                        license: candidate.license(),
                        source: candidate.source(),
                    },
                };
                serde_json::to_writer(&mut writer, &row)?;
                writer.write_all(b"\n")?;
            }
        }
    }
    writer.flush()?;
    Ok(())
}

fn ensure_model_artifacts(manifest_path: &Path, model_root: &Path) -> Result<()> {
    let bytes = fs::read(manifest_path)
        .with_context(|| format!("read model artifact manifest {}", manifest_path.display()))?;
    let manifest: ModelArtifactManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse model artifact manifest {}", manifest_path.display()))?;
    if manifest.schema_version != 1 {
        bail!(
            "unsupported model artifact manifest schema {}",
            manifest.schema_version
        );
    }
    fs::create_dir_all(model_root)
        .with_context(|| format!("create model cache {}", model_root.display()))?;

    for artifact in manifest.artifacts {
        let destination = model_root.join(&artifact.name);
        if destination.is_file() {
            let actual = sha256_file(&destination)?;
            if actual == artifact.sha256 {
                continue;
            }
            bail!(
                "cached model hash mismatch for {}: expected {}, got {}",
                destination.display(),
                artifact.sha256,
                actual
            );
        }

        let partial = model_root.join(format!(".{}.download", artifact.name));
        let download_result = (|| -> Result<()> {
            let mut response = ureq::get(&artifact.url)
                .call()
                .with_context(|| format!("download {}", artifact.url))?;
            let mut output =
                File::create(&partial).with_context(|| format!("create {}", partial.display()))?;
            std::io::copy(&mut response.body_mut().as_reader(), &mut output)
                .with_context(|| format!("write {}", partial.display()))?;
            output.sync_all()?;
            let actual = sha256_file(&partial)?;
            if actual != artifact.sha256 {
                bail!(
                    "downloaded model hash mismatch for {}: expected {}, got {}",
                    artifact.name,
                    artifact.sha256,
                    actual
                );
            }
            fs::rename(&partial, &destination).with_context(|| {
                format!(
                    "atomically move {} to {}",
                    partial.display(),
                    destination.display()
                )
            })?;
            Ok(())
        })();
        if download_result.is_err() {
            let _ = fs::remove_file(&partial);
        }
        download_result?;
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn candidate_or_unavailable<T>(
    name: &'static str,
    license: &'static str,
    source: &'static str,
    candidate: Result<T>,
) -> Box<dyn Candidate>
where
    T: Candidate + 'static,
{
    match candidate {
        Ok(candidate) => Box::new(candidate),
        Err(error) => Box::new(UnavailableCandidate {
            name,
            license,
            source,
            reason: format!("{error:#}"),
        }),
    }
}

struct UnavailableCandidate {
    name: &'static str,
    license: &'static str,
    source: &'static str,
    reason: String,
}

impl Candidate for UnavailableCandidate {
    fn name(&self) -> &'static str {
        self.name
    }

    fn license(&self) -> &'static str {
        self.license
    }

    fn source(&self) -> &'static str {
        self.source
    }

    fn recognize(&self, _fixture: &Fixture, _image_path: &Path) -> Result<Recognition> {
        bail!("{}", self.reason)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ModelFamily {
    English,
    Latin,
    EasternSlavic,
    ChineseGeneral,
    Korean,
    Devanagari,
    Arabic,
}

impl ModelFamily {
    const ALL: [Self; 7] = [
        Self::English,
        Self::Latin,
        Self::EasternSlavic,
        Self::ChineseGeneral,
        Self::Korean,
        Self::Devanagari,
        Self::Arabic,
    ];

    fn for_language(language: &str) -> Self {
        match language {
            "en" => Self::English,
            "es" | "pt-BR" | "de" | "fr" | "it" => Self::Latin,
            "ru" | "uk" | "auto" => Self::EasternSlavic,
            "zh-Hans" | "zh-Hant" | "ja" => Self::ChineseGeneral,
            "ko" => Self::Korean,
            "hi" => Self::Devanagari,
            "ar" => Self::Arabic,
            _ => Self::English,
        }
    }

    fn recognizer(self) -> (&'static str, &'static str) {
        match self {
            Self::English => ("en_PP-OCRv5_rec_mobile.onnx", "ppocrv5_en_dict.txt"),
            Self::Latin => ("latin_PP-OCRv5_rec_mobile.onnx", "ppocrv5_latin_dict.txt"),
            Self::EasternSlavic => ("eslav_PP-OCRv5_rec_mobile.onnx", "ppocrv5_eslav_dict.txt"),
            Self::ChineseGeneral => ("ch_PP-OCRv5_rec_mobile.onnx", "ppocrv5_dict.txt"),
            Self::Korean => ("korean_PP-OCRv5_rec_mobile.onnx", "ppocrv5_korean_dict.txt"),
            Self::Devanagari => (
                "devanagari_PP-OCRv5_rec_mobile.onnx",
                "ppocrv5_devanagari_dict.txt",
            ),
            Self::Arabic => ("arabic_PP-OCRv5_rec_mobile.onnx", "ppocrv5_arabic_dict.txt"),
        }
    }
}

struct PpOcrCandidate {
    pipelines: HashMap<ModelFamily, OAROCR>,
}

impl PpOcrCandidate {
    fn new(models: &Path) -> Result<Self> {
        let detector = models.join("ch_PP-OCRv5_det_mobile.onnx");
        require_file(&detector)?;
        let mut pipelines = HashMap::new();
        for family in ModelFamily::ALL {
            let (recognizer_name, dictionary_name) = family.recognizer();
            let recognizer = models.join(recognizer_name);
            let dictionary = models.join(dictionary_name);
            require_file(&recognizer)?;
            require_file(&dictionary)?;
            let pipeline = OAROCRBuilder::new(&detector, &recognizer, &dictionary)
                .build()
                .with_context(|| format!("build PP-OCRv5 {family:?} pipeline"))?;
            pipelines.insert(family, pipeline);
        }
        Ok(Self { pipelines })
    }
}

impl Candidate for PpOcrCandidate {
    fn name(&self) -> &'static str {
        "ppocr-v5-onnx-cpu"
    }

    fn license(&self) -> &'static str {
        "Apache-2.0"
    }

    fn source(&self) -> &'static str {
        PPOCR_SOURCE
    }

    fn recognize(&self, fixture: &Fixture, image_path: &Path) -> Result<Recognition> {
        let image: RgbImage = image::open(image_path)
            .with_context(|| format!("decode {}", image_path.display()))?
            .to_rgb8();
        let pipeline = self
            .pipelines
            .get(&ModelFamily::for_language(&fixture.language))
            .ok_or_else(|| anyhow!("no PP-OCR pipeline for {}", fixture.language))?;
        let started = Instant::now();
        let result = pipeline
            .predict(vec![image])
            .context("PP-OCR inference failed")?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("PP-OCR returned no image result"))?;
        Ok(Recognition {
            text: postprocess_recognized_text(&fixture.language, &result.concatenated_text("\n")),
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

struct TesseractCandidate {
    executable: PathBuf,
    tessdata: PathBuf,
}

impl TesseractCandidate {
    fn new(executable: Option<PathBuf>, tessdata: Option<PathBuf>) -> Result<Self> {
        let executable = executable.ok_or_else(|| anyhow!("--tesseract-exe was not supplied"))?;
        let tessdata = tessdata.ok_or_else(|| anyhow!("--tessdata was not supplied"))?;
        require_file(&executable)?;
        if !tessdata.is_dir() {
            bail!(
                "Tesseract tessdata directory not found: {}",
                tessdata.display()
            );
        }
        let status = Command::new(&executable)
            .arg("--version")
            .status()
            .context("run Tesseract version probe")?;
        if !status.success() {
            bail!("Tesseract version probe exited with {status}");
        }
        Ok(Self {
            executable,
            tessdata,
        })
    }

    fn language_code(language: &str) -> &'static str {
        match language {
            "en" => "eng",
            "ru" => "rus",
            "uk" => "ukr",
            "es" => "spa",
            "pt-BR" => "por",
            "de" => "deu",
            "fr" => "fra",
            "it" => "ita",
            "zh-Hans" => "chi_sim",
            "zh-Hant" => "chi_tra",
            "ja" => "jpn",
            "ko" => "kor",
            "hi" => "hin",
            "ar" => "ara",
            "auto" => "eng+rus",
            _ => "eng",
        }
    }
}

impl Candidate for TesseractCandidate {
    fn name(&self) -> &'static str {
        "tesseract-5"
    }

    fn license(&self) -> &'static str {
        "Apache-2.0"
    }

    fn source(&self) -> &'static str {
        TESSERACT_SOURCE
    }

    fn recognize(&self, fixture: &Fixture, image_path: &Path) -> Result<Recognition> {
        let languages = Self::language_code(&fixture.language);
        for language in languages.split('+') {
            require_file(&self.tessdata.join(format!("{language}.traineddata")))?;
        }
        let started = Instant::now();
        let output = Command::new(&self.executable)
            .arg(image_path)
            .arg("stdout")
            .arg("--tessdata-dir")
            .arg(&self.tessdata)
            .arg("-l")
            .arg(languages)
            .arg("--psm")
            .arg("6")
            .output()
            .context("run Tesseract")?;
        if !output.status.success() {
            bail!(
                "Tesseract failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(Recognition {
            text: String::from_utf8(output.stdout).context("Tesseract emitted non-UTF-8")?,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

struct WindowsOcrCandidate;

impl WindowsOcrCandidate {
    fn new() -> Result<Self> {
        if !has_package_identity() {
            bail!("benchmark process has no Windows package identity");
        }
        use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.context("initialize WinRT MTA")?;
        Ok(Self)
    }
}

impl Candidate for WindowsOcrCandidate {
    fn name(&self) -> &'static str {
        "windows-media-ocr"
    }

    fn license(&self) -> &'static str {
        "Windows component; benchmark-only comparator"
    }

    fn source(&self) -> &'static str {
        WINDOWS_OCR_SOURCE
    }

    fn recognize(&self, fixture: &Fixture, image_path: &Path) -> Result<Recognition> {
        use windows::Globalization::Language;
        use windows::Graphics::Imaging::BitmapDecoder;
        use windows::Media::Ocr::OcrEngine;
        use windows::Storage::{FileAccessMode, StorageFile};
        use windows::core::HSTRING;

        let canonical = image_path
            .canonicalize()
            .with_context(|| format!("canonicalize {}", image_path.display()))?;
        let hpath = HSTRING::from(canonical.to_string_lossy().as_ref());
        let file = StorageFile::GetFileFromPathAsync(&hpath)?.join()?;
        let stream = file.OpenAsync(FileAccessMode::Read)?.join()?;
        let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
        let bitmap = decoder.GetSoftwareBitmapAsync()?.join()?;
        let engine = if fixture.language == "auto" {
            OcrEngine::TryCreateFromUserProfileLanguages()?
        } else {
            let language = Language::CreateLanguage(&HSTRING::from(fixture.language.as_str()))?;
            if !OcrEngine::IsLanguageSupported(&language)? {
                bail!(
                    "Windows OCR language pack is unavailable: {}",
                    fixture.language
                );
            }
            OcrEngine::TryCreateFromLanguage(&language)?
        };
        let started = Instant::now();
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        Ok(Recognition {
            text: result.Text()?.to_string(),
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

fn has_package_identity() -> bool {
    use windows::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

    let mut length = 0u32;
    let status = unsafe { GetCurrentPackageFullName(&mut length, None) };
    status != APPMODEL_ERROR_NO_PACKAGE && length > 0
}

fn require_file(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        bail!("required file not found: {}", path.display())
    }
}

fn postprocess_recognized_text(language: &str, text: &str) -> String {
    if language != "ar" {
        return text.to_owned();
    }

    text.lines()
        .map(|line| {
            let bidi = unicode_bidi::BidiInfo::new(line, None);
            bidi.paragraphs
                .first()
                .map(|paragraph| {
                    bidi.reorder_line(paragraph, paragraph.range.clone())
                        .into_owned()
                })
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::postprocess_recognized_text;

    #[test]
    fn arabic_visual_order_is_restored_per_line() {
        assert_eq!(
            postprocess_recognized_text("ar", "ةمجرت\nةشاشلا"),
            "ترجمة\nالشاشة"
        );
    }

    #[test]
    fn non_arabic_text_is_not_reordered() {
        assert_eq!(postprocess_recognized_text("ru", "Перевод"), "Перевод");
    }
}
