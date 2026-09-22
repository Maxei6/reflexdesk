# Plan 04 — Turnkey local planner

**Priority:** P1  
**Status:** ACCEPTANCE PENDING  
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
- **Hardware tiers:** `models/registry.json` updated with low (`spark-x2.5-compact`), mid (`spark-x2.5`), high (`spark-x2.5-pro`), and flexible (`local-openai-compatible`) tier metadata.
- **Runtime backends:**
  - `CrispAsrChatPlanner`: native compact GGUF runtime adapter with lazy loading, 5-minute idle timeout unload, and deterministic weights preflight.
  - `OpenAiCompatibleLocalPlanner`: loopback HTTP adapter with strict offline consent validation (`allow_remote` + persisted `allow_online_ai` gates).
- **Constrained output & repair:** `repair_and_parse_json` strips markdown code blocks and repairs truncated braces/quotes. Any parse or validation failure maps strictly to `unknown` action envelope with no execution.
- **Context budgeter:** `PlannerContext::budgeted_text` caps desktop accessibility and browser semantic state to 4k tokens (~16k characters), truncating at UTF-8 boundaries.
- **Privacy & Redaction:** Prompts route through `crate::redaction::redact_text` prior to any logging or diagnostic emission.
- **Fresh install behavior:** With no local model or external server running, health check returns `false` and calls yield `planner-unavailable` without erroring setup, ensuring the deterministic reflex router remains the primary zero-dependency baseline.
