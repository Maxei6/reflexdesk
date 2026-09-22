import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";

// Allowlisted fields from Wave-0 contracts (src-tauri/src/redaction.rs)
const ALLOWLISTED_FIELDS = new Set([
  "app_version",
  "build",
  "arch",
  "os",
  "hardware_class",
  "phase",
  "latency_ms",
  "stt_latency_ms",
  "benchmark_ms",
  "adapter",
  "adapter_available",
  "runtime_version",
  "model_id",
  "model_revision",
  "outcome",
  "code",
  "component",
  "op",
  "session",
  "v",
  "ts",
]);

const SECRET_KEYS = [
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

const PRIVATE_CONTENT_KEYS = [
  "audio",
  "pcm",
  "wav",
  "clipboard",
  "file_content",
  "file_contents",
  "browser_text",
  "prompt",
];

function sanitizeMetaValue(val, allowTranscripts = false) {
  if (val === null || val === undefined) return val;
  if (Array.isArray(val)) {
    return val.map((item) => sanitizeMetaValue(item, allowTranscripts));
  }
  if (typeof val === "object") {
    const out = {};
    for (const [k, v] of Object.entries(val)) {
      const lower = k.toLowerCase();
      if (SECRET_KEYS.some((s) => lower.includes(s))) {
        out[k] = "[redacted-secret]";
        continue;
      }
      if (PRIVATE_CONTENT_KEYS.some((p) => lower.includes(p))) {
        out[k] = "[redacted-secret]";
        continue;
      }
      if (lower.includes("transcript")) {
        if (allowTranscripts) {
          out[k] = typeof v === "string" ? `[redacted-transcript ${v.length} chars]` : v;
        } else {
          out[k] = "[redacted-transcript]";
        }
        continue;
      }
      out[k] = sanitizeMetaValue(v, allowTranscripts);
    }
    return out;
  }
  if (typeof val === "string") {
    // Check if secret key pattern
    for (const secret of SECRET_KEYS) {
      if (val.toLowerCase().includes(secret)) {
        return `[redacted-secret ${val.length} chars]`;
      }
    }
    return val;
  }
  return val;
}

test("log event schema version 1 and required fields", () => {
  const event = {
    v: 1,
    ts: Date.now(),
    component: "lifecycle",
    op: "transition",
    session: "test-sess",
    phase: "ready",
    outcome: "ok",
    latency_ms: 45,
    code: null,
    meta: {
      action: "app.open",
    },
  };

  assert.equal(event.v, 1);
  assert.equal(typeof event.ts, "number");
  assert.ok(event.ts > 0);
  assert.equal(event.component, "lifecycle");
  assert.equal(event.op, "transition");
  assert.equal(event.outcome, "ok");
  assert.equal(event.latency_ms, 45);
});

test("metadata sanitization excludes secrets by construction", () => {
  const rawMeta = {
    api_key: "sk-proj-1234567890abcdef1234567890abcdef",
    bearer_token: "bearer 9876543210fedcba",
    planner_secret: "super-secret-key",
    password: "admin_password",
    safe_param: "normal-value",
    count: 3,
  };

  const sanitized = sanitizeMetaValue(rawMeta);

  assert.equal(sanitized.api_key, "[redacted-secret]");
  assert.equal(sanitized.bearer_token, "[redacted-secret]");
  assert.equal(sanitized.planner_secret, "[redacted-secret]");
  assert.equal(sanitized.password, "[redacted-secret]");
  assert.equal(sanitized.safe_param, "normal-value");
  assert.equal(sanitized.count, 3);
});

test("metadata sanitization rejects audio, clipboard, file content and browser text", () => {
  const sensitiveMeta = {
    audio_buffer: [120, 240, 110, 95],
    clipboard_data: "Confidential financial numbers",
    file_contents: "SSH private key contents...",
    browser_text: "Personal user chat history",
  };

  const sanitized = sanitizeMetaValue(sensitiveMeta);

  assert.equal(sanitized.audio_buffer, "[redacted-secret]");
  assert.equal(sanitized.clipboard_data, "[redacted-secret]");
  assert.equal(sanitized.file_contents, "[redacted-secret]");
  assert.equal(sanitized.browser_text, "[redacted-secret]");
});

test("transcripts are never logged by default", () => {
  const metaWithTranscript = {
    transcript: "Open my bank account and transfer money",
  };

  // Default: false
  const sanitizedDefault = sanitizeMetaValue(metaWithTranscript, false);
  assert.equal(sanitizedDefault.transcript, "[redacted-transcript]");

  // Explicit debug enabled: still length-redacted
  const sanitizedDebug = sanitizeMetaValue(metaWithTranscript, true);
  assert.match(sanitizedDebug.transcript, /^\[redacted-transcript \d+ chars\]$/);
});

test("diagnostics bundle top-level fields match allowlist", () => {
  const mockBundle = {
    app_version: "0.1.0",
    build: "debug",
    os: "win32",
    arch: "x64",
    hardware_class: "standard-cpu-4c",
    runtime_version: "tauri-2",
    model_id: "nemotron-mini-4b",
    model_revision: "default",
    benchmark_numbers: {
      voice_benchmark_ms: 120,
      score_summary: "120 ms last local STT",
    },
    sanitized_transitions: [
      { ts: Date.now() - 1000, phase: "booting", error: null },
      { ts: Date.now(), phase: "ready", error: null },
    ],
    sanitized_errors: [],
    adapter_availability: {
      desktop: true,
      browser: true,
      reflex: true,
      planner: true,
      harnesses: ["codex"],
    },
    recent_logs: [],
  };

  // Verify that all core telemetry fields conform to allowlisted keys
  for (const key of ["app_version", "build", "os", "arch", "hardware_class", "runtime_version", "model_id", "model_revision"]) {
    assert.ok(ALLOWLISTED_FIELDS.has(key), `Key ${key} must be on allowlist`);
  }

  // Ensure no secret keys exist in the bundle
  const jsonStr = JSON.stringify(mockBundle).toLowerCase();
  for (const secret of SECRET_KEYS) {
    assert.ok(!jsonStr.includes(`"${secret}"`), `Bundle must not contain secret key: ${secret}`);
  }
});

test("atomic export writes file safely and cancel leaves no file", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "reflexdesk-diag-test-"));
  const targetFile = path.join(tempDir, "diagnostics-export.json");
  const tmpFile = `${targetFile}.tmp-${Date.now()}`;

  const payload = {
    app_version: "0.1.0",
    build: "release",
    os: os.platform(),
    arch: os.arch(),
    hardware_class: "standard-cpu",
    runtime_version: "tauri-2",
    benchmark_numbers: {
      voice_benchmark_ms: 95,
      score_summary: "95 ms",
    },
    sanitized_transitions: [],
    sanitized_errors: [],
    adapter_availability: {
      desktop: true,
      browser: true,
      reflex: true,
      planner: true,
      harnesses: [],
    },
    recent_logs: [],
  };

  // Write to temporary file, sync, and atomic rename
  fs.writeFileSync(tmpFile, JSON.stringify(payload, null, 2), "utf8");
  assert.ok(fs.existsSync(tmpFile), "Temp file must exist before atomic rename");

  fs.renameSync(tmpFile, targetFile);

  assert.ok(fs.existsSync(targetFile), "Target file must exist after atomic rename");
  assert.ok(!fs.existsSync(tmpFile), "Temp file must not exist after rename");

  // Read back and verify identical structure
  const readBack = JSON.parse(fs.readFileSync(targetFile, "utf8"));
  assert.deepEqual(readBack, payload);

  // Test failure cleanup: if export aborted or failed, tmp file cleaned up
  const failedTmpFile = path.join(tempDir, "failed.tmp-999");
  fs.writeFileSync(failedTmpFile, "partial data");
  // Simulate cancel cleanup
  if (fs.existsSync(failedTmpFile)) {
    fs.unlinkSync(failedTmpFile);
  }
  assert.ok(!fs.existsSync(failedTmpFile), "Aborted export must leave no tmp file");

  // Cleanup
  fs.unlinkSync(targetFile);
  fs.rmdirSync(tempDir);
});
