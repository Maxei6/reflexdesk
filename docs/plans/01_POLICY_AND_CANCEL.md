# Plan 01 — Policy engine and deterministic cancel

**Priority:** P0.5  
**Status:** CODE COMPLETE (ACCEPTANCE PENDING)
**Blocks:** desktop control, browser control, powerful tools

## Objective

Create a mandatory deterministic authorization layer between all planners and
all side-effecting tools. Add a universal emergency cancel path that bypasses
STT/planners.

## Architecture

Define an ActionEnvelope containing:
- tool name
- typed arguments
- source: reflex / planner / harness / user
- session id
- risk class
- requested capability
- verification contract

Risk classes:
- SAFE
- SENSITIVE
- DESTRUCTIVE
- EXTERNAL_COMMIT

Policy outcomes:
- ALLOW
- DENY
- CONFIRM
- REQUIRE_UNLOCK

## Work packages

1. Central Rust tool registry; remove ad-hoc execution paths.
2. Per-tool schema validation.
3. Risk/confirmation metadata.
4. Session-scoped permissions and user preferences.
5. Deterministic global cancel token checked before/after every long operation.
6. Hard hotkey cancel that stops capture, aborts current action and interrupts
   owned agents.
7. Explicit negation/correction handling before tool routing.
8. Confirmation UI that cannot be spoofed by tool output.

## Failure cases

- “don't close Chrome” must not become close Chrome.
- “actually stop” must preempt queued work.
- planner invents a tool.
- malformed arguments.
- delayed tool completes after cancellation.
- two actions race for destructive state.
- confirmation window loses focus.

## Tests

Create adversarial command corpus covering negation, ambiguity, corrections,
destructive actions and prompt-injection attempts.

## Definition of done

- no side-effecting tool executes outside the policy engine;
- cancel has bounded latency and does not require STT;
- all destructive actions require explicit deterministic confirmation;
- unit/integration tests prove denied actions cannot bypass policy.
