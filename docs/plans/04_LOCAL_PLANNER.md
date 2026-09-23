# Plan 04 — Turnkey local planner

**Priority:** P1  
**Status:** CODE COMPLETE (ACCEPTANCE PENDING)  
**Depends on:** policy/action schemas

## Objective

A normal user must not need Ollama, LM Studio or endpoint configuration.
ReflexDesk should ship or provision a compact local tool-use planner and select
it automatically.

## Adapter

Define LocalPlanner:
- health()
- capabilities()
- plan(context, request) -> typed ActionEnvelope[]
- cancel(session)
- unload()

The planner may plan; it may not execute.

## Work packages

1. Build a benchmark corpus from real ReflexDesk tasks.
2. Evaluate current compact multilingual/tool-use candidates.
3. Select at least low/mid/high hardware tiers.
4. Implement a native or managed local runtime adapter.
5. Lazy load/unload and memory-pressure behavior.
6. Constrained structured output / grammar.
7. Planner context budgeter using semantic desktop/browser state.
8. Fallback/escalation rules when confidence is low.

## Metrics

- tool selection correctness
- argument correctness
- task completion
- first-action latency
- total latency
- peak RAM/VRAM
- multilingual behavior

## Definition of done

Fresh install can handle a novel multi-step local task without the user
installing a model server or editing an endpoint, and incorrect planner output
cannot bypass schemas/policy.

## Implementation and Acceptance Notes (Wave 2)

- **Adapter trait:** `src-tauri/src/planner.rs` implements `LocalPlanner` with `health()`, `capabilities()`, `plan()`, `cancel()`, and `unload()`. The planner only plans; execution is strictly delegated to the policy gate via `ActionEnvelope`.
- **Benchmark corpus:** `tests/fixtures/planner/corpus.jsonl` contains 34 realistic ReflexDesk tasks across tool selection, argument validation, multi-step flows, multilingual tasks (Italian, Spanish, French, German, Portuguese), ambiguous out-of-scope requests, and adversarial prompt injections.
- **Baseline planner:** `BuiltinPlanner` is compiled into ReflexDesk and is always available on a fresh install. It handles high-confidence app opening, browser navigation/search, desktop focus, explicit agent delegation, and explicit multi-step sequencing without any model server or endpoint.
- **Registry cleanup:** placeholder low/mid/high planner checkpoints with no real artifact identity were removed. `models/registry.json` now declares the built-in planner as the default and keeps the OpenAI-compatible local endpoint only as an optional escalation interface.
- **Runtime backends:**
  - `BuiltinPlanner`: zero-configuration deterministic baseline; it only emits schema-validated `ActionEnvelope` values and never executes directly.
  - `OpenAiCompatibleLocalPlanner`: optional loopback/approved endpoint adapter for requests outside the deterministic grammar, with consent and endpoint policy gates.
  - The previous `CrispAsrChatPlanner` seam remains fail-closed unless real planner weights/runtime are provisioned; it is no longer presented as the fresh-install default.
- **Constrained output & repair:** `repair_and_parse_json` strips markdown code blocks and repairs truncated braces/quotes. Any parse or validation failure maps strictly to `unknown` action envelope with no execution.
- **Context budgeter:** `PlannerContext::budgeted_text` caps desktop accessibility and browser semantic state to 4k tokens (~16k characters), truncating at UTF-8 boundaries.
- **Privacy & Redaction:** Prompts route through `crate::redaction::redact_text` prior to any logging or diagnostic emission.
- **Fresh install behavior:** planner health is true with no external runtime. Ordinary explicit multi-step commands are planned locally. Ambiguous requests fail closed with `planner-needs-model` unless an optional model backend is healthy; no request can bypass schema validation or the policy gate.
