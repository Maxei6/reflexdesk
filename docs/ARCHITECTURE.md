# ReflexDesk architecture

## Execution tiers

### Tier 0 — deterministic command cache
Known safe commands and compiled skills execute directly.

### Tier 1 — reflex model
A fast typed decision model selects among known intents/actions using current desktop/browser state.

### Tier 2 — compact local planner
Novel multi-step requests are decomposed into typed tool calls by a local generative model.

### Tier 3 — external harness
Coding/research/agent-heavy work is delegated through a structured harness adapter.

### Tier 4 — optional cloud escalation
Only when the user explicitly enables an online provider.

## Core boundaries
- audio: microphone, VAD, streaming STT
- reflex: fast typed decision adapters
- planner: local generative planning adapters
- tools: typed tool schemas/execution
- policy: allow/deny/confirm
- browser: semantic browser state/control
- desktop: platform accessibility/control
- harness: ACP/SDK/CLI integrations
- overlay: reactive listening/working state
- benchmark: hardware + task correctness
- models: downloadable model registry

## Rule
The generative planner is a fallback, not the default loop.
