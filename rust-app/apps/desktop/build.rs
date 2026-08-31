include!("../../build/windows_manifest.rs");

fn main() {
    embed_windows_manifest();
    #[cfg(target_os = "windows")]
    {
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../resources/icons/sayflex.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let icon = icon
            .to_str()
            .expect("SayFlex icon path must be valid Unicode");
        winresource::WindowsResource::new()
            .set_icon(icon)
            .set("ProductName", "SayFlex")
            .set("FileDescription", "SayFlex local voice and translation")
            .set("InternalName", "SayFlex")
            .set("OriginalFilename", "SayFlex.exe")
            .compile()
            .expect("failed to embed SayFlex Windows resources");
    }
    slint_build::compile("ui/app.slint").expect("failed to compile Slint UI");
}
