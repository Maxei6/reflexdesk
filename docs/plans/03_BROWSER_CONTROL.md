# Plan 03 — Browser DOM/CDP control

**Priority:** P1  
**Status:** NOT STARTED  
**Depends on:** policy engine

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
