# Plan 05 — Embedded reflex / Laya layer

**Priority:** P1  
**Status:** NOT STARTED

## Objective

Make the fast semantic decision layer truly turnkey and measurable. Laya cannot
remain an optional localhost service that normal users must start themselves.

## Work packages

1. Define ReflexProvider interface:
   route(candidates, context) -> choice / null / confidence.
2. Benchmark Laya against deterministic routing and compact alternatives.
3. Select deployment form: embedded library, ONNX/native runtime or managed
   child sidecar.
4. Package exact model/runtime with license metadata.
5. Start/health/restart automatically through ProcessSupervisor.
6. Cache decisions where the semantic state is unchanged.
7. Calibrate confidence against actual action correctness.
8. Add language-specific benchmark coverage.

## Decision rule

Keep Laya only if it materially improves ambiguity resolution without harming
latency/reliability. Deterministic rules remain the fastest tier.

## Definition of done

No normal-user configuration is required; the reflex layer is either available
automatically or cleanly bypassed, and confidence thresholds are based on a
reproducible labeled benchmark rather than intuition.
