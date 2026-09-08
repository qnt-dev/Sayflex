/// Stamps a shipped executable with what it is and which release it belongs to.
///
/// Every process in the package is one a person can find in Task Manager, and
/// until this existed only the desktop carried a version — the five workers
/// reported nothing at all, so an installed set could not be identified from
/// the files themselves. The release check now asks each of them and refuses a
/// package whose parts disagree.
///
/// The icon is the desktop's own business: a worker has no window.
pub fn embed_windows_version(description: &str, file_name: &str) {
    #[cfg(target_os = "windows")]
    {
        winresource::WindowsResource::new()
            .set("ProductName", "SayFlex")
            .set("FileDescription", description)
            .set("InternalName", file_name)
            .set("OriginalFilename", file_name)
            .set("CompanyName", "SayFlex")
            .compile()
            .expect("failed to embed SayFlex version resources");
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (description, file_name);
    }
}
