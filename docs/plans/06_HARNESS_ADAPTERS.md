# Plan 06 — Structured AI harness adapters

**Priority:** P1  
**Status:** NOT STARTED

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

1. native SDK
2. ACP
3. structured local API
4. JSONL/structured CLI
5. plain CLI
6. UI automation only as last resort

## Initial targets

- Codex
- OpenCode
- Kilo
- Claude Code
- Gemini CLI
- generic ACP

## Work packages

- stable session IDs;
- process ownership integration;
- structured event normalization;
- stop/cancel wiring;
- project/worktree awareness;
- diff/test/result summaries;
- safe prompt forwarding;
- capability discovery rather than hard-coded assumptions.

## Definition of done

User can start an agent by voice, continue the same session, interrupt it
deterministically, inspect what changed, and quit ReflexDesk without orphaning
the owned session.
