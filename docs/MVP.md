# ReflexDesk current implementation

## P0 implemented

- first-run onboarding with language selection
- microphone permission proof
- local engine readiness gate
- end-to-end “Hello ReflexDesk” setup test
- measured local STT latency persisted in settings
- hidden/background normal startup
- native system-tray lifecycle
- single-instance protection
- start-at-login support
- durable versioned settings
- readiness/error/recovery state machine
- watchdog restart for the local speech engine
- owned AI-harness process cleanup on explicit quit
- compact active-monitor bottom-center overlay
- semantic colored particle states
- AudioWorklet microphone capture
- microphone disconnect handling
- dynamic loopback STT port + per-session authentication
- NVIDIA Nemotron 3.5 ASR Streaming 0.6B local STT
- normal/legacy x86-64 runtime selection
- Moonshine fallback
- deterministic fast router
- safe app/browser tools
- conservative AI-harness launch bridge
- Windows/macOS/Linux installer pipelines

See [P0.md](P0.md) for lifecycle and security decisions.

## Deliberate boundaries

- first Nemotron use needs network access to fetch model weights
- Windows/Linux CUDA/Vulkan runtime auto-selection is not yet enabled
- native accessibility-tree control is not yet implemented
- browser DOM/CDP control is not yet implemented
- structured ACP adapters and harness interrupt/resume are not yet implemented
- stable installers are not signed/notarized until external credentials exist
- automatic updater activation waits for updater signing keys
- the current CrispASR realtime WebSocket is not enabled because its listener is not loopback/auth hardened; P0 uses authenticated local HTTP after a short speech-final boundary
