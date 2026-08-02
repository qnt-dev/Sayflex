include!("../../build/windows_manifest.rs");

fn main() {
    embed_windows_manifest();
    slint_build::compile("ui/companion.slint").expect("failed to compile Companion Orb UI");
}
