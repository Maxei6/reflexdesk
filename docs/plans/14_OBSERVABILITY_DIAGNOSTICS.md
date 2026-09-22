# Plan 14 — Privacy-safe observability

**Priority:** P1  
**Status:** NOT STARTED

## Objective

Make failures debuggable without turning ReflexDesk into telemetry spyware.

## Local logs

Structured rotating logs:
- runtime lifecycle
- STT health/performance
- action/tool results
- process ownership
- updater/model manager

Default redaction:
- no audio
- no secrets
- no clipboard/file contents
- no full browser text
- no transcripts unless explicitly enabled for debugging

## Diagnostics bundle

User-triggered export contains:
- app version/build
- OS/hardware class
- runtime/model versions
- benchmark numbers
- recent sanitized errors/state transitions
- installed adapter availability

Preview contents before export.

## Optional telemetry

Off by default. If ever added:
- explicit opt-in
- aggregate operational metrics only
- separate consent for crash reports
- no conversation/transcript collection

## Definition of done

A support issue can be diagnosed from an intentionally exported sanitized bundle
without asking the user to expose private command history.
