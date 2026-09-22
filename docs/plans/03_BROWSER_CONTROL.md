# Plan 03 — Browser DOM/CDP control

**Priority:** P1  
**Status:** ACCEPTANCE PENDING  
**Depends on:** policy engine  
**Driver:** Plan03Browser agent  
**Last Updated:** 2026-09-22  

## Objective

Provide reliable semantic browser automation without driving the browser through
desktop pixels.

## Architecture

Preferred order:
1. ReflexDesk browser extension for DOM/accessibility state.
2. Authenticated localhost bridge to desktop app.
3. CDP where available for tabs/navigation/downloads/network-level state.
4. Desktop/vision fallback only for browser chrome or inaccessible surfaces.

## Tool surface

- browser.tabs
- browser.open
- browser.inspect
- browser.find
- browser.click
- browser.type
- browser.select
- browser.scroll
- browser.extract
- browser.wait
- browser.download
- browser.verify

## Security

- random per-install pairing secret;
- origin-scoped extension permissions;
- no arbitrary localhost unauthenticated control;
- redact password/payment fields from planner context by default;
- sensitive form submission goes through policy confirmation.

## Work packages

1. Protocol/schema between extension and Tauri.
2. Chrome/Chromium extension.
3. Firefox compatibility.
4. semantic DOM snapshot compression;
5. robust selector/reference strategy;
6. SPA mutation/wait logic;
7. download and tab lifecycle;
8. verification/retry loop.

## Definition of done

A test suite can complete navigation, search, form filling, tab switching and
downloads across representative sites without image-based clicking, with
post-action verification and clear handling of blocked/captcha/auth states.

## Acceptance Notes & Implementation Summary

- **Protocol**: `extension/protocol.json` specifies versioned envelope (`{v, session, pairing, tabId, ref, action, args, nonce}`), 12 `browser.*` actions, snapshot compression rules (tag skipping, container pruning, 500 element limit, redaction), and standard error codes (`stale-ref`, `spa-mutation`, `blocked`, `captcha`, `auth-required`, `element-not-found`, `action-timeout`, `bridge-unavailable`, `pairing-rejected`).
- **Chrome MV3 / Firefox Extension**: `extension/chrome/manifest.json`, `content.js`, `background.js`, and `popup.html`/`popup.js` implemented with MutationObserver SPA tracking, semantic accessibility DOM builder, sensitive field masking, and local bridge WebSocket client.
- **Runtime Engine**: `src-tauri/src/browser.rs` exposes `tabs`, `open` (using `security::sanitize_open_external`), `inspect`, `find`, `click`, `type_text`, `select`, `scroll`, `extract`, `wait`, `download`, `verify_action`, deterministic `health()`, and status reporting.
- **Policy & Gate**: All browser tools declared in `src-tauri/src/policy.rs` tool registry with schema validation in `validate_args`, sensitive submission detection (`is_sensitive_submission`), and verification wiring through `policy::verify_stub`.
- **Commands & ACL**: `get_browser_status` and `get_browser_pairing_secret` registered in `src-tauri/src/lib.rs`, `src-tauri/build.rs`, `permissions/reflexdesk.toml`, and `capabilities/main.json`.
