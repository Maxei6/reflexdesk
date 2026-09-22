# Plan 02 — Semantic desktop control

**Priority:** P1  
**Status:** NOT STARTED  
**Depends on:** policy engine

## Objective

Control arbitrary desktop applications through native accessibility APIs rather
than fixed app aliases or screen coordinates.

## Platform backends

- Windows: UI Automation (UIA)
- macOS: Accessibility / AX APIs
- Linux: AT-SPI

Expose one normalized model:
```text
DesktopSnapshot
  active_app
  windows[]
  focused_window
  elements[]
    id, role, name, value, bounds, enabled, focused, actions[]
```

## Tool surface

- desktop.inspect
- desktop.find
- desktop.focus_window
- desktop.invoke
- desktop.click
- desktop.type
- desktop.press_key
- desktop.scroll
- desktop.read
- desktop.verify
- desktop.close_window

## Work packages

1. Define canonical Rust domain types.
2. Implement Windows backend first.
3. Add stable element references with stale-reference recovery.
4. Add macOS permission/onboarding flow.
5. Add Linux AT-SPI backend and graceful unsupported-state errors.
6. Build semantic selector engine ranked by role/name/context.
7. Post-action re-inspection and verification.
8. Vision fallback interface only for elements inaccessible semantically.

## Failure cases

- duplicated labels/buttons;
- inaccessible custom canvas apps;
- stale accessibility nodes after navigation;
- modal dialogs;
- privilege boundary/admin windows;
- multiple monitors/DPI;
- app closes between inspect and action;
- slow UI mutation.

## Definition of done

Representative tasks such as “open Settings, find Bluetooth and toggle it” work
on all supported OSes without hard-coded coordinates, and every successful tool
call verifies the resulting state.
