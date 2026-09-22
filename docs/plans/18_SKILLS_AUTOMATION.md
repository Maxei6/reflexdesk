# Plan 18 — Reusable local skills

**Priority:** P2  
**Status:** NOT STARTED  
**Depends on:** control engine + policy + verification

## Objective

Turn repeated successful workflows into fast deterministic local skills without
giving generated code unrestricted machine access.

## Model

A skill is a versioned declarative workflow of typed ReflexDesk actions with:
- inputs
- preconditions
- policy requirements
- ordered/conditional steps
- verification
- timeout/cancel behavior

Do not store arbitrary shell code as a “skill”.

## Work packages

1. Skill schema and validator.
2. recorder/compiler from successful verified action traces.
3. user review before saving.
4. parameterization.
5. deterministic execution runtime.
6. version/migration support.
7. import/export with trust metadata.
8. skill permissions and revocation.

## Definition of done

A repeated workflow such as “Start work” can execute without a planner, remains
fully cancelable/policy-controlled, and failures stop at the first unverifiable
step rather than blindly continuing.
