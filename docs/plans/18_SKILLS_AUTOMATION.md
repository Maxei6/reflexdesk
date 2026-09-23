# Plan 18 — Reusable local skills

**Priority:** P2  
**Status:** CODE COMPLETE (ACCEPTANCE PENDING)
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

## Acceptance notes

- **Skill schema & validator:** `schemas/skill.schema.json` defines required fields `[id, version, name, inputs, steps, verification, timeout_ms, cancel]`. Explicit deny-list rejects any field or key named `shell|cmd|script|code|command`.
- **Deterministic execution runtime:** `src-tauri/src/skills.rs::execute_skill` runs declarative workflows without any planner calls (`planner_calls = 0`). Every step flows through `policy::ActionEnvelope` with `ActionSource::User` through `execute_verified`. Cancellation propagates before and during execution (`policy::is_cancelled`). Execution halts immediately at the first unverifiable or failing step.
- **Recorder & compiler:** `SkillRecorder` hooks into `execute_verified` only after successful tool execution and post-action verification. `compile_draft` identifies repetition and proposes parameterized inputs.
- **User review before saving:** Saved skills require explicit validation and parameter confirmation via IPC/UI before persistence.
- **Version migration:** `migrate_v1_to_v2` automatically upgrades legacy v1 skills to v2 with default cancellation and verification contracts.
- **Trust metadata & revocation:** Imported skills track origin, signature, and permissions. Untrusted imports are quarantined (`trusted: false, enabled: false`) until explicit user grant. Permissions can be revoked at any time.
- **Fixtures & test coverage:** Five core fixtures in `tests/fixtures/skills/` (`start-work.json`, `reject-shell.json`, `reject-unverified.json`, `import-trust.json`, `migrate-v1.json`) verified by Node test suite `tests/skills.test.mjs` and Rust unit tests in `src-tauri/src/skills.rs`.
