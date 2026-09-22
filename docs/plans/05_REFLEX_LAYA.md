# Plan 05 — Embedded reflex / Laya layer

**Priority:** P1  
**Status:** COMPLETE (Acceptance Pending Integration Verification)

## Objective

Make the fast semantic decision layer truly turnkey and measurable. Laya cannot
remain an optional localhost service that normal users must start themselves.

## Work packages

1. **Define ReflexProvider interface:**
   Implemented in `src-tauri/src/reflex.rs` via `trait ReflexProvider`:
   `route(&self, candidates: &[CandidateAction], ctx: &ReflexContext) -> Option<(String, f32)>`.
   Provides candidate action descriptions, semantic context hashing, and confidence scoring.

2. **Benchmark Laya against deterministic routing and compact alternatives:**
   Implemented `run_corpus_benchmark()` in `src-tauri/src/reflex.rs` and labeled multilingual
   benchmark corpus in `tests/fixtures/reflex/corpus.jsonl`. Measures accuracy, ambiguity resolution gain,
   and p50/p95 latency across Deterministic (Tier-0), Compact Heuristic, and Laya sidecar.

3. **Select deployment form:**
   Selected managed child sidecar supervised via `ProcessSupervisor` with `OwnershipClass::Internal`
   and ephemeral bearer token auth (`sidecar/laya_service.py`), backed by a compact local heuristic
   classifier (`CompactReflex`) for zero-dependency offline environments.

4. **Package exact model/runtime with license metadata:**
   Registered in `models/registry.json` (`laya-english` and `laya-multilingual`, Apache-2.0 license,
   `convaiinnovations/laya` upstream).

5. **Start/health/restart automatically through ProcessSupervisor:**
   Implemented in `LayaSupervisor` (`src-tauri/src/reflex.rs`):
   - Ephemeral loopback port allocation (`127.0.0.1:0`).
   - Secure random 64-hex bearer token generation (`LAYA_AUTH_TOKEN`).
   - Authenticated `/health` check with bearer token verification.
   - Auto-restart watchdog with bounded backoff and clean bypass on missing runtime.

6. **Cache decisions where the semantic state is unchanged:**
   Implemented `ReflexCache` keyed by `(normalized_text, semantic_state_hash)` with 60-second TTL
   and automatic cache pruning.

7. **Calibrate confidence against actual action correctness:**
   Empirical threshold calibrated at 0.70 confidence (ensuring >= 95% precision on the labeled corpus).
   Low-confidence predictions fail closed or escalate to the planner.

8. **Add language-specific benchmark coverage:**
   Corpus `tests/fixtures/reflex/corpus.jsonl` contains labeled samples in English (`en`), Italian (`it`),
   Spanish (`es`), French (`fr`), and German (`de`) with per-language accuracy breakdown.

## Decision rule

Keep Laya only if it materially improves ambiguity resolution (>= 15% gain on ambiguous queries)
without harming latency/reliability (p95 <= 250ms). Deterministic rules remain the fastest tier.
When Laya is unavailable or uncalibrated, system defaults cleanly to `reflex.provider=deterministic`
with zero cloud calls in offline mode.

## Definition of done

- No normal-user configuration is required; the reflex layer is either available automatically
  or cleanly bypassed.
- Supervised Laya endpoint and health check are authenticated with ephemeral bearer tokens.
- Fast reflexive actions remain LLM-free and function 100% offline.
- Confidence thresholds are grounded in a reproducible labeled benchmark rather than intuition.
