# Plan 13 — Reliability and E2E test system

**Priority:** P1  
**Status:** CODE COMPLETE (ACCEPTANCE PENDING)

## Objective

Move from “it compiles” to reproducible proof that voice requests actually
complete correctly on real OS behavior.

## Test layers

1. **unit**: routers, schemas, policy, state machine (`tests/unit_reliability.test.mjs`, `tests/router.test.mjs`).
2. **component**: STT/model manager/process supervisor (`tests/reliability_component.test.mjs`).
3. **integration**: accessibility/browser/harness adapters (`tests/reliability_integration.test.mjs`, `tests/browser.test.mjs`).
4. **E2E**: audio -> transcript -> action -> verified state (`tests/reliability_e2e.test.mjs`).
5. **soak/fault injection**: comprehensive failure injection suite covering all 13 required scenarios (`tests/reliability_fault_injection.test.mjs`).

## Corpus

All corpora generated and checked in:
- `tests/fixtures/corpora/speech_audio_corpus.jsonl`: 12 entries with 16kHz mono 16-bit PCM WAV audio files covering multiple languages (en, it, es, fr, de), accents (en-US, en-GB, en-IN, it-IT, es-ES, fr-FR, de-DE), speech rates (normal, fast, slow), and noise levels (clean, moderate, noisy, chatter).
- `tests/fixtures/corpora/linguistic_edge_cases.jsonl`: 22 entries covering negation ("don't close Chrome", "do not delete that folder", "never send that email", "actually stop"), corrections ("open Spotify no wait open Chrome", "search for rust actually search for python"), ambiguous app/window labels ("open terminal", "code", "switch window"), and destructive requests ("format drive C", "delete all files in documents", "drop database production", "rm -rf /").
- `tests/fixtures/corpora/device_transitions.jsonl`: 9 entries covering Bluetooth audio profile transitions (A2DP to HFP), default microphone disconnection, sample rate changes (44.1k/48k to 16k), microphone permission revocation, system suspend/sleep and wake/resume, offline/online network transitions, and display DPI scaling changes.
- `tests/fixtures/browser/spa_mutation.json`: scenarios covering stale DOM element references, client-side route changes mid-action, dynamic modal backdrops, and bot/captcha challenges.
- `tests/fixtures/audio/*.wav`: 12 synthetic 16kHz 16-bit mono PCM reference audio fixtures with valid 44-byte RIFF/WAVE headers.

## Required failure injection

All 13 failure injections tested in `tests/reliability_fault_injection.test.mjs`:
1. `model download interrupted`: partial staging download cleaned up, failed state recorded, no partial artifact activated.
2. `disk full`: disk space preflight fails closed with `disk-full` before allocating staging/active buffers.
3. `mic denied/unplugged`: microphone disconnect or permission denial safely transitions runtime to idle/attention without crashing.
4. `STT crash`: process supervisor detects child crash, harvests exit status, and watchdog restarts engine.
5. `planner timeout`: planner request deadline aborts request cleanly and falls back to deterministic reflex.
6. `port conflict`: port 8787 occupancy detected, reports `port-conflict` error and falls back to alternate port.
7. `two instances`: mutex/lockfile detects existing running instance and prevents secondary launch.
8. `hotkey conflict`: global shortcut conflict detected, keeps UI and tray click-through operational.
9. `stale UI element`: DOM detachment returns `stale-ref` error, triggering automatic re-inspection rather than clicking invalid coordinates.
10. `browser SPA mutation`: tab navigation during action returns `spa-mutation`, cancelling dependent subactions.
11. `owned agent crash`: owned harness agent exit code harvested, resources freed, error event emitted.
12. `quit during action`: global cancel token fires, in-flight actions halted, owned children terminated with bounded grace period, no orphans remain.
13. `corrupt config/cache`: invalid JSON settings safely quarantined, falling back to `AppSettings::default()`.

## Definition of done

A versioned test matrix is checked in at `tests/matrix.json`:
- Reports completion rate, false-action rate, cancel latency (p50 and p95), and crash-free soak hours per OS (Windows, macOS, Linux) against release targets.
- Documents physical-audio, sleep/resume, Bluetooth, DPI scaling, and permission revocation cases as lab-gated with deterministic seams.
- `.github/workflows/ci.yml` expanded with a Windows+macOS+Linux matrix running `cargo check` and `cargo test --manifest-path src-tauri/Cargo.toml`.
