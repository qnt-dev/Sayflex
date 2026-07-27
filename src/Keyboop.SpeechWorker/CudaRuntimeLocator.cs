using System.Runtime.InteropServices;

namespace Keyboop.SpeechWorker;

internal static class CudaRuntimeLocator
{
    private static readonly List<nint> LoadedLibraries = [];
    private static readonly object SyncRoot = new();
    private static bool _prepared;

    internal static string Prepare()
    {
        lock (SyncRoot)
        {
            if (_prepared)
            {
                return LoadedLibraries.Count > 0
                    ? "CUDA 13 runtime preloaded."
                    : "CUDA 13 runtime was not found.";
            }

            _prepared = true;
            var directory = FindCudaBinaryDirectory();
            if (directory is null)
            {
                return "CUDA 13 runtime was not found.";
            }

            try
            {
                Load(directory, "cudart64_13.dll");
                Load(directory, "cublasLt64_13.dll");
                Load(directory, "cublas64_13.dll");
                return $"CUDA 13 runtime preloaded from {directory}.";
            }
            catch (Exception)
            {
                foreach (var handle in LoadedLibraries)
                {
                    NativeLibrary.Free(handle);
                }

                LoadedLibraries.Clear();
                return $"CUDA 13 runtime could not be loaded from {directory}.";
            }
        }
    }

    private static string? FindCudaBinaryDirectory()
    {
        var candidates = new List<string>();
        AddEnvironmentCandidate(candidates, "CUDA_PATH");
        AddEnvironmentCandidate(candidates, "CUDA_PATH_V13_3");

        const string toolkitRoot =
            @"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA";
        if (Directory.Exists(toolkitRoot))
        {
            candidates.AddRange(
                Directory.EnumerateDirectories(toolkitRoot, "v13.*")
                    .OrderByDescending(
                        static path => path,
                        StringComparer.OrdinalIgnoreCase));
        }

        foreach (var candidate in candidates.Distinct(
            StringComparer.OrdinalIgnoreCase))
        {
            var binaryDirectory = Path.Combine(candidate, "bin", "x64");
            if (File.Exists(Path.Combine(binaryDirectory, "cudart64_13.dll"))
                && File.Exists(Path.Combine(binaryDirectory, "cublas64_13.dll"))
                && File.Exists(Path.Combine(binaryDirectory, "cublasLt64_13.dll")))
            {
                return binaryDirectory;
            }
        }

        return null;
    }

    private static void AddEnvironmentCandidate(
        List<string> candidates,
        string name)
    {
        var processValue = Environment.GetEnvironmentVariable(name);
        if (!string.IsNullOrWhiteSpace(processValue))
        {
            candidates.Add(processValue);
        }

        var machineValue = Environment.GetEnvironmentVariable(
            name,
            EnvironmentVariableTarget.Machine);
        if (!string.IsNullOrWhiteSpace(machineValue))
        {
            candidates.Add(machineValue);
        }
    }

    private static void Load(string directory, string fileName)
    {
        var handle = NativeLibrary.Load(Path.Combine(directory, fileName));
        LoadedLibraries.Add(handle);
    }
}
