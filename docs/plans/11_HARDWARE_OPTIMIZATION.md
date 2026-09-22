# Plan 11 — Hardware benchmark and backend selection

**Priority:** P1  
**Status:** IMPLEMENTED (ACCEPTANCE PENDING)  
**Depends on:** model manager

## Objective

Choose the fastest correct local stack empirically for the machine instead of
using hardware-brand heuristics.

## Detect

- CPU architecture/features
- logical/physical cores
- RAM
- GPU vendor/device
- VRAM/unified memory
- CUDA availability
- Metal availability
- Vulkan availability
- supported runtime binaries

## Benchmark

Use bundled reference audio and a small labeled task set.

Rank candidates by:
1. correctness threshold
2. real-time factor
3. cold-start latency
4. steady-state latency
5. RAM/VRAM
6. power/CPU load where measurable

## Work packages

- HardwareProfile schema.
- backend candidate registry.
- benchmark runner with timeout.
- cached results keyed by hardware+runtime version.
- rebenchmark after major runtime/model changes.
- automatic fallback after backend crash/OOM.
- Advanced UI for benchmark results/override.

## Definition of done

Installer/setup can automatically choose CPU/CUDA/Metal/Vulkan where supported,
and the selected backend beats alternatives on the measured machine while
meeting correctness gates.

## Implementation & Acceptance Notes

### 1. Hardware Profile (`src-tauri/src/hardware.rs`)
- Empirical detection for OS, CPU architecture, vector features (AVX2, FMA, AVX-512, SSE4.2, NEON), logical and physical core counts, total RAM.
- GPU inspection queries registry display class adapters on Windows, system_profiler on macOS, and sysfs/proc on Linux. Explicit `Unknown` / `None` returned when vendor/device cannot be verified (zero brand heuristics).
- Detection for CUDA (`nvcuda.dll` / `nvidia-smi`), Vulkan (`vulkan-1.dll` / `libvulkan.so`), and Metal.
- Deterministic SHA-256 `hardware_fingerprint` uniquely identifies system hardware configuration for caching.
- Exposes deterministic `health() -> bool`.

### 2. Candidate Registry & Benchmark Runner (`src-tauri/src/benchmark.rs`)
- Registered candidates: `crispasr-cpu-avx2`, `crispasr-cpu-legacy`, `crispasr-cuda`, `crispasr-vulkan`, `crispasr-metal`, `localhost-http`.
- Uses bundled reference WAV audio (`tests/fixtures/audio/audio_en_us_clean.wav`) with header parsing for duration.
- Bounded-timeout runner (default 30s) measures cold-start latency, steady-state latency, Real-Time Factor (RTF), memory usage, and execution correctness.
- Strict ranking: correctness threshold gate first (passed > failed), then RTF (lower = faster), steady-state latency, cold-start latency, and peak RAM.
- Automatic selection of fastest passing candidate (`selected_candidate_id`) with safe portable fallback (`fallback_candidate_id`).
- Auto-fallback on runtime crash or OOM via `record_backend_failure`.
- Manual override capability via `set_backend_override`.
- Persistent cache keyed by `hardware_fingerprint:runtime_version:model_revision` with automatic invalidation on hardware, runtime, or model changes.
- Exposes deterministic `health() -> bool`.

### 3. ModelManager Readiness & Safety
- Model acquisition and activation remains strictly managed by `ModelManager` (disk preflight, staged verify, atomic activate).
- Readiness rule enforced: `health` alone never yields `Ready`; active model verification required.

### 4. IPC Commands & ACL Integration
- Exposed commands: `get_hardware_profile`, `get_benchmark_report`, `run_hardware_benchmark`, `set_backend_override`.
- Updated `get_system_profile` to source from empirical detection and benchmark reports.
- Opted into Tauri ACL deny-by-default via `build.rs`, with `allow-hardware-profile` and `allow-hardware-benchmark` scoped strictly to the `main` window capability.
- Advanced UI card updated in `index.html` and `src/main.js` with benchmark status display and manual "Run benchmark" control.
