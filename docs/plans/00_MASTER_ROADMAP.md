# Master roadmap

## Product objective

ReflexDesk should feel like an operating-system capability, not an AI demo:
install it, complete a short local setup, then control the desktop/browser/agents
by voice with predictable latency, visible state and deterministic safety.

## Architecture target

```text
Voice
  -> local STT
  -> deterministic reflex / semantic router
  -> optional local planner
  -> policy + cancel gate
  -> desktop/browser/harness adapter
  -> execute
  -> verify resulting state
  -> report success/failure
```

The model never receives unrestricted machine authority.

## Development sequence

### Phase A — control foundation
- policy/cancel envelope
- accessibility-tree desktop model
- browser DOM/CDP model
- typed action verification
- planner only emits typed actions

### Phase B — intelligence
- turnkey local planner
- packaged reflex/Laya runtime
- structured harness sessions
- hardware-based backend selection

### Phase C — hardening
- strict Tauri capabilities/CSP
- OS secret storage
- process-tree ownership
- model lifecycle manager
- structured logs and diagnostics
- automated regression/E2E matrix

### Phase D — release
- localization
- visual/tray polish
- signed/notarized installers
- signed updater channels
- release-candidate soak

## Architectural invariants

- Offline mode makes zero AI cloud calls.
- Stop/cancel must not depend on an LLM.
- Native accessibility/DOM/CDP precede vision.
- Pixel clicking is a fallback and must be explicitly identified as such.
- Every tool has risk metadata and verification rules.
- Every owned process has an owner, lifecycle and cleanup path.
- Every model/runtime has exact identity, license and health checks.
- UI state must reflect actual runtime state, never optimistic state.
- Stable releases fail closed if signing/update validation is incomplete.

## Success metric

The primary milestone is not command count. It is reliable completion rate on a
small, representative task suite across supported OSes.

A task only counts as successful when the requested final state is verified.
