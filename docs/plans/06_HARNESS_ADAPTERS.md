# Plan 06 — Structured AI harness adapters

**Priority:** P1  
**Status:** IMPLEMENTED (Acceptance Pending)

## Objective

Control external AI coding/agent harnesses as real sessions: launch, send,
observe, interrupt, resume and close.

## Adapter contract

- detect
- capabilities
- start(cwd, prompt)
- send(message)
- interrupt
- resume
- status
- stream_events
- artifacts/diff
- close

## Integration order

1. native SDK (`native_sdk`)
2. ACP (`acp`)
3. structured local API (`local_api`)
4. JSONL/structured CLI (`jsonl_cli`)
5. plain CLI (`plain_cli`)
6. UI automation only as last resort (`ui_fallback`, marked with `via:ui-fallback`)

## Initial targets

- Codex (`CodexAdapter`): verified `codex exec` JSONL CLI, prompt injection enabled
- OpenCode (`OpenCodeAdapter`): `--help` ACP probing with plain CLI fallback, conservative prompt injection
- Kilo (`KiloAdapter`): `--help` ACP probing with plain CLI fallback, conservative prompt injection
- Claude Code (`ClaudeAdapter`): plain CLI, conservative prompt injection
- Gemini CLI (`GeminiAdapter`): plain CLI, conservative prompt injection
- Generic ACP (`GenericAcpAdapter`): ACP daemon RPC transport
- UI Fallback (`UiFallbackAdapter`): last-resort fallback with explicit `via:ui-fallback` marker

## Work packages

- **stable session IDs**: Formatted as `hs_<8-hex>` based on supervisor process identifier.
- **process ownership integration**: All harness sessions spawn strictly through `ProcessSupervisor` with `OwnershipClass::OwnedSession`. Quitting ReflexDesk terminates all owned trees; no orphaned grandchildren.
- **structured event normalization**: Normalized events emit `{seq, session_id, kind, text_delta?, state?, exit_code?, summary?, timestamp_ms}`.
- **stop/cancel wiring**: `interrupt()` and `resume()` wire directly to `crate::policy::cancel_session()` and `crate::policy::clear_session()`. Global cancel cascades to session interruption.
- **project/worktree awareness**: Sessions accept validated `cwd`; `artifacts_diff()` captures git status and diff stat.
- **diff/test/result summaries**: Structured `ArtifactsDiff` reports `files_changed`, `additions`, `deletions`, and `diff_summary`.
- **safe prompt forwarding**: Only verified CLI contracts (`codex exec`) launch with CLI prompt injection. All other adapters launch conservatively without arbitrary argument injection.
- **capability discovery rather than hard-coded assumptions**: Probing via `probe_executable()` inspects `--version` and `--help` for flags (such as `--acp`).
- **deterministic health check**: `harness::health() -> bool` verifies module integrity; `harness::harness_health(id)` provides structured status per harness.

## Definition of done

User can start an agent by voice, continue the same session, interrupt it
deterministically, inspect what changed, and quit ReflexDesk without orphaning
the owned session.
