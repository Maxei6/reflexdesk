# Plan 03 — Browser DOM/CDP control

**Priority:** P1  
**Status:** CODE COMPLETE (LIVE ACCEPTANCE PENDING)
**Depends on:** policy engine  
**Driver:** Plan03Browser agent  
**Last Updated:** 2026-09-23  

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
- **Chrome MV3 Extension**: `extension/chrome/` contains a valid Chrome/Chromium MV3 package with MutationObserver SPA tracking, semantic accessibility DOM snapshots, sensitive-field masking, nonce replay rejection, and the authenticated local WebSocket bridge.
- **Firefox MV3 Extension**: `extension/firefox/` contains a separate Firefox package using the same versioned authenticated protocol instead of mixing Chrome and Gecko manifest fields in one package.
- **Authenticated Bridge**: `src-tauri/src/browser.rs` binds only to `127.0.0.1`, requires a per-install pairing secret stored in the OS credential vault, correlates protocol version/session/nonce/pairing on every response, bounds message size and timeouts, and disconnects on protocol violations.
- **Runtime Engine**: `tabs`, `open`, `inspect`, `find`, `click`, `type_text`, `select`, `scroll`, `extract`, `wait`, `download`, and `verify_action` now execute through the authenticated extension bridge. Bridge health reflects a real connected extension instead of simulated state.
- **Policy & Gate**: All browser tools are declared in `src-tauri/src/policy.rs`, validated before dispatch, and sensitive submission remains confirmation-gated.
- **Commands & ACL**: `get_browser_status` and `get_browser_pairing_secret` remain least-privilege Tauri commands for the main window.

- Acceptance remains open until live Chrome tests complete navigation, search,
  form filling, tab switching, downloads, blocked/captcha/auth handling, and
  post-action verification across representative sites in Chrome and Firefox.
  Direct CDP remains an optional optimization for surfaces where the extension
  cannot provide equivalent state; it is not required for the current semantic tool surface.
