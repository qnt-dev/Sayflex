fn main() {
    #[cfg(target_os = "windows")]
    if std::env::var_os("CARGO_FEATURE_PPOCR").is_some() {
        // The pinned static ONNX Runtime build includes its DirectML provider
        // factory even when SayFlex selects the CPU execution provider. Link
        // the Windows SDK import library so that provider factory is complete.
        println!("cargo:rustc-link-lib=DirectML");
    }
}
