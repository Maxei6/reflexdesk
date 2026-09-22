# Plan 07 — Tauri and IPC security hardening

**Priority:** P1  
**Status:** CODE COMPLETE (ACCEPTANCE PENDING)

## Objective

Reduce the blast radius of a compromised WebView or malicious local content.

## Work packages

1. Replace `csp: null` with the narrowest working CSP.
2. Split main and overlay capabilities.
3. Overlay receives only state/audio/window permissions it truly needs.
4. Main dashboard receives settings/diagnostics permissions only.
5. Create custom command permissions for sensitive Rust commands.
6. Validate all IPC arguments in Rust.
7. Deny arbitrary filesystem/process/network access by default.
8. Threat-model localhost services, extension bridge and updater.
9. Dependency audit + supply-chain policy.
10. Security regression tests in CI.

## Threat scenarios

- XSS in main WebView;
- compromised browser page attacking localhost;
- malicious planner output;
- malicious extension message;
- path traversal;
- shell argument injection;
- downgrade/update tampering;
- untrusted model/runtime artifact.

## Definition of done

A documented threat model exists, CSP is enabled, capabilities are
window-specific, sensitive commands have explicit permission scopes, and a
frontend compromise cannot directly obtain unrestricted OS control.
