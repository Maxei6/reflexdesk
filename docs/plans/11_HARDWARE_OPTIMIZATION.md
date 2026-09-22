# Plan 11 — Hardware benchmark and backend selection

**Priority:** P1  
**Status:** NOT STARTED  
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
