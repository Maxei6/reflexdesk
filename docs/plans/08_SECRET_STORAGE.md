# Plan 08 — Secret storage and account connections

**Priority:** P1  
**Status:** NOT STARTED  
**Depends on:** security hardening

## Objective

Support optional online providers without placing credentials in settings.json,
localStorage, logs or command history.

## Design

Prefer OS-backed secure storage / Tauri Stronghold abstraction.

Store only opaque secret references in normal settings.

## Work packages

1. SecretStore interface: set/get/delete/list metadata.
2. Stronghold/OS secure backend.
3. Migration guard: detect and reject plaintext secrets.
4. Provider connection UI: Connect / Test / Disconnect.
5. Redaction helper used by all logs and diagnostics.
6. API-key rotation and invalid-key recovery.
7. Export diagnostics excludes secrets by construction.
8. Zeroize sensitive buffers where practical.

## Definition of done

Search of app settings/logs/localStorage contains no provider credentials, and
all provider integrations retrieve secrets only through SecretStore.
