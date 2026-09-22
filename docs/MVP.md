# v0.1 MVP

ReflexDesk v0.1 is intentionally narrow and testable.

## Works in this milestone

- Tauri desktop shell
- transparent always-on-top particle overlay
- microphone-energy reactive particle animation
- global toggle: Command/Ctrl + Shift + Space
- microphone tracks stop when listening is disabled
- NVIDIA Nemotron 3.5 ASR Streaming 0.6B as the default local STT
- pinned CrispASR native runtime packaged per desktop platform
- utterance-boundary local VAD and persistent localhost STT server
- Moonshine WASM fallback
- deterministic fast router for common safe commands
- native app open / browser open / browser search tools
- harness detection and conservative launch bridge
- optional local Laya sidecar adapter
- configurable local OpenAI-compatible planner endpoint
- automatic /v1/models discovery when planner model is set to auto
- Windows/macOS/Linux Tauri bundle targets

## Truth boundary

- First Nemotron use can download ~458 MB of model weights.
- After model/runtime assets are local, STT makes no AI cloud calls.
- The current integration executes only after a short end-of-speech boundary; it does not act on unstable partial transcription.
- The bundled Windows/Linux x86-64 runtime defaults to CPU for broad compatibility.
- Automatic GPU-runtime selection is still roadmap work.
- Harness prompt injection is deliberately conservative until each structured adapter is verified.

## Next before calling it production ready

- installer hardware benchmark + automatic CPU/GPU runtime selection
- verified structured ACP adapters for OpenCode/Kilo
- richer multi-step planner loop with verification/retries
- automatic Laya runtime packaging / ONNX path
- native accessibility trees for Windows/macOS/Linux
- browser extension + CDP bridge
- signed/notarized releases
- offline egress test harness
