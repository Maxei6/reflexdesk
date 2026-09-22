import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MicTranscriber, ModelArch } from "@moonshine-ai/moonshine-wasm";
import { ParticleOrb } from "./lib/particles.js";

const orb = new ParticleOrb(document.getElementById("orb"));
orb.start();

const partial = document.getElementById("partialText");
const stateText = document.getElementById("stateText");
const root = document.getElementById("overlayRoot");

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

function settings() {
  return {
    sttProvider: "nemotron",
    language: "auto",
    ...JSON.parse(localStorage.getItem(SETTINGS_KEY) || "{}"),
  };
}

async function setupOverlayWindow() {
  const win = getCurrentWindow();
  await win.setAlwaysOnTop(true);
  await win.setIgnoreCursorEvents(true);
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
    const sample = count ? sum / count : input[Math.min(start, input.length - 1)];
    output[i] = Math.max(-32768, Math.min(32767, Math.round(sample * 32767)));
  }

  return output;
}

function rmsOf(input) {
  let sum = 0;
  for (const sample of input) sum += sample * sample;
  return Math.sqrt(sum / Math.max(1, input.length));
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

    stateText.textContent = "TRANSCRIBING · NEMOTRON 3.5";
    root.classList.add("working");
    orb.setLevel(0.78);

    try {
      const result = await invoke("stt_transcribe", {
        samples,
        sampleRate: TARGET_RATE,
        language: voice.language || "auto",
      });
      const text = String(result && result.text ? result.text : "").trim();
      if (text) {
        partial.textContent = text;
        await emit("reflexdesk://transcript", {
          text,
          sttLatencyMs: result.latency_ms,
        });
      }
    } catch (error) {
      const message = String(error);
      partial.textContent = message;
      if (
        message.toLowerCase().includes("starting")
        || message.toLowerCase().includes("downloading")
      ) {
        nemotronReady = false;
        pollNemotron();
      }
    } finally {
      root.classList.remove("working");
      if (active) {
        stateText.textContent = nemotronReady
          ? "LISTENING · NEMOTRON 3.5"
          : "SETTING UP NEMOTRON 3.5";
        orb.setLevel(0.2);
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

  const context = new AudioContext();
  await context.resume();

  const source = context.createMediaStreamSource(stream);
  const analyser = context.createAnalyser();
  analyser.fftSize = 512;
  source.connect(analyser);

  const processor = context.createScriptProcessor(4096, 1, 1);
  source.connect(processor);
  processor.connect(context.destination);

  processor.onaudioprocess = function (event) {
    const input = event.inputBuffer.getChannelData(0);
    const rms = rmsOf(input);
    orb.setLevel(active ? Math.min(1, 0.14 + rms * 6.6) : 0.04);
    processNemotronFrame(input, context.sampleRate, rms);
  };

  audio = { stream, context, source, analyser, processor };
}

async function teardownAudio() {
  resetUtterance();

  if (!audio) return;
  try { audio.processor.disconnect(); } catch {}
  try { audio.source.disconnect(); } catch {}
  try { audio.analyser.disconnect(); } catch {}
  for (const track of audio.stream.getTracks()) track.stop();
  try { await audio.context.close(); } catch {}
  audio = null;
}

async function ensureMoonshine() {
  if (moonshine || sttStarting) return;
  sttStarting = true;

  try {
    const voice = settings();
    const language = voice.language === "auto" ? "en" : voice.language;
    stateText.textContent = "LOADING MOONSHINE FALLBACK";

    moonshine = new MicTranscriber()
      .language(language)
      .modelArch(ModelArch.MediumStreaming)
      .onText(function (text) { partial.textContent = text || ""; })
      .onLine(function (line) {
        const text = String(line && line.text ? line.text : "").trim();
        if (text) emit("reflexdesk://transcript", { text });
        partial.textContent = "";
      });

    await moonshine.load();
    if (active) await moonshine.start();
    stateText.textContent = "LISTENING · MOONSHINE";
  } catch (error) {
    console.error("Moonshine unavailable", error);
    partial.textContent = "Moonshine unavailable: " + String(error);
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
        stateText.textContent = "LISTENING · NEMOTRON 3.5";
        partial.textContent = "";
        clearReadyPoll();
      }
    } catch {}
  }, 500);
}

async function ensureNemotron() {
  stateText.textContent = "SETTING UP NEMOTRON 3.5";
  partial.textContent = "First run downloads the local ~458 MB model.";

  try {
    const status = await invoke("stt_start");
    nemotronReady = Boolean(status.ready);
    if (nemotronReady) {
      stateText.textContent = "LISTENING · NEMOTRON 3.5";
      partial.textContent = "";
    } else {
      pollNemotron();
    }
  } catch (error) {
    nemotronReady = false;
    partial.textContent = "Nemotron unavailable: " + String(error);
    stateText.textContent = "NEMOTRON RUNTIME ERROR";
  }
}

async function ensureStt() {
  const provider = settings().sttProvider || "nemotron";
  if (provider === "manual") {
    stateText.textContent = "VOICE VISUALIZER";
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

  if (active) {
    stateText.textContent = "STARTING LOCAL VOICE";
    try {
      await setupAudio();
      await ensureStt();
    } catch (error) {
      stateText.textContent = "MIC BLOCKED";
      partial.textContent = String(error);
    }
  } else {
    clearReadyPoll();
    nemotronReady = false;
    if (moonshine) {
      try { await moonshine.stop(); } catch {}
    }
    await teardownAudio();
    partial.textContent = "";
    stateText.textContent = "PAUSED";
    orb.setLevel(0.04);
    try { await getCurrentWindow().hide(); } catch {}
  }
}

setupOverlayWindow();

listen("reflexdesk://active", function (event) {
  applyActive(event.payload);
});

listen("reflexdesk://stt-status", function (event) {
  if (!active || settings().sttProvider !== "nemotron") return;
  const message = String(event.payload && event.payload.message ? event.payload.message : "");
  if (message) {
    partial.textContent = message.replace(/\x1b\[[0-9;]*m/g, "").slice(0, 120);
  }
});

listen("reflexdesk://working", function (event) {
  root.classList.toggle("working", Boolean(event.payload));
  if (event.payload) {
    stateText.textContent = "WORKING";
    orb.setLevel(0.82);
  } else if (active) {
    stateText.textContent =
      settings().sttProvider === "nemotron" && nemotronReady
        ? "LISTENING · NEMOTRON 3.5"
        : "LISTENING";
  }
});
