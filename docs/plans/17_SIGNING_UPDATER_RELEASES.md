# Plan 17 — Signing, notarization and updater

**Priority:** RELEASE BLOCKER  
**Status:** BLOCKED ON EXTERNAL CREDENTIALS

## Objective

Ship trusted stable binaries and cryptographically verified updates.

## Windows

- acquire trusted code-signing certificate/service;
- sign EXE/MSI;
- verify signature in CI;
- document SmartScreen/reputation behavior.

## macOS

- Developer ID Application certificate;
- hardened runtime as required;
- notarize;
- staple ticket;
- verify Gatekeeper on clean machine.

## Updater

Channels:
- nightly
- beta
- stable

Requirements:
- Tauri updater signing key kept only in secrets;
- public key embedded in app;
- signed manifests/artifacts;
- staged rollout;
- rollback/recovery path;
- update cannot run during model migration without transaction coordination.

## CI policy

Preview workflows remain unsigned.
Stable workflow fails if any required credential/signature/notarization check is
missing.

## Definition of done

Fresh Windows/macOS machines install without avoidable trust warnings, updater
rejects tampered artifacts, and a staged rollback is tested before stable v1.

## Implementation notes (one-pass, 2026-09-22)

Status: PARTIAL — scaffolding implemented, credential-blocked items open.
Tests: `tests/updater.test.mjs` 10/10 pass (preflight fail-closed, anti-downgrade,
signature presence, staged rollouts, model-migration lockout).

Implemented without credentials:
- `src-tauri/src/updater.rs`: channels (nightly/beta/stable), `verify_manifest`
  (strict anti-downgrade, fail-closed on missing pubkey, unsigned/tampered
  rejection, sha256 presence, staged cohort eligibility), mutual exclusion with
  model transactions both directions (`is_update_in_progress` /
  `ModelManager::{begin_transaction, is_transaction_in_progress}` + RAII guard).
- Policy-gated tools `system.update_check` (Safe) / `system.update_apply`
  (Destructive + confirm + external); Tauri commands + ACL least-privilege.
- `tauri.conf.json` updater scaffolding with empty pubkey/endpoints (disabled
  until a real key is embedded); `release.yml` preflight job fails closed
  without credentials; `scripts/release-preflight.mjs` + artifact metadata.
- Manifest fixtures: valid, tampered, unsigned, downgrade, staged rollout.

Still BLOCKED ON EXTERNAL CREDENTIALS (not done, no placeholders committed):
Windows Authenticode cert, Apple Developer ID + notarization/stapling,
Tauri updater private signing key, SmartScreen/Gatekeeper clean-machine
verification, staged-rollback drill before stable v1.
