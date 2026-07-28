# Third-party translation models

Translation models are not distributed inside the installer. The application
downloads a user-selected pack directly from the pinned upstream revision and
verifies every artifact before installation.

| Catalog entry | Upstream model | Pinned revision | License |
| --- | --- | --- | --- |
| Fast multilingual | `Qwen/Qwen3.5-4B`, GGUF by `bartowski` | `4168f45a16a1290d65a4ec0fa312ae917a4c15d6` | Apache-2.0 |
| Quality multilingual | `Qwen/Qwen3.5-9B`, GGUF by `bartowski` | `182be2fd6c7bc44887d88a91cb03ff009cc9f549` | Apache-2.0 |

Inference uses the official llama.cpp Windows Vulkan archive from release
`b10164`, downloaded from `ggml-org/llama.cpp` and verified before atomic
extraction. llama.cpp is MIT-licensed.

The exact URLs, sizes, and SHA-256 values used by the installer live in
`crates/translation-engine/models/catalog.json`.

Before publishing a catalog update:

1. Review the model card and license at the exact pinned revision.
2. Verify that commercial redistribution or user-directed download matches the
   intended release channel.
3. Recompute byte lengths and SHA-256 independently.
4. Run an interrupted-download test and both translation directions.
5. Update this notice and any attribution shown by the public download page.
