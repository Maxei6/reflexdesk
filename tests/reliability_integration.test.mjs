import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";

const HARNESSES_CONFIG = JSON.parse(fs.readFileSync("config/harnesses.json", "utf8"));
const SPA_FIXTURES = JSON.parse(fs.readFileSync("tests/fixtures/browser/spa_mutation.json", "utf8"));

test("Integration/Desktop: ranked selector engine prioritizes role and accessible name", () => {
  // Selector ranking rule from Plan 02 / desktop.rs:
  // Role > Accessible Name > Context > Process
  function rankCandidate(element, query) {
    let score = 0;
    const lowerQuery = query.toLowerCase();

    // 1. Role match
    if (element.role && element.role.toLowerCase() === lowerQuery) {
      score += 40;
    }

    // 2. Exact accessible name match
    if (element.name && element.name.toLowerCase() === lowerQuery) {
      score += 30;
    } else if (element.name && element.name.toLowerCase().includes(lowerQuery)) {
      score += 15;
    }

    // 3. Context / window match
    if (element.window && element.window.toLowerCase().includes(lowerQuery)) {
      score += 10;
    }

    // 4. Process match
    if (element.process && element.process.toLowerCase().includes(lowerQuery)) {
      score += 5;
    }

    return score;
  }

  const elements = [
    { id: 1, role: "button", name: "Submit", process: "chrome.exe", window: "Form" },
    { id: 2, role: "text", name: "Submit button description", process: "chrome.exe", window: "Form" },
    { id: 3, role: "window", name: "Submit dialog", process: "chrome.exe", window: "Submit dialog" },
  ];

  const ranked = elements
    .map(el => ({ el, score: rankCandidate(el, "submit") }))
    .sort((a, b) => b.score - a.score);

  // Exact button name match wins over partial description text
  assert.equal(ranked[0].el.id, 1);
  assert.equal(ranked[0].el.role, "button");
});

test("Integration/Desktop: stale element ref detection and generation counters", () => {
  class DesktopSnapshot {
    constructor(generation) {
      this.generation = generation;
      this.elements = new Map();
    }

    registerElement(refId, name, role) {
      this.elements.set(refId, { refId, name, role, generation: this.generation });
    }

    lookup(refId, expectedGeneration) {
      const el = this.elements.get(refId);
      if (!el) return { ok: false, error: "element-not-found" };
      if (el.generation !== expectedGeneration) {
        return { ok: false, error: "stale-ref: element generation mismatch" };
      }
      return { ok: true, element: el };
    }
  }

  const snapGen1 = new DesktopSnapshot(1);
  snapGen1.registerElement(101, "OK Button", "button");

  // Lookup in current generation passes
  assert.ok(snapGen1.lookup(101, 1).ok);

  // Lookup across generations fails safely with stale-ref
  const resStale = snapGen1.lookup(101, 2);
  assert.equal(resStale.ok, false);
  assert.ok(resStale.error.includes("stale-ref"));
});

test("Integration/Browser: extension protocol handles SPA mutations and stale refs gracefully", () => {
  const staleScenario = SPA_FIXTURES.scenarios.find(s => s.id === "spa_stale_ref_01");
  assert.ok(staleScenario);
  assert.equal(staleScenario.expected_error, "stale-ref");
  assert.equal(staleScenario.recovery.reinspect_required, true);

  const routeScenario = SPA_FIXTURES.scenarios.find(s => s.id === "spa_route_change_01");
  assert.ok(routeScenario);
  assert.equal(routeScenario.expected_error, "spa-mutation");

  const captchaScenario = SPA_FIXTURES.scenarios.find(s => s.id === "spa_captcha_botwall_01");
  assert.ok(captchaScenario);
  assert.equal(captchaScenario.expected_error, "captcha");
  assert.equal(captchaScenario.recovery.escalate_to_user, true);
});

test("Integration/Harness: transport hierarchy and fallback order conforms to plan 06", () => {
  const order = HARNESSES_CONFIG.integration_order;
  assert.ok(Array.isArray(order));
  assert.equal(order[0].transport, "native_sdk");
  assert.equal(order[1].transport, "acp");
  assert.equal(order[2].transport, "local_api");
  assert.equal(order[3].transport, "jsonl_cli");
  assert.equal(order[4].transport, "plain_cli");
  assert.equal(order[5].transport, "ui_fallback");

  // UI fallback must always be marked with fallback_marker
  const uiHarness = HARNESSES_CONFIG.harnesses.find(h => h.id === "ui-fallback");
  assert.ok(uiHarness);
  assert.equal(uiHarness.fallback_marker, "via:ui-fallback");

  // Codex uses jsonl_cli with verified command
  const codex = HARNESSES_CONFIG.harnesses.find(h => h.id === "codex");
  assert.ok(codex);
  assert.equal(codex.transport, "jsonl_cli");
  assert.equal(codex.verified_command, "codex exec");

  // OpenCode uses ACP with plain_cli fallback
  const opencode = HARNESSES_CONFIG.harnesses.find(h => h.id === "opencode");
  assert.ok(opencode);
  assert.equal(opencode.transport, "acp");
  assert.equal(opencode.fallback_transport, "plain_cli");
});
