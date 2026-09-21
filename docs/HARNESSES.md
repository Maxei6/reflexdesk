# AI harness bridge

ReflexDesk can control mature AI harnesses instead of recreating their orchestration.

## Adapter contract
A harness adapter should expose:
- detect
- start(cwd)
- send(message)
- interrupt
- resume
- stream_events
- status
- open_ui
- close

## Preferred integration order
1. native embedded SDK
2. ACP
3. structured local server API
4. structured CLI / JSONL events
5. plain CLI process adapter
6. UI automation only as last resort

## Initial targets
- OpenCode
- Kilo
- generic ACP
- Codex
- Claude Code
- Gemini-compatible CLIs
- generic local command harness

Voice steering must support immediate interrupt/stop.
