# Plan 16 — Tray icon and visual polish

**Priority:** P2  
**Status:** NOT STARTED

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
