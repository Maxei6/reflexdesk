<p align="center">
  <img src="assets/reflexdesk-hero.webp" alt="ReflexDesk" width="100%" />
</p>

# ⚡ ReflexDesk

### Your computer, with reflexes.

**Talk. It reacts.** ReflexDesk is an open-source voice layer for your computer, browser, and AI agents — designed to run locally and work fully offline.

> 🎙️ “Open Spotify.”  
> 🌐 “Search for the latest local models.”  
> 🤖 “Run Codex on this repo.”  
> ✋ “Stop.”

**Simple commands take the reflex path. Hard work escalates only when it needs to.**

- 📴 **Offline-first** — local speech, local routing, local planning
- ⚡ **Fast path** — no giant LLM for “open Spotify”
- ✨ **Reactive particle overlay** — visible whenever ReflexDesk is listening
- ⌨️ **Instant global toggle** — `Command/Ctrl + Shift + Space`
- 🧠 **Configurable AI** — Moonshine, Laya, local planners, future models
- 🤖 **Harness-aware** — OpenCode, Kilo, Codex, Claude Code, Gemini, ACP
- 💻 **Windows · macOS · Linux**

## 🚀 Run the current MVP

```bash
npm install
npm run dev
```

The first Moonshine model load may download its local model pack. After it is cached, the STT path can run offline.

You can also type commands into the test bar before STT is ready.

## 🫧 The Reflex

When listening is active, ReflexDesk shows a transparent **white-particle orb** above your desktop. It moves with microphone energy and changes state while tools or agent harnesses are working.

Press **Command/Ctrl + Shift + Space** at any time to activate or kill listening immediately.

## 🏎️ Execution path

```text
Voice → local STT → deterministic reflex → tool
                    ↓ unsure
                   Laya
                    ↓ novel
             local planner / harness
```

The generative model is a fallback — not the main loop.

## 🧪 Current v0.1

The repository now contains a runnable Tauri MVP with:

- Moonshine WASM streaming STT
- animated transparent particle overlay
- global voice toggle
- deterministic fast command router
- optional local Laya sidecar
- local OpenAI-compatible planner bridge
- automatic local model discovery through `/v1/models`
- safe native app/browser tools
- AI harness detection + conservative launch adapter
- configurable workspace/model/provider settings
- CI checks and tagged Windows/macOS/Linux installer builds

See [`docs/MVP.md`](docs/MVP.md) for the exact implemented boundary.

## 🔒 Offline mode

Offline means **no AI cloud calls**. Localhost model servers are allowed; remote model endpoints are blocked unless hybrid mode is explicitly enabled.

For authenticated OpenAI-compatible endpoints, set `REFLEXDESK_PLANNER_API_KEY` in the app environment instead of storing secrets in browser storage.

## 📦 Installers

Tagged releases (`v*`) trigger cross-platform Tauri builds for Windows, macOS, and Linux. Preview builds are intentionally unsigned until signing/notarization is configured.

## 📜 License

MIT. Third-party model licenses stay with their upstream projects. See `THIRD_PARTY_NOTICES.md` and `models/registry.json`.
