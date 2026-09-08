include!("../../build/windows_manifest.rs");
include!("../../build/windows_version.rs");

fn main() {
    embed_windows_manifest();
    embed_windows_version("SayFlex speech recognition worker", "SayFlex.Speech.exe");
}
