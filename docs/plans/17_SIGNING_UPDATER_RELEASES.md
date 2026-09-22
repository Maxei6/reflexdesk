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
