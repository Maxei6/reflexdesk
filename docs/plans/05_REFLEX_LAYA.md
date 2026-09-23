# Plan 05 — Embedded reflex / Laya layer

**Priority:** P1  
**Status:** IN PROGRESS (local inference integration; benchmark and turnkey packaging pending)

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
   Local smoke run on the 53 labeled samples (Windows CPU, alongside other installs):
   raw top-choice accuracy 7/53, ambiguous 4/15, p50 469 ms, p95 8350 ms.
   At the provisional 0.70 threshold, zero samples yielded an actionable
   `browser.search` prediction. These are one-machine observations, not a
   reproducible performance claim; they fail the decision rule.

3. **Select deployment form and package runtime:**
   Uses an explicitly prepared Python virtualenv and verified multilingual weights in
   `~/.reflexdesk/laya`. `scripts/prepare_laya.py` downloads once while online;
   the sidecar loads a local path with Hugging Face offline flags and never substitutes
   a heuristic while claiming to be Laya. The ordinary installer does **not** bundle
   Python, PyTorch, or the 644 MB checkpoint.

4. **Start and health-check the supervised process:**
   The supervised child uses a random loopback port, ephemeral bearer token,
   authenticated health checks and a bounded startup wait. It starts only after
   the explicit preparation step has installed a runtime and checkpoint. Activation benchmarks
   the real sidecar; on a failed gain/latency gate it shuts the process down and
   retains deterministic/compact routing. Cold-load/crash-recovery timing still
   needs dedicated measurement.

5. **Cache decisions where the semantic state is unchanged:**
   Implemented `ReflexCache` keyed by `(normalized_text, semantic_state_hash)` with 60-second TTL
   and automatic cache pruning.

6. **Calibrate confidence against real routing accuracy:**
   The current 0.70 confidence cutoff is provisional, **not empirically calibrated**.
   Laya is not selected unless the real-checkpoint benchmark meets the decision
   rule. Even then, only browser search can use the original utterance as query;
   target-bearing actions must go through deterministic/planner extraction and policy.

7. **Add language-specific benchmark coverage:**
   Corpus `tests/fixtures/reflex/corpus.jsonl` contains labeled samples in English (`en`), Italian (`it`),
   Spanish (`es`), French (`fr`), and German (`de`) with per-language accuracy breakdown.

## Decision rule

Keep Laya only if it materially improves ambiguity resolution (>= 15% gain on ambiguous queries)
without harming latency/reliability (p95 <= 250ms). Deterministic rules remain the fastest tier.
When Laya is unavailable or fails the gate, deterministic and compact local
routing remains available with zero cloud calls in offline mode. The current
checkpoint fails the measured gate; 0.70 is not a calibrated threshold.

## Definition of done (not yet met)

- Install and verify a local checkpoint without requiring Python on an end-user machine.
- Supervised Laya endpoint and health check authenticated with ephemeral tokens.
- Fast deterministic actions remain LLM-free and offline.
- Measure real-checkpoint accuracy, precision and p95 on the multilingual corpus.
- Prefer Laya only if its ambiguity gain and latency meet the decision rule above.
