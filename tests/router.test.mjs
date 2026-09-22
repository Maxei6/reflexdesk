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
