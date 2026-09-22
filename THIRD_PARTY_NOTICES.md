# Third-party notices

ReflexDesk source code is MIT-licensed.

Third-party libraries, model runtimes, model weights, tokenizers, and bundled assets retain their own licenses.

## CrispASR

ReflexDesk packages a pinned CrispASR runtime during desktop builds.

- Project: `CrispStrobe/CrispASR`
- Runtime pin: `v0.8.35`
- License: MIT
- Build assets are SHA-256 verified before packaging.
- Where present upstream, CrispASR license/notices are copied into the app resources.

## NVIDIA Nemotron 3.5 ASR Streaming 0.6B

Default speech model:

- Upstream: `nvidia/nemotron-3.5-asr-streaming-0.6b`
- License: OpenMDW-1.1
- Runtime GGUF conversion: `cstr/nemotron-3.5-asr-streaming-GGUF`
- Default runtime quantization: Q4_K
- Model weights are downloaded on first use and are not committed to ReflexDesk.

Use and redistribution of the model remain governed by the upstream model license. The ReflexDesk MIT license does not replace or modify those terms.

## Moonshine

Moonshine remains an optional compatibility fallback. Its code and model assets retain the licenses declared by the exact upstream version/checkpoint used.

## Model-pack policy

Before adding any model to an official ReflexDesk model pack, record:

- exact repository/model identifier
- exact revision or checksum
- license
- commercial-use restrictions
- redistribution requirements
- source URL
- runtime/quantization source
- attribution requirements

See `models/registry.json`.
