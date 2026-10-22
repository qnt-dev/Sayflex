#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechWorkerWindowsMetadata {
    pub description: &'static str,
    pub original_filename: &'static str,
}

pub fn windows_metadata(
    cuda_enabled: bool,
    vulkan_enabled: bool,
    cpu_compat: bool,
) -> Result<SpeechWorkerWindowsMetadata, &'static str> {
    match (cuda_enabled, vulkan_enabled, cpu_compat) {
        (false, false, false) => Ok(SpeechWorkerWindowsMetadata {
            description: "SayFlex CPU speech recognition worker",
            original_filename: "SayFlex.Speech.Cpu.exe",
        }),
        (false, false, true) => Ok(SpeechWorkerWindowsMetadata {
            description: "SayFlex CPU speech recognition worker for processors without AVX2",
            original_filename: "SayFlex.Speech.CpuCompat.exe",
        }),
        (true, false, false) => Ok(SpeechWorkerWindowsMetadata {
            description: "SayFlex CUDA speech recognition worker",
            original_filename: "SayFlex.Speech.Cuda.exe",
        }),
        (false, true, false) => Ok(SpeechWorkerWindowsMetadata {
            description: "SayFlex Vulkan speech recognition worker",
            original_filename: "SayFlex.Speech.Vulkan.exe",
        }),
        (true, true, _) => Err("CUDA and Vulkan speech-worker features are mutually exclusive"),
        (_, _, true) => Err("the compatibility build is a CPU-only speech worker"),
    }
}

#[cfg(test)]
mod tests {
    use super::{SpeechWorkerWindowsMetadata, windows_metadata};

    #[test]
    fn each_published_backend_owns_its_installed_windows_filename() {
        assert_eq!(
            windows_metadata(false, false, false),
            Ok(SpeechWorkerWindowsMetadata {
                description: "SayFlex CPU speech recognition worker",
                original_filename: "SayFlex.Speech.Cpu.exe",
            })
        );
        assert_eq!(
            windows_metadata(false, false, true).map(|metadata| metadata.original_filename),
            Ok("SayFlex.Speech.CpuCompat.exe")
        );
        assert_eq!(
            windows_metadata(true, false, false),
            Ok(SpeechWorkerWindowsMetadata {
                description: "SayFlex CUDA speech recognition worker",
                original_filename: "SayFlex.Speech.Cuda.exe",
            })
        );
        assert_eq!(
            windows_metadata(false, true, false),
            Ok(SpeechWorkerWindowsMetadata {
                description: "SayFlex Vulkan speech recognition worker",
                original_filename: "SayFlex.Speech.Vulkan.exe",
            })
        );
    }

    #[test]
    fn mutually_exclusive_gpu_backends_cannot_claim_one_file() {
        assert!(windows_metadata(true, true, false).is_err());
        assert!(windows_metadata(true, false, true).is_err());
        assert!(windows_metadata(false, true, true).is_err());
    }
}
