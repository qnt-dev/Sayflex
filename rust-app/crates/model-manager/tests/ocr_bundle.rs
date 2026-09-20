use std::fs;

use rust_model_manager::{BundleInstaller, screen_ocr_bundle};
use tempfile::tempdir;

#[test]
fn ocr_bundle_declares_every_verified_owned_artifact() {
    let bundle = screen_ocr_bundle();

    assert_eq!(bundle.id, "screen-ocr-ppocrv5-mobile-v3.9.2");
    assert_eq!(bundle.license.as_deref(), Some("Apache-2.0"));
    assert!(
        bundle
            .attribution
            .as_deref()
            .is_some_and(|value| value.contains("RapidOCR"))
    );
    assert_eq!(bundle.artifacts.len(), 15);
    assert!(
        bundle
            .artifacts
            .iter()
            .any(|artifact| artifact.relative_path == "detector/ch_PP-OCRv5_det_mobile.onnx")
    );
    assert!(
        bundle
            .artifacts
            .iter()
            .any(|artifact| artifact.relative_path == "recognizers/arabic_PP-OCRv5_rec_mobile.onnx")
    );
    assert!(
        bundle
            .artifacts
            .iter()
            .any(|artifact| artifact.relative_path == "dictionaries/ppocrv5_devanagari_dict.txt")
    );
    bundle.validate().unwrap();
}

#[test]
fn complete_ocr_removal_accounts_for_models_receipt_partial_and_backup() {
    let root = tempdir().unwrap();
    let bundle = screen_ocr_bundle();
    let installer = BundleInstaller::new(root.path().to_owned());
    let final_path = installer.bundle_path(&bundle);
    let partial_path = root.path().join(format!(".{}.download", bundle.id));
    let previous_path = root.path().join(format!("{}.previous", bundle.id));

    for base in [&final_path, &partial_path, &previous_path] {
        for artifact in &bundle.artifacts {
            let path = base.join(&artifact.relative_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"x").unwrap();
        }
        fs::write(base.join("bundle.verified.json"), b"receipt").unwrap();
    }

    let receipt = installer.remove_complete(&bundle).unwrap();

    assert_eq!(
        receipt.removed_files,
        3 * (bundle.artifacts.len() as u64 + 1)
    );
    assert_eq!(
        receipt.removed_bytes,
        3 * (bundle.artifacts.len() as u64 + b"receipt".len() as u64)
    );
    assert!(!final_path.exists());
    assert!(!partial_path.exists());
    assert!(!previous_path.exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
