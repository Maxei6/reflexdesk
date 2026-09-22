# Plan 13 — Reliability and E2E test system

**Priority:** P1  
**Status:** NOT STARTED

## Objective

Move from “it compiles” to reproducible proof that voice requests actually
complete correctly on real OS behavior.

## Test layers

1. unit: routers, schemas, policy, state machine.
2. component: STT/model manager/process supervisor.
3. integration: accessibility/browser/harness adapters.
4. E2E: audio -> transcript -> action -> verified state.
5. soak/fault injection.

## Corpus

Include:
- multiple languages/accents
- fast/slow speech
- background noise
- negation/corrections
- ambiguous app/window labels
- destructive requests
- Bluetooth/default-device changes
- sleep/resume
- offline/online transitions

## Required failure injection

- model download interrupted
- disk full
- mic denied/unplugged
- STT crash
- planner timeout
- port conflict
- two instances
- hotkey conflict
- stale UI element
- browser SPA mutation
- owned agent crash
- quit during action
- corrupt config/cache

## Definition of done

A versioned test matrix reports completion rate, false-action rate, cancel
latency and crash-free soak results per OS. Release gates consume these metrics.
