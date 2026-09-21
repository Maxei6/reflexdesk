# ReflexDesk contributor and coding-agent instructions

## Goal
Build the fastest practical local-first voice control layer for desktop computers, browsers, and AI harnesses.

## Non-negotiable principles
1. Offline mode makes zero AI cloud calls.
2. Prefer deterministic/native APIs over visual clicking.
3. Prefer accessibility trees, DOM, and CDP over screenshots.
4. Simple actions must not require a generative LLM.
5. Models request actions; deterministic policy controls execution.
6. Destructive/high-impact tools require explicit policy and confirmation.
7. STT, reflex, planner, vision, and harness backends must be replaceable adapters.
8. Benchmark correctness before speed.
9. Keep the base app useful without a large generative model.
10. Never claim benchmark numbers until reproducibly measured.
11. Listening state must always be visible and globally killable immediately.

## UX invariant
The active-listening indicator is a semi-transparent, always-on-top reactive overlay. Default toggle is Ctrl+Shift+Space on Windows/Linux and Cmd+Shift+Space on macOS. Shortcut is configurable.

## Architecture direction
- Desktop shell: Tauri
- Core: Rust where practical
- STT: configurable adapter
- Reflex: Laya initially
- Planner: configurable compact local tool-use LLM
- Browser: DOM/accessibility/CDP first
- Desktop: native accessibility APIs
- Harnesses: structured SDK/ACP/CLI adapters
- Permissions: deterministic policy engine

Keep modules small, interfaces explicit, and dependencies minimal.
