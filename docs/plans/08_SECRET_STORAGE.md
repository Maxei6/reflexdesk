# Plan 08 — Secret storage and account connections

**Priority:** P1  
**Status:** CODE COMPLETE (ACCEPTANCE PENDING)
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

## Implementation Notes

### 1. Backend Choice: OS-Backed Secure Vault (`OsVaultSecretStore`)
- Implemented in `src-tauri/src/secrets.rs`.
- Design choice: Implemented an OS-level per-user filesystem vault (permissions `0o700` on directories and `0o600` on payload files under Unix, user-profile ACL restricted on Windows) rather than Tauri Stronghold (which imposes password-prompt UX friction) or system C-dependencies like `libsecret` (which fails in headless Linux CI environments).
- Supports atomic key rotation: setting a new key for an existing provider replaces payload bytes, updates `updated_at_ts`, and removes the previous payload file.
- In-memory store (`InMemorySecretStore`) provided for isolated unit testing.

### 2. Opaque Secret References in Settings
- `AppSettings` stores only `planner_secret_ref: Option<SecretRef>`.
- `SecretRef` contains strictly `{ provider: String, id: String }`. Plaintext API keys, passwords, and tokens are never written to `settings.json` or `localStorage`.

### 3. Migration Guard: Plaintext Detection & Quarantine
- `detect_plaintext_secret_key` inspects raw JSON before deserialization for sensitive keys (`api_key`, `token`, `password`, `secret`, `authorization`, etc.) at all nesting levels.
- Exempts opaque references (`_ref`).
- If a plaintext secret is detected, `settings.json` is atomically renamed to `settings.json.quarantine.<timestamp>`, a redacted warning is logged, and loading fails closed with a generic error rather than silent-defaulting.

### 4. Provider Connection UI (Connect / Test / Disconnect)
- Added Provider Connection card in `index.html` under Privacy.
- IPC commands registered with Tauri ACL (`allow-connect-provider`, `allow-test-provider`, `allow-disconnect-provider`, `allow-get-provider-status`, `allow-list-secret-metadata` in `src-tauri/permissions/reflexdesk.toml` and `src-tauri/capabilities/main.json`).
- Immediate DOM zeroization: API key password input is cleared immediately upon submission.

### 5. API-Key Rotation and Invalid-Key Recovery
- Replaced `std::env::var("REFLEXDESK_PLANNER_API_KEY")` in `src-tauri/src/planner.rs` with `SecretStore` lookup.
- Credentials sent only to policy-approved endpoints (`check_endpoint_allowed`).
- HTTP 401 and 403 responses are mapped to sanitized `auth-invalid` recovery status with clear UI prompt to rotate credentials.

### 6. Harness Process Environment Sanitization
- `process_supervisor.rs` automatically strips all provider API keys (`REFLEXDESK_PLANNER_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.) from child process environments spawned as `OwnershipClass::OwnedSession`.

### 7. Memory Zeroization
- `SecretBytes` buffer implements `Drop` using volatile writes and memory fences to ensure sensitive data is wiped from RAM pages when released.

### 8. Verification
- Node test suite `tests/secrets.test.mjs` verifies plaintext detection, nested token rejection, opaque reference preservation, and lack of credentials in settings or localStorage.
- Static validation `scripts/validate.mjs` passes.
