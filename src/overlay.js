import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MicTranscriber, ModelArch } from "@moonshine-ai/moonshine-wasm";
import { ParticleOrb } from "./lib/particles.js";

const orb = new ParticleOrb(document.getElementById("orb"));
orb.start();
const partial = document.getElementById("partialText");
const stateText = document.getElementById("stateText");
const root = document.getElementById("overlayRoot");
let mic = null, transcriber = null, sttStarting = false, active = false;

async function setupOverlayWindow() {
  const win = getCurrentWindow();
  await win.setAlwaysOnTop(true);
  await win.setIgnoreCursorEvents(true);
}

async function setupEnergyMeter() {
  if (mic) return;
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true } });
    const context = new AudioContext();
    const source = context.createMediaStreamSource(stream);
    const analyser = context.createAnalyser();
    analyser.fftSize = 512;
    source.connect(analyser);
    const data = new Uint8Array(analyser.fftSize);
    const tick = () => {
      analyser.getByteTimeDomainData(data);
      let sum = 0;
      for (const v of data) { const x = (v - 128) / 128; sum += x * x; }
      const rms = Math.sqrt(sum / data.length);
      orb.setLevel(active ? Math.min(1, 0.18 + rms * 5.8) : 0.04);
      requestAnimationFrame(tick);
    };
    tick();
    mic = { stream, context };
  } catch (error) {
    stateText.textContent = "MIC BLOCKED";
    partial.textContent = String(error);
  }
}

async function ensureStt() {
  if (transcriber || sttStarting) return;
  sttStarting = true;
  try {
    stateText.textContent = "LOADING LOCAL STT";
    const voiceSettings = JSON.parse(localStorage.getItem("reflexdesk.settings.v1") || "{}");
    if ((voiceSettings.sttProvider || "moonshine") !== "moonshine") {
      stateText.textContent = "VOICE VISUALIZER";
      partial.textContent = "STT disabled in settings";
      return;
    }
    transcriber = new MicTranscriber()
      .language(voiceSettings.language || "en")
      .modelArch(ModelArch.MediumStreaming)
      .onText((text) => { partial.textContent = text || ""; })
      .onLine((line) => {
        const text = String(line?.text || "").trim();
        if (text) emit("reflexdesk://transcript", { text });
        partial.textContent = "";
      });
    await transcriber.load();
    if (active) await transcriber.start();
    stateText.textContent = "LISTENING";
  } catch (error) {
    console.error("Moonshine unavailable", error);
    stateText.textContent = "VOICE VISUALIZER";
    partial.textContent = "STT unavailable — open settings";
    transcriber = null;
  } finally {
    sttStarting = false;
  }
}

async function applyActive(next) {
  active = Boolean(next);
  root.classList.toggle("listening", active);
  stateText.textContent = active ? "LISTENING" : "PAUSED";
  if (active) {
    await setupEnergyMeter();
    await ensureStt();
    if (transcriber) { try { await transcriber.start(); } catch (_) {} }
  } else if (transcriber) {
    try { await transcriber.stop(); } catch (_) {}
    partial.textContent = "";
  }
}

setupOverlayWindow();
listen("reflexdesk://active", (event) => applyActive(event.payload));
listen("reflexdesk://working", (event) => {
  root.classList.toggle("working", Boolean(event.payload));
  if (event.payload) { stateText.textContent = "WORKING"; orb.setLevel(0.78); }
  else if (active) stateText.textContent = "LISTENING";
});
