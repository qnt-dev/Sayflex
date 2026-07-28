# Third-party notices

The Rust application is distributed under the adjacent MIT license. Its exact
dependency graph is locked in `Cargo.lock` and checked by `cargo-deny` in CI.

## Slint

The user interface uses Slint 1.17 under the Slint Royalty-free Desktop,
Mobile, and Web Applications License 2.0. The required `AboutSlint` attribution
widget is available from the application's top-level About page.

Copyright © SixtyFPS GmbH <info@slint.dev>

<https://slint.dev/>

## Whisper

Speech recognition uses OpenAI Whisper model weights and the whisper.cpp
runtime through `whisper-rs`.

- OpenAI Whisper: <https://github.com/openai/whisper>
- whisper.cpp: <https://github.com/ggml-org/whisper.cpp>
- whisper-rs: <https://github.com/tazz4843/whisper-rs>

## Translation models

Optional translation packs are downloaded only after an explicit user choice.
Their exact artifact, checksum, origin, and license metadata are documented in
`docs/THIRD_PARTY_MODELS.md` and the built-in translation catalog.

The current packs are quantized derivatives of Alibaba Cloud's Qwen3.5 4B and
9B models under Apache-2.0. Local inference uses the MIT-licensed llama.cpp
runtime from `ggml-org/llama.cpp`. Neither model weights nor the runtime are
bundled in the base installer; both are fetched only when translation is
enabled.

## Other Rust dependencies

The application includes permissively licensed Rust crates listed in
`Cargo.lock`. Their SPDX expressions are enforced by `deny.toml`; upstream
copyright and license files remain available from the corresponding crate
sources and registry packages.
