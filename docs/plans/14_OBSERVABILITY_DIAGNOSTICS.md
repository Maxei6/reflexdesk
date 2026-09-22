# Plan 14 — Privacy-safe observability

**Priority:** P1  
**Status:** ACCEPTANCE PENDING
**Implementation:** Code-complete (Wave 3)
## Objective

Make failures debuggable without turning ReflexDesk into telemetry spyware.

## Local logs

Structured rotating logs:
- runtime lifecycle
- STT health/performance
- action/tool results
- process ownership
- updater/model manager

Default redaction:
- no audio
- no secrets
- no clipboard/file contents
- no full browser text
- no transcripts unless explicitly enabled for debugging

## Diagnostics bundle

User-triggered export contains:
- app version/build
- OS/hardware class
- runtime/model versions
- benchmark numbers
- recent sanitized errors/state transitions
- installed adapter availability

Preview contents before export.

## Optional telemetry

Off by default. If ever added:
- explicit opt-in
- aggregate operational metrics only
- separate consent for crash reports
- no conversation/transcript collection

## Definition of done

A support issue can be diagnosed from an intentionally exported sanitized bundle
without asking the user to expose private command history.

## Acceptance Notes & Evidence

1. **Structured Rotating Local Logs:**
   - Implemented in `src-tauri/src/observability.rs` matching `{ v: 1, ts, component, op, session, phase, outcome, latency_ms, code, meta }` event schema.
   - In-memory bounded circular buffer (1,000 events) and disk-persisted rotating logs (`reflexdesk.log`, `.1`, `.2`, `.3`, max 1 MB per file, bounded to ~3 MB total).

2. **Strict Default Redaction at All Sinks:**
   - Redaction routed through `redact_text` and `redact_error` before persisting or emitting.
   - Zero raw audio samples or audio buffers.
   - Zero secrets (API keys, bearer tokens, passwords, private keys, client secrets stripped/redacted).
   - Zero clipboard contents, file contents, full browser text, or raw prompts.
   - Transcripts are NEVER stored or logged unless explicitly enabled for debugging via `debug_transcripts_enabled()` or `REFLEXDESK_DEBUG_TRANSCRIPTS=1`.

3. **Sanitized Diagnostics Bundle:**
   - `get_diagnostics_preview` and `export_diagnostics` return identical `DiagnosticsBundle` objects.
   - Top-level fields strictly conform to Wave-0 allowlisted fields (`app_version`, `build`, `os`, `arch`, `hardware_class`, `runtime_version`, `model_id`, `model_revision`, `benchmark_numbers`, `sanitized_transitions`, `sanitized_errors`, `adapter_availability`, `recent_logs`).
   - Excludes secrets by construction.

4. **Cancel-Leaves-No-File Export Semantics:**
   - Export writes to a temporary file (`.tmp-<timestamp>`) with disk sync, followed by atomic rename to the target path.
   - If export fails, is cancelled, or destination is invalid, any temporary file is deleted immediately and no corrupted or partial file is left on disk.

5. **UI Controls & Preview:**
   - Advanced card in `index.html` and `src/main.js` includes "Preview" and "Export" buttons with pre-export formatted preview container and status reporting.

6. **Verification:**
   - Comprehensive Node test suite in `tests/observability.test.mjs` (6/6 passing).
   - Full project test suite passing: `npm test` (53/53 tests green).
