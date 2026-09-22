import test from "node:test";
import assert from "node:assert/strict";
import { routeFast } from "../src/lib/router.js";

test("routes common app open locally", () => {
  assert.deepEqual(routeFast("open spotify"), {
    kind: "tool",
    action: "app.open",
    args: { app: "spotify" },
    confidence: 0.97,
  });
});

test("routes browser search", () => {
  const route = routeFast("search for local AI models");
  assert.equal(route.action, "browser.search");
  assert.equal(route.args.query, "local AI models");
});

test("unknown command escalates", () => {
  assert.equal(routeFast("compare my last three invoices").kind, "planner");
});

test("routes onboarding voice proof deterministically", () => {
  assert.deepEqual(routeFast("hello reflexdesk"), {
    kind: "control",
    action: "reflex.ping",
    args: {},
    confidence: 0.99,
  });
});

test("routes stop listening without planner", () => {
  const route = routeFast("stop listening");
  assert.equal(route.kind, "control");
  assert.equal(route.action, "voice.stop");
});
