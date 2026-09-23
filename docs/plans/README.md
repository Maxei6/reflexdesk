# ReflexDesk production plans

This directory is the execution authority for work beyond P0.

P0 desktop productization is already implemented. These plans describe what must
happen before ReflexDesk should be called production-grade.

## Rules

- Do not work from README wish-lists. Work from the relevant plan here.
- One subsystem plan per development branch.
- A plan is not DONE because code exists; its acceptance criteria and tests must pass.
- Do not silently weaken offline/privacy/safety invariants to improve demos.
- Prefer native semantic interfaces over screenshots and pixel clicking.
- Powerful tools must land behind the policy engine, never ahead of it.
- Every new backend must have a deterministic health check and failure mode.
- Stable-release gates are separate from feature-complete gates.

## Recommended execution order

| Order | Plan | Priority | Status |
| --- | --- | --- | --- |
| 1 | [Policy + Cancel](01_POLICY_AND_CANCEL.md) | P0.5 | NOT STARTED |
| 2 | [Desktop Control](02_DESKTOP_CONTROL.md) | P1 | PARTIAL |
| 3 | [Browser Control](03_BROWSER_CONTROL.md) | P1 | ACCEPTANCE PENDING |
| 4 | [Local Planner](04_LOCAL_PLANNER.md) | P1 | NOT STARTED |
| 5 | [Reflex / Laya](05_REFLEX_LAYA.md) | P1 | NOT STARTED |
| 6 | [Harness Adapters](06_HARNESS_ADAPTERS.md) | P1 | NOT STARTED |
| 7 | [Security Hardening](07_SECURITY_HARDENING.md) | P1 | NOT STARTED |
| 8 | [Secret Storage](08_SECRET_STORAGE.md) | P1 | ACCEPTANCE PENDING |
| 9 | [Process Ownership](09_PROCESS_OWNERSHIP.md) | P1 | ACCEPTANCE PENDING |
| 10 | [Model Manager](10_MODEL_MANAGER.md) | P1 | NOT STARTED |
| 11 | [Hardware Optimization](11_HARDWARE_OPTIMIZATION.md) | P1 | NOT STARTED |
| 12 | [Native / Streaming STT](12_STT_NATIVE_STREAMING.md) | P2 | NOT STARTED |
| 13 | [Reliability Testing](13_RELIABILITY_TESTING.md) | P1 | NOT STARTED |
| 14 | [Observability](14_OBSERVABILITY_DIAGNOSTICS.md) | P1 | NOT STARTED |
| 15 | [Localization](15_LOCALIZATION.md) | P2 | NOT STARTED |
| 16 | [Tray + Visual Polish](16_TRAY_ICON_VISUAL_POLISH.md) | P2 | NOT STARTED |
| 17 | [Signing + Updater](17_SIGNING_UPDATER_RELEASES.md) | RELEASE BLOCKER | BLOCKED ON CREDENTIALS |
| 18 | [Skills / Reusable Automation](18_SKILLS_AUTOMATION.md) | P2 | NOT STARTED |
| 19 | [Release Candidate Soak](19_RELEASE_CANDIDATE_SOAK.md) | RELEASE BLOCKER | NOT STARTED |

## Milestones

### M1 — Reliable control engine
Plans 01–06. A normal user can issue a natural-language request and ReflexDesk
can inspect, act, verify and cancel without guessing screen coordinates.

### M2 — Hardened local runtime
Plans 07–14. Security boundaries, secrets, child processes, models, hardware
selection, tests and diagnostics are production-grade.

### M3 — Product polish
Plans 15–18. Localization, tray polish and reusable skills make the app feel
complete rather than technical.

### M4 — Stable release
Plan 19 plus signing/updater requirements. No stable tag until the release
checklist is satisfied.

## Global definition of production-grade

ReflexDesk may be called production-grade only when:

1. common desktop/browser tasks use semantic state and post-action verification;
2. destructive/high-impact actions cannot bypass policy or cancellation;
3. normal users do not need to understand models, endpoints or runtimes;
4. setup, upgrades, sleep/resume, device switching and crash recovery are tested;
5. owned child processes cannot be orphaned by normal exit or crash;
6. secrets are never stored in plaintext app settings/localStorage;
7. stable binaries are signed/notarized and updates are signature-verified;
8. local logs are useful but privacy-safe;
9. the release candidate survives the soak plan with no unresolved blockers.
