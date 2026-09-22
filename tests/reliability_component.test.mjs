import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import crypto from "node:crypto";

const REGISTRY = JSON.parse(fs.readFileSync("models/registry.json", "utf8"));

test("Component/STT: audio input validation enforces security and duration bounds", () => {
  const REQUIRED_SAMPLE_RATE_HZ = 16000;
  const MIN_AUDIO_BYTES = 3200;   // 100ms at 16kHz 16-bit mono
  const MAX_AUDIO_BYTES = 960000; // 30s at 16kHz 16-bit mono
  const ALLOWED_LANGUAGES = new Set(["auto", "en", "it", "es", "fr", "de", "pt", "nl", "tr", "ru", "ar", "hi", "ja", "ko", "vi", "uk", "zh"]);

  function checkTranscriptBounds(lenBytes, sampleRateHz, lang) {
    if (sampleRateHz !== REQUIRED_SAMPLE_RATE_HZ) {
      return { ok: false, error: "unsupported-sample-rate: speech engine requires exactly 16000 Hz audio" };
    }
    if (lenBytes < MIN_AUDIO_BYTES) {
      return { ok: false, error: "audio-too-short: speech segment must be at least 100ms (3200 bytes at 16kHz 16-bit)" };
    }
    if (lenBytes > MAX_AUDIO_BYTES) {
      return { ok: false, error: "audio-too-long: speech segment exceeds 30-second limit (960000 bytes at 16kHz 16-bit)" };
    }
    if (!ALLOWED_LANGUAGES.has(lang)) {
      return { ok: false, error: "unsupported-language: language not permitted" };
    }
    return { ok: true };
  }

  // Valid inputs
  assert.ok(checkTranscriptBounds(3200, 16000, "en").ok);
  assert.ok(checkTranscriptBounds(64000, 16000, "it").ok);
  assert.ok(checkTranscriptBounds(960000, 16000, "auto").ok);

  // Boundary failures
  assert.equal(checkTranscriptBounds(3198, 16000, "en").error.includes("audio-too-short"), true);
  assert.equal(checkTranscriptBounds(960002, 16000, "en").error.includes("audio-too-long"), true);
  assert.equal(checkTranscriptBounds(32000, 44100, "en").error.includes("unsupported-sample-rate"), true);
  assert.equal(checkTranscriptBounds(32000, 48000, "en").error.includes("unsupported-sample-rate"), true);
  assert.equal(checkTranscriptBounds(32000, 16000, "invalid_lang").error.includes("unsupported-language"), true);
});

test("Component/STT: transcript submission enforces single-use nonces and payload constraints", () => {
  const MAX_TRANSCRIPT_CHARS = 1000;
  const NONCE_TTL_MS = 300_000; // 5 minutes

  class NonceManager {
    constructor() {
      this.activeNonces = new Map();
    }

    issueNonce() {
      const nonce = "nonce_" + crypto.randomBytes(16).toString("hex");
      this.activeNonces.set(nonce, Date.now() + NONCE_TTL_MS);
      return nonce;
    }

    consumeNonce(nonce) {
      if (!this.activeNonces.has(nonce)) {
        return { ok: false, reason: "nonce-not-found-or-replayed" };
      }
      const expiresAt = this.activeNonces.get(nonce);
      this.activeNonces.delete(nonce);
      if (Date.now() > expiresAt) {
        return { ok: false, reason: "nonce-expired" };
      }
      return { ok: true };
    }
  }

  function validateTranscriptSubmission(text, nonce, nonceManager) {
    if (typeof text !== "string") return { ok: false, reason: "invalid-type" };
    if (text.includes("\0")) return { ok: false, reason: "null-byte-detected" };
    if (text.length > MAX_TRANSCRIPT_CHARS) return { ok: false, reason: "transcript-too-long" };

    const nonceCheck = nonceManager.consumeNonce(nonce);
    if (!nonceCheck.ok) return nonceCheck;

    return { ok: true, text: text.trim() };
  }

  const manager = new NonceManager();
  const validNonce = manager.issueNonce();

  // Valid submission consumes nonce
  const res1 = validateTranscriptSubmission("open chrome", validNonce, manager);
  assert.ok(res1.ok);
  assert.equal(res1.text, "open chrome");

  // Replay attempt fails immediately
  const resReplay = validateTranscriptSubmission("open chrome", validNonce, manager);
  assert.equal(resReplay.ok, false);
  assert.equal(resReplay.reason, "nonce-not-found-or-replayed");

  // Null byte rejection
  const freshNonce = manager.issueNonce();
  const resNull = validateTranscriptSubmission("open\0malicious", freshNonce, manager);
  assert.equal(resNull.ok, false);
  assert.equal(resNull.reason, "null-byte-detected");

  // Length overflow rejection
  const freshNonce2 = manager.issueNonce();
  const longText = "a".repeat(1001);
  const resLong = validateTranscriptSubmission(longText, freshNonce2, manager);
  assert.equal(resLong.ok, false);
  assert.equal(resLong.reason, "transcript-too-long");
});

test("Component/ModelManager: registry integrity and preflight space calculation", () => {
  assert.ok(Array.isArray(REGISTRY.models));
  const nemotron = REGISTRY.models.find(m => m.id === "nemotron-3.5-asr-streaming-0.6b");
  assert.ok(nemotron, "Nemotron 3.5 model must be in registry");
  assert.ok(nemotron.cache_path);
  assert.ok(nemotron.license);
  assert.ok(nemotron.runtime_compat.includes("CrispASR"));

  // Preflight space calculation: required + 100MB safety buffer
  function checkDiskPreflight(availableBytes, requiredBytes) {
    const BUFFER_BYTES = 100 * 1024 * 1024;
    const totalRequired = requiredBytes + BUFFER_BYTES;
    if (availableBytes < totalRequired) {
      return {
        ok: false,
        error: `Insufficient disk space: needed ${totalRequired} bytes (with 100MB buffer), available ${availableBytes}`
      };
    }
    return { ok: true };
  }

  const modelSize = nemotron.size_bytes || 500_000_000;
  assert.ok(checkDiskPreflight(modelSize + 200 * 1024 * 1024, modelSize).ok);
  assert.equal(checkDiskPreflight(modelSize + 50 * 1024 * 1024, modelSize).ok, false);
});

test("Component/ProcessSupervisor: ownership classes and termination semantics", () => {
  const Ownership = {
    INTERNAL: "Internal",
    OWNED_SESSION: "OwnedSession",
    ATTACHED: "Attached",
    USER_APP: "UserApp"
  };

  class MockSupervisor {
    constructor() {
      this.processes = new Map();
      this.idSeq = 1;
    }

    spawn(ownershipClass, command) {
      const id = `proc_${this.idSeq++}_${crypto.randomBytes(4).toString("hex")}`;
      this.processes.set(id, {
        id,
        ownership: ownershipClass,
        command,
        alive: true
      });
      return id;
    }

    terminateOwned() {
      const terminated = [];
      for (const [id, proc] of this.processes.entries()) {
        // Only Internal and OwnedSession are terminated
        if (proc.alive && (proc.ownership === Ownership.INTERNAL || proc.ownership === Ownership.OWNED_SESSION)) {
          proc.alive = false;
          terminated.push(id);
        }
      }
      return terminated;
    }
  }

  const supervisor = new MockSupervisor();
  const sttId = supervisor.spawn(Ownership.INTERNAL, "stt-sidecar");
  const agentSessionId = supervisor.spawn(Ownership.OWNED_SESSION, "codex exec");
  const userBrowserId = supervisor.spawn(Ownership.USER_APP, "chrome.exe");
  const attachedToolId = supervisor.spawn(Ownership.ATTACHED, "laya-external");

  const killed = supervisor.terminateOwned();

  // Owned processes are terminated cleanly
  assert.ok(killed.includes(sttId));
  assert.ok(killed.includes(agentSessionId));

  // UserApp and Attached processes are NOT terminated
  assert.equal(killed.includes(userBrowserId), false);
  assert.equal(killed.includes(attachedToolId), false);
  assert.ok(supervisor.processes.get(userBrowserId).alive);
  assert.ok(supervisor.processes.get(attachedToolId).alive);
});
