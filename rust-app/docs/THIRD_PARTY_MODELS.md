# Third-party translation models

Translation models are not distributed inside the installer. The application
downloads a user-selected pack directly from the pinned upstream revision and
verifies every artifact before installation.

| Catalog entry | Upstream model | Pinned revision | License shown by catalog |
| --- | --- | --- | --- |
| Fast RU→EN | `Helsinki-NLP/opus-mt_tiny_rus-eng` | `795b0b2fb13d17b917287421c15e0d70db9ba940` | Apache-2.0 |
| Fast EN→RU | `Helsinki-NLP/opus-mt_tiny_eng-rus` | `3172938c8acb3f54ab522db1aaca1f12bf168322` | Apache-2.0 |
| Quality RU→EN | `Helsinki-NLP/opus-mt-ru-en` | `fbd6dc73284f95536648512cc21d57f19191961a` | CC-BY-4.0 |
| Quality EN→RU | `Helsinki-NLP/opus-mt-en-ru` | `bb09c99d180016eac6819df3dae68edb1690fdee` | Apache-2.0 |

The exact URLs, sizes, and SHA-256 values used by the installer live in
`crates/translation-engine/models/catalog.json`.

Before publishing a catalog update:

1. Review the model card and license at the exact pinned revision.
2. Verify that commercial redistribution or user-directed download matches the
   intended release channel.
3. Recompute byte lengths and SHA-256 independently.
4. Run an interrupted-download test and both translation directions.
5. Update this notice and any attribution shown by the public download page.
