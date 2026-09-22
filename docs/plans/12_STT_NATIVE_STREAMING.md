# Plan 12 — Native / authenticated streaming STT

**Priority:** P2  
**Status:** NOT STARTED

## Objective

Reduce speech-end latency and support stable partial transcripts without
exposing an insecure realtime listener.

## Current constraint

P0 intentionally uses AudioWorklet + short utterance boundary + authenticated
loopback HTTP because the upstream CrispASR realtime listener is not sufficiently
isolated/authenticated for ReflexDesk's threat model.

## Preferred target

```text
AudioWorklet
  -> Rust audio bridge
  -> direct/native CrispASR realtime session
  -> partial transcript
  -> speculative pre-routing
  -> final transcript
  -> execute
```

No public/local network listener is preferred.

## Work packages

1. Evaluate CrispASR C API/FFI stability.
2. Wrap native session in Rust.
3. stream PCM incrementally.
4. expose partial/final events to UI.
5. speculative route preparation; execution only on final.
6. cancellation/reset on stop/device switch.
7. compare latency/WER against current HTTP path.
8. retain HTTP path as fallback during migration.

## Definition of done

Streaming path improves measured speech-to-action latency without opening an
unauthenticated listener, and final transcript correctness is non-regressive.
