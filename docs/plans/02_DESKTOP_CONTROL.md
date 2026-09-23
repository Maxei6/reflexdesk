# Plan 02 — Semantic desktop control

**Priority:** P1  
**Status:** PARTIAL — WINDOWS WIN32 FALLBACK ONLY
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

## Implementation Notes

- Canonical types implemented in `src-tauri/src/desktop.rs`: `ElementBounds`, `DesktopElement`, `WindowInfo`, `DesktopSnapshot`, `ElementSelector`, `DisambiguationCandidate`, `DesktopHealth`.
- Platform backends:
  - Windows: native Win32 window/control enumeration and message-based operations (`WindowsBackend`). UIA patterns are not linked, so custom-rendered and modern controls may be inaccessible.
  - macOS: returns typed `desktop-backend-unavailable` errors. The AX adapter and permission flow are not linked.
  - Linux: detects AT-SPI availability for recovery guidance but returns typed `desktop-backend-unavailable` errors. The semantic AT-SPI adapter is not linked.
- Semantic selector engine: Role (30 pts) > Accessible name (50 pts exact, 45 pts case-insensitive, 25 pts substring) > Value/text (20 pts) > Context (20 pts) > Process/Window (15 pts).
- Stable element references: Monotonic `SNAPSHOT_GENERATION` counter + snapshot caching in `DESKTOP_CACHE` with stale-reference recovery matching by role/name/context on UI mutation.
- Post-action verification: `verify_contract` with polling loop up to `timeout_ms` for `window-focused`, `window-closed`, `desktop-element-state`, and `vision-fallback`.
- Failure modes handled:
  - `ambiguous-element`: returns ranked candidates when multiple elements tie within 5.0 score points.
  - `stale-reference`: attempts recovery via historical metadata, returns typed error if recovery fails.
  - `modal-blocking`: detects blocking modal dialogs.
  - `privilege-boundary`: detects elevation/access-denied boundaries.
  - `app-closed`: detects target window closure before or during action.
  - `slow-mutation`: polling verification loop with timeout error if state is not achieved.
- Tool integration:
  - 11 `desktop.*` tools registered in `policy::tool_registry()` and validated in `policy::validate_args()`.
  - Tool execution dispatched via `tools::execute()` through `execute_verified()`.
  - Post-action verification hook in `policy::verify_stub()`.
  - `get_desktop_health` command with `allow-desktop-health` permission in `permissions/reflexdesk.toml` and `capabilities/main.json`.

- Acceptance remains open: the representative cross-platform tasks in the
  Definition of done have not been exercised, and two platform adapters plus
  Windows UIA remain unimplemented.
