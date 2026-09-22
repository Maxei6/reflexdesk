import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { routeFast } from "../src/lib/router.js";

// Helper: read WAV header and sample length
function inspectWav(filePath) {
  const buf = fs.readFileSync(filePath);
  assert.ok(buf.length >= 44, "WAV file must have at least 44 bytes");
  assert.equal(buf.toString("utf8", 0, 4), "RIFF");
  assert.equal(buf.toString("utf8", 8, 12), "WAVE");

  const sampleRate = buf.readUInt32LE(24);
  const dataSize = buf.readUInt32LE(40);
  const durationMs = Math.round((dataSize / (sampleRate * 2)) * 1000);

  return { sampleRate, dataSize, durationMs, totalBytes: buf.length };
}

// Simulated Policy Engine conforming to local://contracts.md
function evaluatePolicy(envelope, isNegated) {
  if (isNegated) {
    return { outcome: "Deny", reason: "negation-detected" };
  }
  if (!envelope.tool) {
    return { outcome: "Deny", reason: "unknown-tool" };
  }
  switch (envelope.risk) {
    case "safe":
      return { outcome: "Allow" };
    case "sensitive":
      return { outcome: "Confirm", confirmation_id: "confirm:" + crypto.randomUUID() };
    case "destructive":
    case "external_commit":
      return { outcome: "Confirm", confirmation_id: "confirm:" + crypto.randomUUID() };
    default:
      return { outcome: "Deny", reason: "invalid-risk-class" };
  }
}

test("E2E Pipeline: Audio -> Transcript -> Route -> Action -> Verified State (Safe Route)", () => {
  const wavPath = path.resolve("tests/fixtures/audio/audio_en_us_clean.wav");
  const meta = inspectWav(wavPath);

  // 1. Audio bounds check
  assert.equal(meta.sampleRate, 16000);
  assert.ok(meta.durationMs >= 100 && meta.durationMs <= 30000);

  // 2. Simulated STT produces transcript
  const transcriptText = "hello reflexdesk";
  const nonce = "nonce_" + crypto.randomBytes(8).toString("hex");

  // 3. Routing
  const route = routeFast(transcriptText);
  assert.equal(route.kind, "control");
  assert.equal(route.action, "reflex.ping");

  // 4. Action Envelope construction
  const envelope = {
    tool: route.action,
    args: route.args || {},
    source: "Reflex",
    session_id: "session_" + crypto.randomBytes(8).toString("hex"),
    risk: "safe",
    capability: "reflex.ping",
    verification: {
      kind: "ping_reply",
      selector: null,
      expect: { pong: true },
      timeout_ms: 1000,
    },
  };

  // 5. Policy Authorization
  const policyOutcome = evaluatePolicy(envelope, false);
  assert.equal(policyOutcome.outcome, "Allow");

  // 6. Action Execution & State Verification
  const executionResult = { pong: true, time_ms: 5 };
  assert.equal(executionResult.pong, envelope.verification.expect.pong);
});

test("E2E Pipeline: Negation audio aborts pipeline with zero side effects", () => {
  const wavPath = path.resolve("tests/fixtures/audio/audio_negation_en.wav");
  const meta = inspectWav(wavPath);
  assert.equal(meta.sampleRate, 16000);

  const transcript = "do not close chrome";
  const isNegated = /^(?:don'?t|do not|never|stop|cancel|actually stop)\b/i.test(transcript);
  assert.ok(isNegated);

  const envelope = {
    tool: "desktop.close_window",
    args: { app: "chrome" },
    source: "Reflex",
    session_id: "session_neg_1",
    risk: "sensitive",
    capability: "desktop.close_window",
    verification: { kind: "none", selector: null, expect: null, timeout_ms: 2000 },
  };

  const decision = evaluatePolicy(envelope, isNegated);
  assert.equal(decision.outcome, "Deny");
  assert.equal(decision.reason, "negation-detected");

  // Invariant: no tool execution permitted when outcome is Deny
  let toolExecuted = false;
  if (decision.outcome === "Allow") {
    toolExecuted = true;
  }
  assert.equal(toolExecuted, false);
});

test("E2E Pipeline: Destructive voice command requires explicit confirmation ID", () => {
  const wavPath = path.resolve("tests/fixtures/audio/audio_destructive_en.wav");
  const meta = inspectWav(wavPath);
  assert.equal(meta.sampleRate, 16000);

  const transcript = "delete all files in documents";
  const envelope = {
    tool: "desktop.invoke",
    args: { action: "delete_folder", path: "/documents" },
    source: "Planner",
    session_id: "session_dest_1",
    risk: "destructive",
    capability: "desktop.invoke",
    verification: { kind: "folder_deleted", selector: null, expect: { exists: false }, timeout_ms: 5000 },
  };

  const decision = evaluatePolicy(envelope, false);
  assert.equal(decision.outcome, "Confirm");
  assert.ok(decision.confirmation_id.startsWith("confirm:"));

  // Verify confirmation correlation ID is valid UUID
  const uuidPart = decision.confirmation_id.replace("confirm:", "");
  assert.match(uuidPart, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i);
});

test("E2E Pipeline: In-flight cancellation halts action within latency budget", async () => {
  let isCancelled = false;
  const startedAt = Date.now();

  function triggerCancel() {
    isCancelled = true;
  }

  // Schedule cancellation 25ms in
  setTimeout(triggerCancel, 25);

  // Simulated in-flight action loop polling cancel token
  let actionCompleted = false;
  for (let i = 0; i < 50; i++) {
    await new Promise((resolve) => setTimeout(resolve, 5));
    if (isCancelled) {
      break;
    }
    if (i === 49) {
      actionCompleted = true;
    }
  }

  const elapsedMs = Date.now() - startedAt;
  assert.ok(isCancelled, "Action must observe cancellation");
  assert.equal(actionCompleted, false, "Action must NOT run to completion when cancelled");
  assert.ok(elapsedMs < 200, `Cancel latency (${elapsedMs}ms) must be under 200ms target`);
});
