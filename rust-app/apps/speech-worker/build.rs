include!("../../build/windows_manifest.rs");
include!("../../build/windows_version.rs");
mod build_metadata;

fn main() {
    embed_windows_manifest();
    let metadata = build_metadata::windows_metadata(
        std::env::var_os("CARGO_FEATURE_CUDA").is_some(),
        std::env::var_os("CARGO_FEATURE_VULKAN").is_some(),
    )
    .expect("speech-worker backend features must identify one published executable");
    embed_windows_version(metadata.description, metadata.original_filename);
}
