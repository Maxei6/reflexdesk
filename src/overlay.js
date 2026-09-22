import { invoke } from "@tauri-apps/api/core";
import { PhysicalPosition } from "@tauri-apps/api/dpi";
import { emit, listen } from "@tauri-apps/api/event";
import {
  cursorPosition,
  getCurrentWindow,
  monitorFromPoint,
  primaryMonitor,
} from "@tauri-apps/api/window";
import { MicTranscriber, ModelArch } from "@moonshine-ai/moonshine-wasm";
import { ParticleOrb } from "./lib/particles.js";

const root = document.getElementById("overlayRoot");
const partial = document.getElementById("partialText");
const stateText = document.getElementById("stateText");
const orb = new ParticleOrb(document.getElementById("orb"), { compact: true });
orb.start();

const SETTINGS_KEY = "reflexdesk.settings.v1";
const TARGET_RATE = 16000;
const SPEECH_THRESHOLD = 0.017;
const END_SILENCE_MS = 340;
const MAX_COMMAND_SECONDS = 12;
const PRE_ROLL_SAMPLES = Math.round(TARGET_RATE * 0.18);

let audio = null;
let moonshine = null;
let sttStarting = false;
let active = false;
let nemotronReady = false;
let readyPoll = null;
let speaking = false;
let lastSpeechAt = 0;
let preRoll = [];
let utterance = [];
let transcriptionQueue = Promise.resolve();
let transientTimer = null;

function settings() {
  return Object.assign(
    {
      sttProvider: "nemotron",
      language: "auto",
      overlayEnabled: true,
    },
    JSON.parse(localStorage.getItem(SETTINGS_KEY) || "{}"),
  );
}

function applyVisualPreference() {
  root.classList.toggle("visual-disabled", settings().overlayEnabled === false);
}

async function setupOverlayWindow() {
  const win = getCurrentWindow();
  await win.setAlwaysOnTop(true);
  await win.setIgnoreCursorEvents(true);
}

async function positionOverlay() {
  try {
    let monitor = null;
    try {
      const cursor = await cursorPosition();
      monitor = await monitorFromPoint(cursor.x, cursor.y);
    } catch {}

    if (!monitor) monitor = await primaryMonitor();
    if (!monitor) return;

    const win = getCurrentWindow();
    const size = await win.outerSize();
    const scale = Number(monitor.scaleFactor || 1);
    const gap = Math.round(56 * scale);

    const area = monitor.workArea || {
      position: monitor.position,
      size: monitor.size,
    };

    const x =
      area.position.x
      + Math.round((area.size.width - size.width) / 2);
    const y =
      area.position.y
      + area.size.height
      - size.height
      - gap;

    await win.setPosition(new PhysicalPosition(x, y));
  } catch (error) {
    console.warn("Could not position Reflex overlay", error);
  }
}

function setVisualState(state, message) {
  if (transientTimer) clearTimeout(transientTimer);

  orb.setState(state);
  root.dataset.state = state;

  if (message) {
    partial.textContent = message;
    root.classList.add("show-caption");
  } else if (!["error", "warning"].includes(state)) {
    root.classList.remove("show-caption");
  }

  if (state === "success") {
    transientTimer = setTimeout(function () {
      if (active) {
        orb.setState("listening");
        root.dataset.state = "listening";
        root.classList.remove("show-caption");
      }
    }, 420);
  }
}

function resampleTo16k(input, inputRate) {
  if (inputRate === TARGET_RATE) {
    return Int16Array.from(input, function (sample) {
      return Math.max(-32768, Math.min(32767, Math.round(sample * 32767)));
    });
  }

  const ratio = inputRate / TARGET_RATE;
  const length = Math.max(1, Math.floor(input.length / ratio));
  const output = new Int16Array(length);

  for (let i = 0; i < length; i += 1) {
    const start = Math.floor(i * ratio);
    const end = Math.max(start + 1, Math.floor((i + 1) * ratio));
    let sum = 0;
    let count = 0;

    for (let j = start; j < end && j < input.length; j += 1) {
      sum += input[j];
      count += 1;
    }

    const sample = count
      ? sum / count
      : input[Math.min(start, input.length - 1)];

    output[i] = Math.max(-32768, Math.min(32767, Math.round(sample * 32767)));
  }

  return output;
}

function rmsOf(input) {
  let sum = 0;
  for (const sample of input) sum += sample * sample;
  return Math.sqrt(sum / Math.max(1, input.length));
}

let overlaySessionId = null;

function overlaySession() {
  if (overlaySessionId) return overlaySessionId;
  try {
    overlaySessionId = crypto.randomUUID();
  } catch {
    overlaySessionId = "os_" + Math.random().toString(16).slice(2) + Date.now().toString(16);
  }
  return overlaySessionId;
}

async function submitVerifiedTranscript(text, sttLatencyMs) {
  const clean = String(text || "").trim().slice(0, 1000);
  if (!clean) return;
  const nonce = await invoke("transcript_nonce");
  await invoke("submit_transcript", {
    text: clean,
    nonce: nonce,
    sessionId: overlaySession(),
    sttLatencyMs: typeof sttLatencyMs === "number" ? sttLatencyMs : null,
  });
}

function resetUtterance() {
  speaking = false;
  lastSpeechAt = 0;
  preRoll = [];
  utterance = [];
}

function appendPreRoll(pcm) {
  preRoll.push(...pcm);
  if (preRoll.length > PRE_ROLL_SAMPLES) {
    preRoll.splice(0, preRoll.length - PRE_ROLL_SAMPLES);
  }
}

function finalizeNemotronUtterance() {
  if (!speaking || utterance.length < TARGET_RATE * 0.16) {
    resetUtterance();
    return;
  }

  const samples = utterance.slice();
  const voice = settings();
  resetUtterance();

  transcriptionQueue = transcriptionQueue.then(async function () {
    if (!active || voice.sttProvider !== "nemotron") return;

    stateText.textContent = "TRANSCRIBING";
    setVisualState("transcribing");

    try {
      let result = null;
      try {
        const nonce = await invoke("transcript_nonce");
        const streamResult = await invoke("stt_stream_chunk", {
          sessionId: overlaySession(),
          nonce: nonce,
          samples: samples,
          sampleRate: TARGET_RATE,
          language: voice.language || "auto",
          partialHint: null,
          isFinal: true,
        });
        if (streamResult && streamResult.transcript) {
          result = streamResult.transcript;
        }
      } catch {
        // Fallback directly to HTTP utterance path
        result = await invoke("stt_transcribe", {
          samples: samples,
          sampleRate: TARGET_RATE,
          language: voice.language || "auto",
        });
      }
      if (!active) return;

      const text = String(result && result.text ? result.text : "").trim();
      if (text) {
        partial.textContent = text;
        root.classList.add("show-caption");
        await submitVerifiedTranscript(text, result.latency_ms);
      }
    } catch (error) {
      if (!active) return;
      const message = String(error);
      setVisualState("error", message.slice(0, 90));

      if (
        message.toLowerCase().includes("starting")
        || message.toLowerCase().includes("preparing")
      ) {
        nemotronReady = false;
        pollNemotron();
      }
    } finally {
      if (active && root.dataset.state !== "error") {
        stateText.textContent = nemotronReady ? "LISTENING" : "PREPARING";
        orb.setState(nemotronReady ? "listening" : "thinking");
      }
    }
  });
}

function processNemotronFrame(input, inputRate, rms) {
  if (!active || !nemotronReady || settings().sttProvider !== "nemotron") return;

  const pcm = resampleTo16k(input, inputRate);
  const now = performance.now();

  if (rms >= SPEECH_THRESHOLD) {
    if (!speaking) {
      speaking = true;
      utterance = preRoll.slice();
      preRoll = [];
    }
    lastSpeechAt = now;
    orb.setState("hearing");
  } else if (!speaking) {
    orb.setState("listening");
  }

  if (speaking) {
    utterance.push(...pcm);

    const timedOut = lastSpeechAt && now - lastSpeechAt >= END_SILENCE_MS;
    const tooLong = utterance.length >= TARGET_RATE * MAX_COMMAND_SECONDS;
    if (timedOut || tooLong) finalizeNemotronUtterance();
  } else {
    appendPreRoll(pcm);
  }
}

async function setupAudio() {
  if (audio) return;

  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
      channelCount: 1,
    },
  });

  for (const track of stream.getAudioTracks()) {
    track.addEventListener("ended", async function () {
      if (!active) return;
      setVisualState("error", "Microphone disconnected");
      try {
        await emit("reflexdesk://attention", {
          kind: "microphone_disconnected",
          message: "Your microphone disconnected. Reconnect it and start listening again.",
        });
        await invoke("set_listening", { active: false });
      } catch {}
    });
  }

  const context = new AudioContext();
  context.onstatechange = function () {
    if (active && context.state === "suspended") {
      context.resume().catch(function () {});
    }
  };
  await context.resume();
  await context.audioWorklet.addModule("/audio-worklet.js");

  const source = context.createMediaStreamSource(stream);
  const processor = new AudioWorkletNode(context, "reflex-audio-processor");
  const silent = context.createGain();
  silent.gain.value = 0;

  source.connect(processor);
  processor.connect(silent);
  silent.connect(context.destination);

  processor.port.onmessage = function (event) {
    if (!active) return;
    const input = event.data;
    const rms = rmsOf(input);
    orb.setLevel(Math.min(1, 0.12 + rms * 7));
    processNemotronFrame(input, context.sampleRate, rms);
  };

  audio = { stream, context, source, processor, silent };
}

async function teardownAudio() {
  resetUtterance();

  if (!audio) return;

  try { audio.processor.port.onmessage = null; } catch {}
  try { audio.processor.disconnect(); } catch {}
  try { audio.source.disconnect(); } catch {}
  try { audio.silent.disconnect(); } catch {}

  for (const track of audio.stream.getTracks()) track.stop();

  try { await audio.context.close(); } catch {}
  try {
    invoke("stt_cancel_stream", { sessionId: overlaySession() }).catch(function () {});
  } catch {}
  audio = null;
}

async function ensureMoonshine() {
  if (moonshine || sttStarting) return;
  sttStarting = true;

  try {
    const voice = settings();
    const language = voice.language === "auto" ? "en" : voice.language;

    stateText.textContent = "PREPARING";
    setVisualState("thinking");

    moonshine = new MicTranscriber()
      .language(language)
      .modelArch(ModelArch.MediumStreaming)
      .onText(function (text) {
        partial.textContent = text || "";
      })
      .onLine(function (line) {
        const text = String(line && line.text ? line.text : "").trim();
        if (text && active) {
          submitVerifiedTranscript(text, null).catch(function (e) { console.warn("transcript submit failed", e); });
        }
        partial.textContent = "";
      });

    await moonshine.load();
    if (active) await moonshine.start();

    stateText.textContent = "LISTENING";
    setVisualState("listening");
  } catch (error) {
    console.error("Moonshine unavailable", error);
    setVisualState("error", "Local speech fallback unavailable");
    moonshine = null;
  } finally {
    sttStarting = false;
  }
}

function clearReadyPoll() {
  if (readyPoll) clearInterval(readyPoll);
  readyPoll = null;
}

function pollNemotron() {
  if (readyPoll) return;

  readyPoll = setInterval(async function () {
    if (!active || settings().sttProvider !== "nemotron") {
      clearReadyPoll();
      return;
    }

    try {
      const status = await invoke("stt_status");
      nemotronReady = Boolean(status.ready);

      if (nemotronReady) {
        stateText.textContent = "LISTENING";
        partial.textContent = "";
        setVisualState("listening");
        clearReadyPoll();
      }
    } catch {}
  }, 500);
}

async function ensureNemotron() {
  stateText.textContent = "PREPARING";
  setVisualState("thinking", "Preparing local speech…");

  try {
    await invoke("prepare_engine");
    const status = await invoke("stt_status");
    nemotronReady = Boolean(status.ready);

    if (nemotronReady) {
      stateText.textContent = "LISTENING";
      partial.textContent = "";
      setVisualState("listening");
    } else {
      pollNemotron();
    }
  } catch (error) {
    nemotronReady = false;
    setVisualState("error", "Local speech engine unavailable");
  }
}

async function ensureStt() {
  const provider = settings().sttProvider || "nemotron";

  if (provider === "manual") {
    stateText.textContent = "LISTENING";
    setVisualState("listening");
    return;
  }

  if (provider === "moonshine") {
    await ensureMoonshine();
    return;
  }

  await ensureNemotron();
}

async function applyActive(next) {
  active = Boolean(next);
  root.classList.toggle("listening", active);
  applyVisualPreference();

  if (active) {
    await positionOverlay();
    try { await getCurrentWindow().show(); } catch {}
    stateText.textContent = "STARTING";
    setVisualState("listening");

    try {
      await setupAudio();
      await ensureStt();
    } catch (error) {
      stateText.textContent = "MICROPHONE";
      setVisualState("error", "Microphone permission is required");
    }
    return;
  }

  clearReadyPoll();
  nemotronReady = false;

  if (moonshine) {
    try { await moonshine.stop(); } catch {}
  }

  await teardownAudio();
  partial.textContent = "";
  stateText.textContent = "";
  setVisualState("idle");
  root.classList.remove("show-caption");

  try { await getCurrentWindow().hide(); } catch {}
}

setupOverlayWindow();
applyVisualPreference();

listen("reflexdesk://settings", function () {
  applyVisualPreference();
});

listen("reflexdesk://active", function (event) {
  applyActive(event.payload);
});

listen("reflexdesk://stt-status", function (event) {
  if (!active || settings().sttProvider !== "nemotron") return;

  const message = String(
    event.payload && event.payload.message ? event.payload.message : "",
  ).replace(/\x1b\[[0-9;]*m/g, "");

  if (message && !nemotronReady) {
    partial.textContent = "Preparing local speech…";
    root.classList.add("show-caption");
  }
});

listen("reflexdesk://visual-state", function (event) {
  if (!active) return;

  const payload = event.payload;
  const state =
    typeof payload === "string"
      ? payload
      : payload && payload.state
        ? payload.state
        : "listening";

  const message =
    payload && typeof payload === "object" && payload.message
      ? String(payload.message).slice(0, 90)
      : "";

  setVisualState(state, message);
});

listen("reflexdesk://stt-partial", function (event) {
  if (!active) return;
  const payload = event.payload;
  if (payload && payload.text) {
    partial.textContent = payload.text;
    root.classList.add("show-caption");
    if (payload.speculative_action) {
      orb.setState("thinking");
    }
  }
});
