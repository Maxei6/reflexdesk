# 🧠⚡ ReflexDesk

> **Your computer, with reflexes.**

**Instant voice control for your computer, browser, and AI agents — local, private, and fully offline when you want it.**

ReflexDesk turns natural speech into actions without sending every command through a slow cloud agent. Simple commands take the fast path. Complex work can escalate to a local LLM or to the AI harness you already use.

---

## 🎙️ Say it. Your computer does it.

```text
"Open Spotify."
"Close the Reddit tab."
"Find the PDF I downloaded yesterday."
"Move this file into Documents."
"Open this repo in OpenCode and fix the failing tests."
"Stop the agent. Show me the diff first."
```

**Most AI computer-use agents think before every click. ReflexDesk reacts first — and only thinks when it has to.**

---

## ✨ Why ReflexDesk?

- ⚡ **Fast reflex path** — common commands skip the big LLM.
- 📴 **Fully offline mode** — speech, routing, planning, memory, and tools can all stay on-device.
- 🔒 **Private by design** — local mode does not need to send audio, screenshots, files, browser state, or prompts to the cloud.
- 🧠 **Local AI** — lightweight System-1 routing with optional compact local planning.
- 🎛️ **Model-agnostic** — STT, reflex, planner, and vision backends are configurable.
- 💻 **Cross-platform target** — Windows, macOS, and Linux.
- 🌐 **Browser control** — semantic DOM/accessibility/CDP first; vision only when necessary.
- 🤖 **AI harness control** — launch and steer OpenCode, Kilo, Codex, Claude Code, ACP-compatible agents, and more.
- 🧩 **Skills** — repeated workflows can become deterministic reusable actions.
- 🛡️ **Deterministic permissions** — models request actions; policy decides what is actually allowed.
- 📊 **Hardware-aware setup** — benchmark the machine and automatically choose the fastest model stack that meets a correctness threshold.
- ☁️ **Optional online mode** — cloud models are opt-in, never silently enabled.

---

## 🫧 The Reflex overlay

When ReflexDesk is active, a **semi-transparent futuristic voice pulse** floats above the desktop.

It reacts to your voice in real time:

- 🌊 waveform amplitude follows microphone energy
- 🧠 pulse changes state when ReflexDesk is thinking
- ⚡ sharp flash when an action fires
- 🤖 different animation when a harness is working
- ✅ quiet confirmation pulse on success
- ⚠️ warning state when confirmation is required

The overlay should be lightweight, click-through when idle, always-on-top, and visually obvious enough that you always know when ReflexDesk is listening.

### ⌨️ Instant kill / activation key

Default global shortcut:

- **Windows/Linux:** `Ctrl + Shift + Space`
- **macOS:** `⌘ + Shift + Space`

Tap once → activate listening.  
Tap again → stop listening **immediately**.

The shortcut is configurable. A push-to-talk mode will also be supported.

No hidden listening state: if the overlay is gone, ReflexDesk is not actively listening.

---

## 🏎️ How it works

```text
                         🎙️ Voice
                            │
                       local STT
                            │
                            ▼
                    ┌──────────────┐
                    │ Reflex Router │
                    │    Laya       │
                    └──────┬───────┘
                           │
                 known / simple action?
                    ┌──────┴──────┐
                   yes            no
                    │              │
                    ▼              ▼
              ⚡ local skill   🧠 local planner
                    │              │
                    └──────┬───────┘
                           ▼
                     🛡️ Policy Engine
                           │
            ┌──────────────┼───────────────┐
            ▼              ▼               ▼
         Browser        Desktop          Files
       DOM / CDP      Accessibility       OS APIs
            │              │               │
            └──────────────┼───────────────┘
                           ▼
                       ✅ Verify
                           │
                           ▼
                 🤖 Agent harness bridge
             OpenCode · Kilo · Codex · ACP
```

A command like **“mute”** should not wake a multi-billion-parameter model.

A task like **“open this repository, find why CI is failing, fix it, rerun the tests, then show me the diff”** can escalate to a local planner or a full coding harness.

---

## 📴 Offline means offline

ReflexDesk is designed so the intelligence stack can work with the network physically disconnected.

```text
Microphone      → local
Speech-to-text  → local
Intent routing  → local
Planner         → local
Desktop tools   → local
Browser control → local
Memory          → local
History         → local
```

If the task itself needs the internet — for example opening a website — the browser can use it.

But **offline mode must make zero AI cloud calls**.

This will become a release invariant, not just a marketing claim.

---

## 🔌 AI harnesses

ReflexDesk should not reinvent every coding agent.

Instead, it becomes the **voice control layer for the agents you already use**.

```text
"Open this folder in OpenCode."
"Ask Kilo to implement the issue."
"Run Codex on this repo."
"Stop."
"Use the stronger model."
"Run the tests again."
"Show me what changed."
```

Target adapters:

- 🟢 OpenCode
- 🟢 Kilo
- 🟢 Generic ACP
- 🟡 Codex
- 🟡 Claude Code
- 🟡 Gemini / compatible CLI harnesses
- 🟡 Generic local CLI adapter

Structured SDKs/protocols such as **ACP** are preferred over screen-scraping another agent UI.

---

## 🧠 Configurable local models

ReflexDesk does not hard-code one model stack.

The installer detects hardware, benchmarks compatible candidates, and recommends the fastest configuration that meets a minimum correctness target.

| Layer | Purpose | Example candidates |
|---|---|---|
| 🎧 STT | Streaming speech recognition | Moonshine, multilingual ASR backends |
| ⚡ Reflex | Fast typed decisions | Laya |
| 🧠 Planner | Multi-step local reasoning/tool use | compact agent models such as Spark-class models |
| 👁️ Vision | fallback when structured UI data is unavailable | configurable |
| ☁️ Online | optional hard-task escalation | user-selected provider |

Better models can be added through the registry without rewriting the app.

---

## 🧪 Hardware-aware setup

On first launch ReflexDesk inspects:

- CPU architecture and instruction sets
- available RAM
- GPU / VRAM
- Apple unified memory
- accelerator/NPU availability when supported
- OS/runtime support

Then it benchmarks a small compatible candidate set.

Model selection prioritizes:

1. ✅ tool-call correctness
2. ✅ argument correctness
3. ✅ task completion
4. ⚡ time-to-first-action
5. ⚡ total latency
6. 🧠 RAM / VRAM use
7. 🔋 CPU efficiency

**Fast but unreliable does not win.**

---

## 🛠️ Tools, not magic clicks

ReflexDesk exposes small typed tools:

```text
app.open        app.close       app.focus
browser.open    browser.search  browser.click
browser.type    browser.scroll  browser.tabs
window.list     window.focus
file.find       file.open       file.move       file.rename
clipboard.read  clipboard.write
media.play_pause              media.volume
harness.start   harness.send    harness.stop
```

Models request tool calls. ReflexDesk validates them, checks permissions, executes them with native APIs, then verifies the result.

---

## 🛡️ Safety by architecture

AI does **not** get unrestricted authority over the machine.

```yaml
file.delete:
  risk: destructive
  confirmation: always

media.pause:
  risk: safe
  confirmation: never
```

The model can propose an action.

**The deterministic policy engine decides whether it is allowed.**

---

## 🧩 Skills

Repeated successful workflows can become fast reusable skills.

```text
first time:
voice → planner → tools → verified success

later:
voice → reflex → compiled skill → instant execution
```

Example: **“Start work.”** can launch your browser, mail, Slack, GitHub, IDE, and preferred AI harness without asking an LLM to rediscover the same steps every morning.

---

## 🎯 Performance goals

These are targets, not benchmark claims yet:

- ⚡ **sub-300 ms P50** speech-end → visible action for simple commands on capable hardware
- 🧠 **70–90%** of repeated everyday commands resolved without a generative LLM
- 📴 **zero AI network calls** in offline mode
- 💻 usable on ordinary CPU-based laptops
- 🔁 deterministic fallback and verification when confidence is low

Real benchmarks will be published once reproducible builds are available.

---

## 🗺️ Roadmap

### 1 — ⚡ Reflex core
- [ ] streaming microphone input
- [ ] configurable STT provider
- [ ] Laya decision adapter
- [ ] typed tool registry
- [ ] deterministic policy engine
- [ ] global activation/kill shortcut
- [ ] futuristic reactive overlay

### 2 — 💻 Desktop + browser
- [ ] Windows UI Automation
- [ ] macOS Accessibility
- [ ] Linux AT-SPI
- [ ] browser extension / CDP bridge
- [ ] semantic verification

### 3 — 🧠 Local planner
- [ ] model registry
- [ ] hardware detection
- [ ] local runtime abstraction
- [ ] benchmark-based model selector
- [ ] offline multi-step planning

### 4 — 🤖 Harness bridge
- [ ] OpenCode adapter
- [ ] Kilo adapter
- [ ] generic ACP adapter
- [ ] Codex adapter
- [ ] Claude Code adapter
- [ ] voice steering + interrupt/resume

### 5 — 📦 Packaging
- [ ] Windows installer
- [ ] macOS app / DMG
- [ ] Linux AppImage / deb
- [ ] signed releases
- [ ] automatic model-pack setup
- [ ] reproducible offline-mode tests

---

## 🤝 Contributing

ReflexDesk is early.

The goal is not to build another giant AI framework. The goal is to make computer control **fast, local, understandable, and boringly reliable**.

Contributions around desktop accessibility APIs, speech recognition, local inference, browser automation, agent harness integrations, model benchmarking, packaging, and UX are welcome.

---

## 📜 License

ReflexDesk is released under the **MIT License**.

Third-party models and dependencies keep their own licenses. See `THIRD_PARTY_NOTICES.md` and `models/registry.json` before redistribution.

---

<p align="center">
  <strong>🧠 ReflexDesk — Your computer, with reflexes. ⚡</strong>
</p>
