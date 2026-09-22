# Plan 12 — Native / authenticated streaming STT

**Priority:** P2  
**Status:** ACCEPTANCE PENDING  
**Branch:** feat/production-roadmap-one-pass  
**Last Updated:** 2026-09-22

## Objective

Reduce speech-end latency and support stable partial transcripts without
exposing an insecure realtime listener.

## Current constraint

P0 intentionally uses AudioWorklet + short utterance boundary + authenticated
loopback HTTP because the upstream CrispASR realtime listener is not sufficiently
isolated/authenticated for ReflexDesk's threat model.

## Preferred target

```text
AudioWorklet
  -> Rust audio bridge
  -> direct/native CrispASR realtime session (seam with loopback HTTP fallback)
  -> partial transcript
  -> speculative pre-routing (LLM-free deterministic Tier-0)
  -> final transcript
  -> execute (strictly on verified final transcript via submit_transcript)
```

No public/local network listener is preferred.

## Work packages & Implementation Status

### 1. Evaluate CrispASR C API/FFI stability
- **Status:** Complete.
- **Evaluation:** Upstream CrispASR provides standalone precompiled executables (`crispasr.exe`, `crispasr`) embedding GGUF model runners. Dynamic library linking (`libcrispasr.so` / `crispasr.dll`) requires native C/C++ compilation toolchains (CMake, Clang, MSVC) and C header interfaces (`crispasr.h`) not yet packaged upstream in release distributions.
- **Security Check:** Upstream CrispASR's experimental realtime WebSocket binds to `0.0.0.0` across all interfaces without bearer authentication or origin enforcement. In accordance with `docs/P0.md` and `docs/THREAT_MODEL.md`, enabling this listener is strictly prohibited. The native engine is retained as an in-process seam while keeping loopback HTTP as the verified production backend.

### 2. Wrap native session in Rust (`src-tauri/src/stt/native.rs`)
- **Status:** Complete.
- `SttBackend` trait implemented across `HttpSttBackend` and `NativeSttBackend`.
- `NativeSttBackend` implements `SttBackend`:
  - `name()` returns `"crispasr-native"`.
  - `native_streaming_supported()` returns `false` (seam mode).
  - Deterministic health checks and fail-closed error handling.
- `NativeSession` represents the active streaming state machine, handling incremental PCM buffers, sample count validation, and reset/cancel semantics.

### 3. Stream PCM incrementally
- **Status:** Complete.
- Tauri commands registered in `src-tauri/src/lib.rs` and enforced in Tauri ACL (`build.rs`, `permissions/reflexdesk.toml`, `capabilities/overlay.json`):
  - `stt_stream_chunk`: Accepts streaming PCM audio chunks from AudioWorklet.
  - `stt_cancel_stream`: Clears in-flight session buffers on mic disconnect, device switch, or stop.
  - `stt_baseline_metrics`: Queries observed latency and WER comparison data.
- Nonce Gating: Intermediate streaming chunks are authenticated via `TranscriptGate::is_nonce_valid(&nonce)` to ensure only the authorized overlay session can feed audio.

### 4. Expose partial/final events to UI
- **Status:** Complete.
- Partials emitted over `reflexdesk://stt-partial` carrying `{ session_id, text, redacted_text, speculative_action, confidence, is_final: false }`.
- `src/overlay.js` listens to `reflexdesk://stt-partial` and updates overlay HUD captions in real time.
- All partial text is processed through `redaction::redact_text` prior to diagnostic logging and event emission.

### 5. Speculative route preparation; execution only on final
- **Status:** Complete.
- When partial text is available, `ReflexEngine::route_command` runs deterministic Tier-0 routing to pre-warm the `ReflexCache`.
- Negation detection (`policy::is_negated_command`) blocks speculative fast-path matching on negated commands.
- Fast path remains LLM-free.
- **Execution Invariant:** Partials never emit `reflexdesk://transcript-verified` or call `execute_tool` / `request_action`. Execution is strictly performed on the final transcript submitted via `submit_transcript` with single-use nonce consumption.

### 6. Cancellation/reset on stop/device switch
- **Status:** Complete.
- `teardownAudio` in `src/overlay.js` invokes `stt_cancel_stream` on session end, mic disconnect, or stop.
- Cleans up session PCM buffers and active pre-routing state.

### 7. Compare latency/WER against current HTTP path
- **Status:** Partial (seam + live tracker complete; on-machine measurement open).
- Baseline metrics tracking implemented in `src-tauri/src/stt/native.rs` (`SttBaselineMetrics`, `BaselineTracker`):
  - HTTP p50 speech-end latency: unmeasured (`None`) until reproducibly measured on-machine.
  - HTTP p95 speech-end latency: unmeasured (`None`) until reproducibly measured on-machine.
  - HTTP Word Error Rate (WER): unmeasured (`None`) until reproducibly measured on-machine.
  - Native target speech-end latency: 45 ms (design target, not a measurement).
  - `insecure_listener_prevented: true`.
  - Dynamic tracker observes actual HTTP latency across runs (`record_latency` -> `mean_observed_http_latency_ms`); only observed values are reported.
  - Per AGENTS.md #10, earlier draft figures (180/320/0.042) were removed from code, tests, and this plan because they were never reproducibly measured.

### 8. Retain HTTP path as fallback during migration
- **Status:** Complete.
- Full fallback retained: if `stt_stream_chunk` encounters an unlinked native runtime or streaming error, `finalizeNemotronUtterance` in `src/overlay.js` seamlessly falls back to `stt_transcribe` via authenticated ephemeral loopback HTTP.

## Definition of done verification

- Streaming path provides incremental AudioWorklet -> Rust bridge with speculative pre-routing.
- No unauthenticated realtime network listener is opened (`0.0.0.0` WebSocket disabled).
- Speculative pre-routing warms the reflex cache to reduce execution latency upon final transcript submission (reduction unmeasured).
- Unit and integration tests pass (full JS suite 96/96 green as of 2026-09-22, including `tests/stt_streaming.test.mjs`; Rust tests not yet run — no toolchain in this environment).
