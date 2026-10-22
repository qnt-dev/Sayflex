include!("../../build/windows_manifest.rs");
include!("../../build/windows_version.rs");
mod build_metadata;

fn main() {
    embed_windows_manifest();
    let metadata = build_metadata::windows_metadata(
        std::env::var_os("CARGO_FEATURE_CUDA").is_some(),
        std::env::var_os("CARGO_FEATURE_VULKAN").is_some(),
        std::env::var_os("CARGO_FEATURE_CPU_COMPAT").is_some(),
    )
    .expect("speech-worker backend features must identify one published executable");
    embed_windows_version(metadata.description, metadata.original_filename);
    if std::env::var_os("CARGO_FEATURE_CUDA").is_some()
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // nvcc marks every object with a default-library request for the
        // static CUDA runtime. Refuse it so the import library for
        // cudart64_*.dll wins: no NVIDIA runtime code is linked into this
        // executable, and the user downloads the DLL from NVIDIA on opt-in.
        println!("cargo:rustc-link-arg=/NODEFAULTLIB:cudart_static.lib");
    }
}
