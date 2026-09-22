# Plan 10 — Model and runtime manager

**Priority:** P1  
**Status:** NOT STARTED

## Objective

Make model acquisition boring, resumable and repairable instead of relying on
opaque first-run downloads.

## Manifest

Each artifact records:
- logical id
- upstream repo
- exact revision
- license
- expected size
- checksum where authoritative
- runtime compatibility
- minimum hardware
- cache path
- migration/replacement rules

## Work packages

1. ModelManager Rust service.
2. disk-space preflight.
3. resumable downloads.
4. atomic staging -> verify -> activate.
5. progress events for onboarding.
6. repair corrupted/missing assets.
7. rollback previous compatible version.
8. cache garbage collection and user-visible disk usage.
9. offline-install/import option.
10. license/notice presentation.

## Definition of done

Network interruption, disk full, corrupted file and upgrade interruption never
leave ReflexDesk in a false Ready state; retry resumes safely without redownloading
everything.
