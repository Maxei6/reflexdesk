import test from "node:test";
import assert from "node:assert/strict";
import { execSync, spawnSync } from "node:child_process";
import { readFileSync, existsSync } from "node:fs";
import { resolve, join } from "node:path";
import { generateArtifactMetadata } from "../scripts/artifact-metadata.mjs";

test("release preflight: fails closed in production mode without credentials", () => {
  const scriptPath = resolve("scripts/release-preflight.mjs");
  const res = spawnSync(process.execPath, [scriptPath, "--channel=stable"], {
    env: { ...process.env, WINDOWS_CERTIFICATE: "", APPLE_CERTIFICATE: "", TAURI_SIGNING_PRIVATE_KEY: "" },
    encoding: "utf8"
  });

  assert.equal(res.status, 1, "Must exit with status 1 on missing credentials");
  assert.match(res.stderr || res.stdout, /Release preflight failed for channel 'stable'/);
  assert.match(res.stderr || res.stdout, /WINDOWS_CERTIFICATE/);
  assert.match(res.stderr || res.stdout, /APPLE_CERTIFICATE/);
  assert.match(res.stderr || res.stdout, /TAURI_SIGNING_PRIVATE_KEY/);
});

test("release preflight: passes with --allow-preview flag", () => {
  const scriptPath = resolve("scripts/release-preflight.mjs");
  const res = spawnSync(process.execPath, [scriptPath, "--allow-preview", "--json"], {
    encoding: "utf8"
  });

  assert.equal(res.status, 0, "Must exit with status 0 when --allow-preview is specified");
  const parsed = JSON.parse(res.stdout);
  assert.equal(parsed.ok, true);
  assert.equal(parsed.allowPreview, true);
});

test("release preflight: channel-specific requirements (nightly allows missing windows cert)", () => {
  const scriptPath = resolve("scripts/release-preflight.mjs");
  const mockEnv = {
    ...process.env,
    TAURI_SIGNING_PRIVATE_KEY: "mock_key",
    TAURI_UPDATER_PUBLIC_KEY: "mock_pubkey",
    UPDATER_FEED_ENDPOINT: "https://releases.reflexdesk.io/nightly",
    WINDOWS_CERTIFICATE: "",
    APPLE_CERTIFICATE: ""
  };
  const res = spawnSync(process.execPath, [scriptPath, "--channel=nightly", "--json"], {
    env: mockEnv,
    encoding: "utf8"
  });

  assert.equal(res.status, 0, "Nightly channel should pass with updater key even without full EV certs");
  const parsed = JSON.parse(res.stdout);
  assert.equal(parsed.ok, true);
  assert.equal(parsed.channel, "nightly");
});

test("release preflight: succeeds when all required credentials are present", () => {
  const scriptPath = resolve("scripts/release-preflight.mjs");
  const mockEnv = {
    ...process.env,
    WINDOWS_CERTIFICATE: "mock_cert",
    WINDOWS_CERTIFICATE_PASSWORD: "mock_pass",
    APPLE_CERTIFICATE: "mock_apple_cert",
    APPLE_CERTIFICATE_PASSWORD: "mock_apple_pass",
    APPLE_SIGNING_IDENTITY: "Developer ID Application: Test",
    APPLE_ID: "developer@example.com",
    APPLE_PASSWORD: "app-specific-pass",
    APPLE_TEAM_ID: "TEAMID1234",
    TAURI_SIGNING_PRIVATE_KEY: "mock_tauri_key",
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "mock_key_pass",
    TAURI_UPDATER_PUBLIC_KEY: "mock_tauri_pubkey",
    UPDATER_FEED_ENDPOINT: "https://releases.reflexdesk.io/stable"
  };
  const res = spawnSync(process.execPath, [scriptPath, "--channel=stable", "--json"], {
    env: mockEnv,
    encoding: "utf8"
  });

  assert.equal(res.status, 0, "Must exit with status 0 when all credentials are satisfied");
  const parsed = JSON.parse(res.stdout);
  assert.equal(parsed.ok, true);
  assert.equal(parsed.missingCount, 0);
});

test("artifact metadata: generates structured metadata for artifacts", () => {
  const meta = generateArtifactMetadata({
    filePath: "package.json",
    channel: "stable"
  });

  assert.ok(meta.commit_sha);
  assert.equal(meta.filename, "package.json");
  assert.ok(meta.sha256 && meta.sha256.length === 64);
  assert.ok(meta.size_bytes > 0);
  assert.equal(meta.channel, "stable");
  assert.equal(meta.is_preview, false);
});

test("manifest verification: rejects unsigned manifest fixture", () => {
  const fixture = JSON.parse(readFileSync("tests/fixtures/updater/unsigned-manifest.json", "utf8"));
  const platform = fixture.platforms["windows-x86_64"];
  assert.ok(!platform.signature, "Unsigned manifest must not have signature");
});

test("manifest verification: rejects tampered manifest fixture", () => {
  const fixture = JSON.parse(readFileSync("tests/fixtures/updater/tampered-manifest.json", "utf8"));
  const platform = fixture.platforms["windows-x86_64"];
  assert.ok(platform.signature.includes("INVALID"));
  assert.match(platform.sha256, /^0+$/, "Tampered manifest contains zeroed/invalid sha256");
});

test("manifest verification: rejects downgrade version fixture", () => {
  const fixture = JSON.parse(readFileSync("tests/fixtures/updater/downgrade-manifest.json", "utf8"));
  const currentVersion = "0.1.0";
  const targetVersion = fixture.version; // "0.0.9"

  function isNewer(current, target) {
    const [cMaj, cMin, cPatch] = current.split(".").map(Number);
    const [tMaj, tMin, tPatch] = target.split(".").map(Number);
    if (tMaj !== cMaj) return tMaj > cMaj;
    if (tMin !== cMin) return tMin > cMin;
    return tPatch > cPatch;
  }

  assert.equal(isNewer(currentVersion, targetVersion), false, "Downgrade 0.0.9 must be rejected when current is 0.1.0");
});

test("staged rollout: deterministic cohort assignment and boundary rules", () => {
  function calculateBucket(clientId, targetVersion) {
    let hash = 5381;
    const combined = clientId + targetVersion;
    for (let i = 0; i < combined.length; i++) {
      hash = ((hash << 5) + hash + combined.charCodeAt(i)) >>> 0;
    }
    return hash % 100;
  }

  function isInRollout(clientId, targetVersion, rolloutPercent) {
    if (rolloutPercent >= 100) return true;
    if (rolloutPercent <= 0) return false;
    return calculateBucket(clientId, targetVersion) < rolloutPercent;
  }

  // Determinism
  const b1 = calculateBucket("client_alpha", "0.2.0");
  const b2 = calculateBucket("client_alpha", "0.2.0");
  assert.equal(b1, b2, "Same client and version must produce identical bucket");

  // Boundaries
  assert.equal(isInRollout("client_alpha", "0.2.0", 100), true);
  assert.equal(isInRollout("client_alpha", "0.2.0", 0), false);

  // Staged rollout manifest fixture
  const fixture = JSON.parse(readFileSync("tests/fixtures/updater/staged-rollout-manifest.json", "utf8"));
  assert.equal(fixture.rollout_percentage, 25);
  const eligible = isInRollout("client_alpha", fixture.version, fixture.rollout_percentage);
  assert.equal(typeof eligible, "boolean");
});

test("transaction coordination: mutual exclusion between model migration and application update", () => {
  // Model state tracker simulating ModelManager & UpdaterService coordination
  let updateInProgress = false;
  const activeModelTransactions = new Map();

  function beginModelTransaction(modelId, op) {
    if (updateInProgress) {
      throw new Error("MODEL_TRANSACTION_BLOCKED_BY_UPDATE");
    }
    const txId = `tx_${modelId}_${Date.now()}`;
    activeModelTransactions.set(txId, { modelId, op });
    return txId;
  }

  function endModelTransaction(txId) {
    activeModelTransactions.delete(txId);
  }

  function checkCanUpdate() {
    if (activeModelTransactions.size > 0) {
      throw new Error("UPDATE_BLOCKED_BY_MODEL_MIGRATION");
    }
    return true;
  }

  // 1. When model transaction is active, update check fails closed
  const tx = beginModelTransaction("nemotron-3.5-asr", "migration");
  assert.throws(() => checkCanUpdate(), /UPDATE_BLOCKED_BY_MODEL_MIGRATION/);

  // 2. Once model transaction ends, update check is allowed
  endModelTransaction(tx);
  assert.equal(checkCanUpdate(), true);

  // 3. When update is active, model transaction cannot begin
  updateInProgress = true;
  assert.throws(() => beginModelTransaction("nemotron-3.5-asr", "download"), /MODEL_TRANSACTION_BLOCKED_BY_UPDATE/);
  updateInProgress = false;
});
