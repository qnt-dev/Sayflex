pub fn embed_windows_manifest() {
    println!("cargo:rerun-if-env-changed=SAYFLEX_BUILD_COMMIT");
    #[cfg(target_os = "windows")]
    {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../windows/app.manifest");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
}
