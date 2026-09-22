# Plan 16 — Tray icon and visual polish

**Priority:** P2  
**Status:** ACCEPTANCE PENDING

## Objective

Make ReflexDesk legible at true system-utility sizes and keep visual state
consistent across tray, overlay and dashboard.

## Tray assets

Create dedicated pixel-tested assets for:
- idle/ready
- listening
- working
- attention/error

Do not scale the complex 1024px particle artwork down blindly.

macOS:
- monochrome template assets

Windows/Linux:
- small state-specific icon variants appropriate to platform behavior

## Overlay polish

- test 100/125/150/200% scaling;
- multiple monitors and dock/taskbar orientations;
- reduced-motion mode;
- high-contrast/accessibility;
- no focus stealing;
- no click interception;
- animation budget target and battery impact.

## Definition of done

Tray states are recognizable at 16–24px and the overlay remains correctly
positioned/readable across DPI, taskbar/dock and monitor configurations.

## Acceptance & Implementation Notes

- **Tray Assets (`assets/tray/`)**:
  - 40 pixel-tested assets generated and committed: `idle`, `ready`, `listening`, `working`, and `attention` across 16px, 20px, and 24px sizes.
  - Dedicated macOS monochrome template assets (`*-template-{16,20,24}.png`) and platform-independent aliases (`{state}.png` and `{state}-template.png`).
  - Pixel designs use geometric clarity (hollow ring for idle, filled vibrant core for ready, 3-bar audio wave for listening, orbiting cluster for working, negative-space exclamation badge for attention).
- **Tray State Mapping (`src-tauri/src/tray.rs`)**:
  - Defined `TrayState` enum (`Idle`, `Ready`, `Listening`, `Working`, `Attention`).
  - `phase_to_tray_state` maps all 13 `Phase` variants and active listening state.
  - Compile-time embedded icons via `include_bytes!` with platform-specific template flag handling on macOS.
  - Dynamic `update()` refreshes the icon and tooltip on phase/listening transitions.
  - Embedded Rust unit tests verify mapping completeness and PNG decodability.
- **Overlay Geometry (`src/lib/geometry.js`)**:
  - `calculateOverlayPosition(monitor, windowSize, options)` handles 100%, 125%, 150%, and 200% DPI scaling factors.
  - Handles multi-monitor negative coordinates (e.g. secondary displays positioned to the left or above primary display).
  - Handles work area insets (taskbar/dock at bottom, top, left, or right) and clamps overlay coordinates to prevent clipping.
  - Provides graceful fallback to centered desktop coordinates upon monitor disconnect/removal.
- **Animation Budget & Battery Impact (`src/lib/particles.js` & `src/overlay.js`)**:
  - `ParticleOrb` monitors `prefers-reduced-motion: reduce`. When active, continuous `requestAnimationFrame` is completely cancelled, and state transitions draw a single static frame.
  - Dynamic change listener restarts RAF or returns to static rendering when system reduced-motion settings toggle.
  - `src/overlay.js` calls `orb.stop()` when listening is inactive and `orb.start()` only when active, eliminating background GPU/CPU burn when the overlay is hidden.
- **Accessibility & Focus/Click Invariants (`src/overlay.css` & `src/overlay.js`)**:
  - `pointer-events: none !important` applied across overlay root, orb canvas, and caption container to guarantee zero click interception.
  - `setFocusable(false)` added to window setup to prevent focus stealing.
  - High-contrast rules added for `@media (forced-colors: active)` and `@media (prefers-contrast: more)` with high-contrast text and border treatments.
- **Always-Visible Active-Listening & Kill Switch**:
  - Semi-transparent, always-on-top overlay activates and displays state whenever listening is active.
  - Global hotkey (`Ctrl+Shift+Space` / `Cmd+Shift+Space`) preserves deterministic kill switch via `policy::cancel_now("hotkey-cancelled")` and `set_listening_internal(app, false)`.
- **Test Coverage**:
  - Node suite in `tests/tray-mapping.test.mjs` verifies DPI scaling (100%, 125%, 150%, 200%), multi-monitor negative coordinates, dock insets, clamping, fallback, and asset header validation.
