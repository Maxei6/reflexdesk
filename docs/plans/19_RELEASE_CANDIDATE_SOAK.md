# Plan 19 — Release candidate soak

**Priority:** RELEASE BLOCKER  
**Status:** IN PROGRESS / BLOCKED ON HARDWARE

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

## Acceptance notes

### Implemented Scaffolding and Tooling

1. **Frozen Release Thresholds (`tests/soak/thresholds.json`)**:
   - Quantitative gates frozen prior to soak execution:
     - `crash_free_sessions_pct`: min 99.9%
     - `crash_free_runtime_hours`: min 72.0 hours
     - `task_completion_rate`: min 0.98
     - `false_action_rate`: max 0.01
     - `cancel_latency_p95_ms`: max 200 ms
     - `startup_readiness_p95_ms`: max 3000 ms
     - `max_memory_growth_mb_per_24h`: max 50.0 MB
     - `orphan_process_count`: max 0 (strict 0 tolerated)
     - `update_success_rate`: min 0.99
     - `update_rollback_rate`: max 0.00 (zero corruption tolerated)
   - Enforces the rule: thresholds cannot be relaxed post-test without documented architectural exception.

2. **Soak Matrix Scaffolding (`tests/soak/matrix.json`)**:
   - **Operating systems**: Windows 11 (24H2/23H2/22H2), Windows 10 (22H2/21H2), Windows Server 2025/2022; macOS 15 Sequoia (Apple Silicon), macOS 14 Sonoma (Apple Silicon & Intel x86_64); Ubuntu 24.04 LTS, Ubuntu 22.04 LTS, Fedora 40/41.
   - **Hardware profiles**: Low-end CPU-only (2-4 cores, no AVX2), modern x86, Apple Silicon (M-series, Metal/ANE), NVIDIA discrete GPU (CUDA >= 7.0), mixed-DPI multi-monitor arrangements.
   - **12 Scenarios**: 24h background, 72h soak, repeated listen cycles (1000+), sleep/resume, network loss, mic/Bluetooth switching, agent lifecycle, model/update interruption, forced crashes and recovery, setup/upgrade/uninstall, shortcut conflicts, disk pressure.
   - **Unmeasured values strictly null**: In accordance with the contract, all unmeasured physical fleet values remain strictly `null` and are never asserted as product claims.

3. **Release Blocker Registry (`tests/soak/blockers.json`)**:
   - Structured registry tracking exit criteria blockers:
     - `BLK-001`: Multi-OS 72-hour physical fleet hardware soak (`BLOCKED_ON_HARDWARE`).
     - `BLK-002`: Production code signing and notarization credentials (`BLOCKED_ON_CREDENTIALS`).
     - `BLK-003`: Cross-platform semantic desktop adapters (`OPEN`).
     - `BLK-004`: Signed updater activation and rollback (`OPEN`).
   - Tracks known limitations (LIM-001 for HTTP loopback STT seam, LIM-002 for local planner fallback).
   - Exit criteria remain false until all code, hardware, and credential blockers resolve.

4. **Entry Criteria Checklist Tooling (`scripts/soak-entry-check.mjs`)**:
   - Evaluates authoritative plan status, not plan-file presence.
   - Requires both dependency lockfiles for a passing security result.
   - Treats unmeasured OS metrics as `WARN`, never as a fabricated E2E pass.
   - Reports the current desktop-plan gap as `FAIL`, so the release candidate is
     not ready to begin production soak.
   - Supports `--json` and `--strict` execution modes.

5. **Soak Collection & Redaction-Routed Observability (`scripts/soak-collect.mjs`)**:
   - Ingestion and metrics computation for structured observability logs (`LogEvent`).
   - Strict Wave-0 redaction pipeline:
     - Strips tokens, passwords, API keys (`sk-*`, `rd_*`, Bearer tokens).
     - Redacts URLs to scheme + host only.
     - Sanitizes metadata objects against `DENIED_META_KEYS` (`api_key`, `token`, `transcript`, `audio`, `clipboard`, `file_content`, `browser_text`, `prompt`).
     - Enforces `ALLOWLISTED_FIELDS` (`app_version`, `build`, `arch`, `os`, `hardware_class`, `phase`, `latency_ms`, `code`, `outcome`, etc.).
   - Reuses 13 fault-injection hooks from Plan 13 in `--simulate` mode, benchmarking cancel latencies and resilience.
   - Matrix verification gate (`--check` / `--verify-matrix`) ensuring unmeasured hardware fleet targets are flagged as `BLOCKED ON HARDWARE`.

6. **Manual Soak CI Workflow (`.github/workflows/soak.yml`)**:
   - `workflow_dispatch` workflow targeting `[windows-latest, macos-latest, ubuntu-24.04]`.
   - Runs entry criteria check, fault-injection simulation, and records `BLOCKED ON HARDWARE` in step summary.
   - Supports `strict_gates: true` input to fail closed if physical hardware fleet is unmeasured.

7. **Test Verification (`tests/soak.test.mjs`)**:
   - 8 unit and integration tests passing:
     - Thresholds definition and quantitative gate constraints.
     - Matrix coverage across all required operating systems, hardware, and scenarios.
     - Blocker registry schema and open blocker tracking.
     - Entry criteria evaluation logic.
     - Observability log redaction (text, URL, metadata, and event allowlisting).
     - Metrics calculation (completion rate, false-action rate, cancel percentiles).
     - Fault-injection simulation execution (13/13 hooks pass).
     - Hardware audit reporting `BLOCKED ON HARDWARE` for unmeasured fleet targets.

### Unmet Criteria and Residual Blockers

- **Multi-day physical hardware soak**: Running continuous 24h and 72h soak cycles across the physical fleet matrix (Apple Silicon M-series, low-end CPU-only x86, NVIDIA discrete GPU, physical Bluetooth headsets, and ACPI sleep transitions) requires physical machines. In local workstation and virtual CI environments, this remains `BLOCKED ON HARDWARE` (tracked in `BLK-001`). All metrics remain `null`.
- **Signed and notarized release artifacts**: Verification of signed binaries and updater staging depends on production certificates and keys from Plan 17 (tracked in `BLK-002` as `BLOCKED_ON_CREDENTIALS`).
- **Semantic desktop adapters**: Plan 02 remains partial until Windows UIA,
  macOS AX, and Linux AT-SPI adapters pass representative live tasks (`BLK-003`).
- **Updater activation and rollback**: cryptographic staging is implemented, but
  a signed installer activation/recovery path is still required (`BLK-004`).
