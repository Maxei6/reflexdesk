# Local speech recognition

ReflexDesk defaults to **NVIDIA Nemotron 3.5 ASR Streaming 0.6B** through the native **CrispASR** runtime.

## Why this stack

- NVIDIA's model is a 600M-parameter cache-aware streaming FastConformer/RNN-T ASR.
- It supports multilingual speech, including English and Italian.
- CrispASR runs the model locally without requiring Python or PyTorch.
- The default runtime model is the Q4_K GGUF conversion (~458 MB) to keep the local footprint practical.
- The bundled CrispASR executable is pinned and SHA-256 verified during the build.

## First-run behavior

ReflexDesk does **not** commit model weights to this repository.

On the first Nemotron use, CrispASR downloads the model into its local cache. That first setup therefore needs network access. Once the model is cached, speech recognition is local and works without AI cloud calls.

## Command path

```text
microphone
  ↓
local RMS + utterance VAD
  ↓
completed short utterance
  ↓
localhost CrispASR server
  ↓
NVIDIA Nemotron 3.5
  ↓
transcript
  ↓
ReflexDesk fast router
```

Nemotron itself is streaming-native, but ReflexDesk v0.1 deliberately waits for a short end-of-speech boundary before executing a desktop command. This avoids acting on unstable partial text.

## Privacy behavior

The CrispASR process can stay warm in memory for lower latency.

It does **not** receive microphone audio while ReflexDesk is off. When the global toggle is disabled, ReflexDesk explicitly stops the WebView microphone tracks and closes its AudioContext.

## CPU compatibility

Desktop builds include:

- normal x86-64 CrispASR runtime for AVX2/FMA CPUs
- legacy x86-64 runtime for older CPUs, selected automatically
- native arm64 runtime where available
- macOS runtime with Metal support built upstream

Automatic GPU-runtime download/selection for Windows/Linux is planned separately so the default installer does not ship hundreds of megabytes of CUDA runtime.

## Fallback

Moonshine remains available as an explicit local fallback provider. It is no longer the default STT.

## Runtime pins

- CrispASR: `v0.8.35`
- Nemotron: `nvidia/nemotron-3.5-asr-streaming-0.6b`
- GGUF runtime model: `cstr/nemotron-3.5-asr-streaming-GGUF` Q4_K
