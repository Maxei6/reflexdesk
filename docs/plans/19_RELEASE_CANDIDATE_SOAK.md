# Plan 19 — Release candidate soak

**Priority:** RELEASE BLOCKER  
**Status:** NOT STARTED

## Objective

Prove the integrated product survives normal usage and ugly machine states
before a stable release.

## Entry criteria

- core P1 plans accepted;
- no unresolved critical/high security finding;
- signed/notarized release workflow available;
- updater staging available;
- representative E2E suite green.

## Soak matrix

Operating systems:
- supported Windows versions;
- current + previous supported macOS;
- representative Ubuntu/Fedora-class Linux targets.

Hardware:
- low-end CPU-only;
- modern x86;
- Apple Silicon;
- NVIDIA GPU;
- multiple DPI/monitor arrangements.

Scenarios:
- 24h and 72h background runtime;
- repeated listen cycles;
- sleep/resume;
- network loss/return;
- microphone/Bluetooth switching;
- agent start/stop;
- model/update interruption;
- forced crashes and recovery;
- uninstall/reinstall/update;
- shortcut conflicts;
- disk pressure.

## Release thresholds

Freeze quantitative gates before the soak:
- crash-free sessions;
- task completion rate;
- false-action rate;
- cancel latency;
- startup/readiness time;
- memory growth;
- orphan-process count;
- update success/rollback rate.

Do not move thresholds after seeing poor results without documenting why.

## Exit criteria

No blocker/major defect remains open; all known limitations are documented;
artifacts are signed, updater-tested and reproducibly associated with the final
commit SHA.
