//! Links the CUDA worker against NVIDIA's DLLs instead of NVIDIA's static
//! loaders.
//!
//! Since CUDA 13 the Toolkit has no import libraries for the runtime and the
//! driver: `cudart.lib`, `cudart_static.lib` and `cuda.lib` are static loaders
//! that end up in the executable. The installer must carry no NVIDIA code
//! (SignPath), so import libraries are made here from the DLLs' own export
//! tables:
//! - `cudart64_*.dll` comes with the opt-in NVIDIA download, beside cuBLAS;
//! - `nvcuda.dll` is part of the NVIDIA display driver.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

pub fn link_dynamic_cuda_runtime() {
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    let cuda = PathBuf::from(env::var("CUDA_PATH").expect("CUDA_PATH must name the CUDA Toolkit"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    let runtime = runtime_dll(&cuda);
    import_library(&runtime, "cudart", "__cudaRegisterFatBinary", &out);
    let system = PathBuf::from(env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned()));
    let driver = system.join("System32").join("nvcuda.dll");
    assert!(
        driver.is_file(),
        "{} is missing: building the CUDA worker needs the NVIDIA display driver",
        driver.display()
    );
    import_library(&driver, "cuda", "cuMemCreate", &out);

    // The import libraries take the Toolkit's names. whisper-rs-sys links
    // `cudart` and `cuda` by name, and this directory is searched before the
    // Toolkit's: ggml's objects come later on the link line than anything
    // this crate names itself, and MSVC's linker searches forward from them,
    // so only the same names keep the static loaders out. nvcc's own
    // default-library request names the static runtime.
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-arg=/NODEFAULTLIB:cudart_static.lib");
}

/// Writes `<out>\<library>.lib`, an import library for every export of `dll`.
/// `required` must be among them, so a wrong DLL fails here and not at link.
fn import_library(dll: &Path, library: &str, required: &str, out: &Path) {
    println!("cargo:rerun-if-changed={}", dll.display());
    let image = fs::read(dll).unwrap_or_else(|error| panic!("{}: {error}", dll.display()));
    let names = export_names(&image).unwrap_or_else(|error| panic!("{}: {error}", dll.display()));
    assert!(
        names.iter().any(|name| name == required),
        "{} does not export {required}",
        dll.display()
    );

    let definition = out.join(format!("{library}.def"));
    let dll_name = dll
        .file_name()
        .and_then(|name| name.to_str())
        .expect("DLL name");
    let mut text = format!("LIBRARY {dll_name}\r\nEXPORTS\r\n");
    for name in &names {
        text.push_str("    ");
        text.push_str(name);
        text.push_str("\r\n");
    }
    fs::write(&definition, text).expect("write the definition file");

    let target = env::var("TARGET").expect("TARGET");
    let status = cc::windows_registry::find_tool(&target, "lib.exe")
        .expect("lib.exe from the MSVC tools")
        .to_command()
        .arg("/nologo")
        .arg("/machine:x64")
        .arg(format!("/def:{}", definition.display()))
        .arg(format!(
            "/out:{}",
            out.join(format!("{library}.lib")).display()
        ))
        .status()
        .expect("run lib.exe");
    assert!(status.success(), "lib.exe could not build {library}.lib");
}

/// `bin\x64\cudart64_*.dll` (CUDA 13) or `bin\cudart64_*.dll` (CUDA 12).
fn runtime_dll(cuda: &Path) -> PathBuf {
    let mut found = Vec::new();
    for directory in [cuda.join("bin").join("x64"), cuda.join("bin")] {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with("cudart64_") && name.ends_with(".dll") {
                found.push(entry.path());
            }
        }
        if !found.is_empty() {
            break;
        }
    }
    assert!(
        found.len() == 1,
        "expected exactly one cudart64_*.dll in {}, found {found:?}",
        cuda.display()
    );
    found.remove(0)
}

/// Names in the export table of a PE32+ image.
fn export_names(image: &[u8]) -> Result<Vec<String>, String> {
    let u16_at = |offset: usize| -> Result<u16, String> {
        image
            .get(offset..offset + 2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .ok_or_else(|| format!("truncated at {offset:#x}"))
    };
    let u32_at = |offset: usize| -> Result<u32, String> {
        image
            .get(offset..offset + 4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .ok_or_else(|| format!("truncated at {offset:#x}"))
    };
    let pe = u32_at(0x3c)? as usize;
    if image.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err("not a PE image".into());
    }
    let coff = pe + 4;
    let sections = usize::from(u16_at(coff + 2)?);
    let optional = coff + 20;
    let optional_size = usize::from(u16_at(coff + 16)?);
    if u16_at(optional)? != 0x20b {
        return Err("not a 64-bit image".into());
    }
    let export_rva = u32_at(optional + 112)?;
    let table = optional + optional_size;
    let offset_of = |rva: u32| -> Result<usize, String> {
        for index in 0..sections {
            let header = table + index * 40;
            let virtual_size = u32_at(header + 8)?;
            let virtual_address = u32_at(header + 12)?;
            let raw_size = u32_at(header + 16)?;
            let raw = u32_at(header + 20)?;
            if rva >= virtual_address && rva < virtual_address + virtual_size.max(raw_size) {
                return Ok((rva - virtual_address + raw) as usize);
            }
        }
        Err(format!("RVA {rva:#x} is outside every section"))
    };
    let directory = offset_of(export_rva)?;
    let count = u32_at(directory + 24)? as usize;
    let names = offset_of(u32_at(directory + 32)?)?;
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let start = offset_of(u32_at(names + index * 4)?)?;
        let end = image[start..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("unterminated export name")?;
        let name =
            std::str::from_utf8(&image[start..start + end]).map_err(|error| error.to_string())?;
        result.push(name.to_owned());
    }
    Ok(result)
}
