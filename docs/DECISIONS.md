# Decision log

## 2026-07-23 — Manual selection is the primary correction model

Automatic layout guessing is unsafe for passwords, code, invented words, deliberate
wrong-layout strings, and ambiguous short tokens. Keyboop therefore does not inspect
or alter ordinary typing by default. The supported workflow is explicit selection,
`Ctrl+Alt+K`, local preview, and explicit apply or Escape. Automatic correction is
retained only as an off-by-default experimental mode.

## 2026-07-23 — Functional independence from the current macOS binary

The public Keyboop source is an algorithmic reference, not a pixel or black-box
parity target. Missing behavior is resolved in favor of safe Windows conventions.

## 2026-07-23 — No paid signing dependency

Development and private builds use unsigned, portable, or self-signed artifacts.
Store packaging can be added later if free Microsoft signing is desired.

## 2026-07-23 — High-quality local real-time ASR

The primary development profile is persistent Whisper `large-v3-turbo` on CUDA for
the installed RTX 3080 Ti. The worker API stays backend-neutral so Parakeet or a
future model can be benchmarked without changing keyboard or UI code.

## 2026-07-23 — Deterministic keyboard hot path

No LLM or speech/translation model participates in layout correction. The input
process owns the detector, event state, and fail-open transaction locally.
