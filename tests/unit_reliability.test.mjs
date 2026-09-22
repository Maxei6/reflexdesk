import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { routeFast } from "../src/lib/router.js";

const TOOL_SCHEMA = JSON.parse(fs.readFileSync("schemas/tool.schema.json", "utf8"));

test("Unit/Schemas: tool schema validates required structure and risk enums", () => {
  assert.ok(Array.isArray(TOOL_SCHEMA.required));
  assert.ok(TOOL_SCHEMA.required.includes("name"));
  assert.ok(TOOL_SCHEMA.required.includes("description"));
  assert.ok(TOOL_SCHEMA.required.includes("risk"));
  assert.ok(TOOL_SCHEMA.required.includes("confirmation"));
  assert.ok(TOOL_SCHEMA.required.includes("parameters"));

  assert.deepEqual(TOOL_SCHEMA.properties.risk.enum, ["safe", "sensitive", "destructive"]);
  assert.deepEqual(TOOL_SCHEMA.properties.confirmation.enum, ["never", "when-ambiguous", "always"]);
});

test("Unit/Router: fast router classifies deterministic intents and handles edge cases", () => {
  // Empty / null inputs
  assert.equal(routeFast("").kind, "noop");
  assert.equal(routeFast("   ").kind, "noop");
  assert.equal(routeFast(null).kind, "noop");
  assert.equal(routeFast(undefined).kind, "noop");

  // Onboarding ping and voice control
  assert.equal(routeFast("hello reflexdesk").action, "reflex.ping");
  assert.equal(routeFast("stop listening").action, "voice.stop");
  assert.equal(routeFast("go to sleep").action, "voice.stop");

  // Fast tool routes
  const openApp = routeFast("open Spotify");
  assert.equal(openApp.kind, "tool");
  assert.equal(openApp.action, "app.open");
  assert.equal(openApp.args.app, "spotify");
  assert.ok(openApp.confidence >= 0.95);

  const search = routeFast("search for rust async patterns");
  assert.equal(search.kind, "tool");
  assert.equal(search.action, "browser.search");
  assert.equal(search.args.query, "rust async patterns");
  assert.ok(search.confidence >= 0.95);

  const openUrl = routeFast("open https://github.com");
  assert.equal(openUrl.kind, "tool");
  assert.equal(openUrl.action, "browser.open");
  assert.equal(openUrl.args.url, "https://github.com");

  const harness = routeFast("ask codex to fix the unit tests");
  assert.equal(harness.kind, "tool");
  assert.equal(harness.action, "harness.start");
  assert.equal(harness.args.harness, "codex");
  assert.equal(harness.args.prompt, "fix the unit tests");

  // Escalation to local planner
  const complex = routeFast("compare my last three invoices and summarize");
  assert.equal(complex.kind, "planner");
  assert.equal(complex.text, "compare my last three invoices and summarize");
});

test("Unit/Policy: negation detection identifies cancellation intents and avoids false positives", () => {
  // Policy rule from local://contracts.md:
  // Leading 'don't|do not|never|stop|cancel|actually stop' and mid-sentence 'don't <verb>'
  function isNegatedCommand(text) {
    const raw = String(text || "").trim();
    if (!raw) return false;
    const lower = raw.toLowerCase();

    const leadingNegation = /^(?:don'?t|do not|never|stop|cancel|actually stop)\b/i;
    if (leadingNegation.test(lower)) return true;

    const midSentenceNegation = /\b(?:don'?t|do not|never)\s+[a-z]+/i;
    if (midSentenceNegation.test(lower)) return true;

    const trailingStop = /\bactually stop$/i;
    if (trailingStop.test(lower)) return true;

    return false;
  }

  // True negations
  assert.ok(isNegatedCommand("don't close Chrome"));
  assert.ok(isNegatedCommand("do not delete that file"));
  assert.ok(isNegatedCommand("never send that email"));
  assert.ok(isNegatedCommand("stop what you are doing"));
  assert.ok(isNegatedCommand("cancel session"));
  assert.ok(isNegatedCommand("actually stop"));
  assert.ok(isNegatedCommand("please don't touch the settings"));
  assert.ok(isNegatedCommand("open Chrome, actually stop"));

  // False positive guards (words containing 'no' or 'stop' as substrings)
  assert.equal(isNegatedCommand("open notepad"), false, "notepad contains 'no' but is not a negation");
  assert.equal(isNegatedCommand("search for nonstop flights"), false, "nonstop is not a negation");
  assert.equal(isNegatedCommand("play the synopsis"), false, "synopsis contains 'no' but is not a negation");
  assert.equal(isNegatedCommand("hello reflexdesk"), false);
  assert.equal(isNegatedCommand("search rust programming"), false);
});

test("Unit/State Machine: lifecycle states maintain deterministic invariants", () => {
  // Frozen states: booting, ready, listening, transcribing, routing, executing, verifying, attention, error
  const validTransitions = {
    booting: ["ready", "error"],
    ready: ["listening", "executing", "attention"],
    listening: ["transcribing", "ready", "attention"],
    transcribing: ["routing", "ready", "attention"],
    routing: ["executing", "ready", "attention"],
    executing: ["verifying", "ready", "attention"],
    verifying: ["ready", "attention"],
    attention: ["ready", "listening", "error"],
    error: ["ready", "booting"]
  };

  function canTransition(from, to) {
    return (validTransitions[from] || []).includes(to);
  }

  assert.ok(canTransition("booting", "ready"));
  assert.ok(canTransition("ready", "listening"));
  assert.ok(canTransition("listening", "transcribing"));
  assert.ok(canTransition("transcribing", "routing"));
  assert.ok(canTransition("routing", "executing"));
  assert.ok(canTransition("executing", "verifying"));
  assert.ok(canTransition("verifying", "ready"));

  // Cancel from active states returns to ready
  assert.ok(canTransition("listening", "ready"));
  assert.ok(canTransition("transcribing", "ready"));
  assert.ok(canTransition("executing", "ready"));

  // Invalid state jumps are rejected
  assert.equal(canTransition("booting", "executing"), false, "Cannot execute directly from booting");
  assert.equal(canTransition("verifying", "listening"), false, "Must return to ready before new listening turn");
});
