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

## Implementation notes

Status: PARTIAL — cryptographic verification and staging implemented; release
credentials, installer activation, rollback, and clean-machine drills remain open.

Implemented without release credentials:
- `src-tauri/src/updater.rs` verifies an Ed25519 signature over canonical
  version/platform/URL/SHA-256 metadata before anti-downgrade and rollout checks.
- Update feeds and artifacts require HTTPS. Downloads are bounded, streamed to a
  temporary file, SHA-256 checked against the signed manifest, fsynced, and
  atomically renamed to verified staging.
- Stable client identity drives deterministic rollout cohorts.
- Update staging and model acquisition/migration are mutually exclusive through
  RAII transaction guards.
- `system.update_check` performs a real channel feed check.
  `system.update_apply` currently stages a verified artifact but reports
  `ok: false` with `installer activation unavailable`; it never claims an update
  was installed.
- The previous rollback command was removed because it only validated a backup
  path and falsely reported success without restoring the application.
- CI uses the committed `Cargo.lock`; preview release preflight remains fail-closed.
- Rust cryptographic tests cover signed downgrade rejection and manifest
  tampering. The Node updater/preflight suite remains green.

Still blocked:
- Windows Authenticode certificate/service.
- Apple Developer ID, notarization, and stapling credentials.
- Production updater signing key/public key and stable feed.
- A real signed platform installer activation path with rollback/recovery.
- SmartScreen/Gatekeeper clean-machine verification and a staged rollback drill
  before stable v1.
