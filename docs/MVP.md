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
- semantic desktop control is currently limited to the Windows Win32 fallback; UIA,
  macOS AX, and Linux AT-SPI adapters are not yet linked
- authenticated Chrome DOM control is implemented; representative-site acceptance,
  Firefox compatibility, and CDP integration remain open
- structured ACP adapters and harness interrupt/resume are not yet implemented
- stable installers are not signed/notarized until external credentials exist
- updater manifests and artifacts are verified and staged, but installer activation
  and rollback remain unavailable until the signed platform updater is integrated
- the current CrispASR realtime WebSocket is not enabled because its listener is not loopback/auth hardened; P0 uses authenticated local HTTP after a short speech-final boundary
