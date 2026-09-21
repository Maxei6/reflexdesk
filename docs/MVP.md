# v0.1 MVP

ReflexDesk v0.1 is intentionally narrow and testable.

## Works in this milestone

- Tauri desktop shell
- transparent always-on-top particle overlay
- microphone-energy reactive animation
- global toggle: Command/Ctrl + Shift + Space
- Moonshine WASM streaming STT adapter
- deterministic fast router for common safe commands
- native app open / browser open / browser search tools
- harness detection and conservative launch bridge
- optional local Laya sidecar adapter
- configurable local OpenAI-compatible planner endpoint
- automatic /v1/models discovery when planner model is set to auto
- Windows/macOS/Linux Tauri bundle targets

## Next before calling it production ready

- verified structured ACP adapters for OpenCode/Kilo
- richer multi-step planner loop with verification/retries
- installer hardware benchmark and model downloader
- automatic Laya runtime packaging / ONNX path
- native accessibility trees for Windows/macOS/Linux
- browser extension + CDP bridge
- signed/notarized releases
- offline egress test harness
