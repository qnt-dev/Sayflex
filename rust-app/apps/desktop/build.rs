include!("../../build/windows_manifest.rs");

fn main() {
    embed_windows_manifest();
    slint_build::compile("ui/app.slint").expect("failed to compile Slint UI");
}
