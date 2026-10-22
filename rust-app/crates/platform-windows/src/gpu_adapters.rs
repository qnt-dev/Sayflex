//! Hardware discovery only: no CUDA context or model is loaded.

#[cfg(windows)]
pub fn nvidia_adapter_name() -> Result<Option<String>, String> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, IDXGIFactory1};
    // SAFETY: DXGI interfaces are owned by their reference-counted Rust wrappers.
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|error| error.to_string())?;
    for index in 0.. {
        // SAFETY: the factory remains alive and the index is a value, not a pointer.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        // SAFETY: the live adapter owns the description returned by value.
        let description = unsafe { adapter.GetDesc1() }.map_err(|error| error.to_string())?;
        if description.VendorId == 0x10de {
            let length = description
                .Description
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(description.Description.len());
            return Ok(Some(String::from_utf16_lossy(
                &description.Description[..length],
            )));
        }
    }
    Ok(None)
}

#[cfg(not(windows))]
pub fn nvidia_adapter_name() -> Result<Option<String>, String> {
    Ok(None)
}
