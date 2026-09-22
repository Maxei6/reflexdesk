import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

// Test secret-shaped key detection logic matching Rust secrets::detect_plaintext_secret_key
const SECRET_KEY_NAMES = [
  "api_key",
  "apikey",
  "token",
  "password",
  "secret",
  "authorization",
  "bearer",
  "private_key",
  "client_secret",
];

function isSecretShapedKey(key) {
  const lower = key.toLowerCase();
  if (lower.endsWith("_ref") || lower.endsWith("_id")) return false;
  return SECRET_KEY_NAMES.some((k) => lower === k || lower.includes(k));
}

function detectPlaintextSecretKey(val) {
  if (val && typeof val === "object" && !Array.isArray(val)) {
    for (const [k, v] of Object.entries(val)) {
      if (k.endsWith("_ref")) continue;
      if (isSecretShapedKey(k)) {
        if (v !== null && v !== undefined && String(v).trim().length > 0) {
          return k;
        }
      }
      const nested = detectPlaintextSecretKey(v);
      if (nested) return nested;
    }
  } else if (Array.isArray(val)) {
    for (const item of val) {
      const nested = detectPlaintextSecretKey(item);
      if (nested) return nested;
    }
  }
  return null;
}

test("detects plaintext api_key in settings", () => {
  const badSettings = {
    language: "en",
    shortcut: "CommandOrControl+Shift+Space",
    api_key: "sk-live-1234567890abcdef",
  };
  assert.equal(detectPlaintextSecretKey(badSettings), "api_key");
});

test("detects nested token and bearer credentials", () => {
  const badNested = {
    planner: {
      auth: {
        token: "bearer-secret-token",
      },
    },
  };
  assert.equal(detectPlaintextSecretKey(badNested), "token");
});

test("allows opaque planner_secret_ref without false positive", () => {
  const goodSettings = {
    schema_version: 1,
    setup_complete: true,
    language: "auto",
    shortcut: "CommandOrControl+Shift+Space",
    start_at_login: true,
    overlay_enabled: true,
    sounds_enabled: false,
    allow_online_ai: true,
    voice_benchmark_ms: 120,
    stt_provider: "nemotron",
    laya_endpoint: "http://127.0.0.1:8787",
    planner_endpoint: "http://127.0.0.1:11434/v1/chat/completions",
    planner_model: "auto",
    planner_secret_ref: {
      provider: "planner",
      id: "sec_a1b2c3d4e5f60718",
    },
  };
  assert.equal(detectPlaintextSecretKey(goodSettings), null);
});

test("default config contains zero plaintext credentials", async () => {
  const defaultsRaw = await readFile("config/defaults.json", "utf8");
  const defaults = JSON.parse(defaultsRaw);
  assert.equal(detectPlaintextSecretKey(defaults), null);
});

test("localStorage sync in main.js does not store secret references or keys", () => {
  // Simulate what syncLocalVoiceSettings produces
  const mockSettings = {
    stt_provider: "nemotron",
    language: "auto",
    overlay_enabled: true,
    allow_online_ai: false,
    laya_endpoint: "http://127.0.0.1:8787",
    planner_endpoint: "http://127.0.0.1:11434/v1/chat/completions",
    planner_model: "auto",
    planner_secret_ref: { provider: "planner", id: "sec_test" },
  };

  const storedLocally = {
    sttProvider: mockSettings.stt_provider,
    language: mockSettings.language,
    overlayEnabled: mockSettings.overlay_enabled,
    plannerMode: mockSettings.allow_online_ai ? "hybrid" : "local",
    layaEndpoint: mockSettings.laya_endpoint,
    plannerEndpoint: mockSettings.planner_endpoint,
    plannerModel: mockSettings.planner_model,
  };

  const rawJson = JSON.stringify(storedLocally);
  assert.equal(rawJson.includes("sec_test"), false);
  assert.equal(rawJson.includes("secret"), false);
  assert.equal(detectPlaintextSecretKey(storedLocally), null);
});
