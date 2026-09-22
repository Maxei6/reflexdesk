# Plan 04 — Turnkey local planner

**Priority:** P1  
**Status:** NOT STARTED  
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
