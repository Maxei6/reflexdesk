import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

test("protocol.json defines version 1 with expected envelope and actions", () => {
  const protocolPath = path.resolve("extension/protocol.json");
  assert.ok(fs.existsSync(protocolPath), "extension/protocol.json must exist");

  const protocol = JSON.parse(fs.readFileSync(protocolPath, "utf-8"));
  assert.equal(protocol.version, 1);
  assert.ok(protocol.envelope);
  assert.deepEqual(protocol.envelope.required, ["v", "session", "action", "nonce"]);

  const expectedActions = [
    "browser.tabs",
    "browser.open",
    "browser.inspect",
    "browser.find",
    "browser.click",
    "browser.type",
    "browser.select",
    "browser.scroll",
    "browser.extract",
    "browser.wait",
    "browser.download",
    "browser.verify",
  ];

  for (const act of expectedActions) {
    assert.ok(protocol.actions[act], `Action ${act} must be defined in protocol.json`);
    assert.ok(protocol.actions[act].result, `Action ${act} must define result schema`);
  }
});

test("protocol.json specifies required error codes", () => {
  const protocol = JSON.parse(fs.readFileSync("extension/protocol.json", "utf-8"));
  const errorCodes = protocol.errorCodes;
  assert.ok(errorCodes);

  const requiredCodes = [
    "stale-ref",
    "spa-mutation",
    "blocked",
    "captcha",
    "auth-required",
    "element-not-found",
    "action-timeout",
    "bridge-unavailable",
    "pairing-rejected"
  ];

  for (const code of requiredCodes) {
    assert.ok(errorCodes[code], `Error code '${code}' must be defined`);
    assert.equal(typeof errorCodes[code].recoverable, "boolean");
    assert.ok(errorCodes[code].description);
  }
});

test("protocol.json defines semantic compression and sensitive redaction rules", () => {
  const protocol = JSON.parse(fs.readFileSync("extension/protocol.json", "utf-8"));
  const compression = protocol.compression;
  assert.ok(compression);
  assert.ok(Array.isArray(compression.skipTags));
  assert.ok(compression.skipTags.includes("script"));
  assert.ok(compression.skipTags.includes("style"));
  assert.ok(compression.redaction);
  assert.ok(Array.isArray(compression.redaction.rules));
  assert.equal(compression.redaction.mask, "[REDACTED]");
});

test("Chrome MV3 extension files exist and manifest is valid", () => {
  const manifestPath = "extension/chrome/manifest.json";
  assert.ok(fs.existsSync(manifestPath), "manifest.json must exist");

  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf-8"));
  assert.equal(manifest.manifest_version, 3);
  assert.ok(manifest.permissions.includes("debugger"));
  assert.ok(manifest.permissions.includes("tabs"));
  assert.ok(manifest.permissions.includes("storage"));

  assert.ok(fs.existsSync("extension/chrome/background.js"), "background.js must exist");
  assert.ok(fs.existsSync("extension/chrome/content.js"), "content.js must exist");
  assert.ok(fs.existsSync("extension/chrome/popup.html"), "popup.html must exist");
  assert.ok(fs.existsSync("extension/chrome/popup.js"), "popup.js must exist");
});
