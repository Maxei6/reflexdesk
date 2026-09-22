#!/usr/bin/env node

/**
 * scripts/release-preflight.mjs
 *
 * Preflight verification for ReflexDesk releases.
 *
 * Validates that all required code-signing certificates, notarization credentials,
 * and updater keys are configured before building production installers or
 * publishing releases.
 *
 * POLICY ENFORCEMENT:
 * - Preview builds (--channel=preview or --allow-preview): Unsigned builds permitted.
 * - Stable/Beta releases (--channel=stable or --channel=beta): FAILS CLOSED (exit 1)
 *   if any required external credential or updater key is missing.
 *
 * Zero-secret invariant: This script NEVER commits or outputs secret values;
 * it only verifies their presence and structural validity.
 */

import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT_DIR = resolve(__dirname, "..");
const TAURI_CONF_PATH = resolve(ROOT_DIR, "src-tauri", "tauri.conf.json");

export function parseArgs(args = process.argv.slice(2)) {
  const options = {
    channel: "stable",
    json: false,
    strict: true,
    platform: "all",
    allowPreview: false,
  };

  for (const arg of args) {
    if (arg === "--json") {
      options.json = true;
    } else if (arg === "--allow-preview") {
      options.allowPreview = true;
    } else if (arg.startsWith("--channel=")) {
      options.channel = arg.split("=")[1].toLowerCase().trim();
    } else if (arg.startsWith("--platform=")) {
      options.platform = arg.split("=")[1].toLowerCase().trim();
    } else if (arg === "--no-strict") {
      options.strict = false;
    }
  }

  return options;
}

export function evaluateReleasePreflight(options = {}, env = process.env) {
  const channel = options.channel || "stable";
  const platform = options.platform || "all";
  const allowPreview = options.allowPreview || channel === "preview";

  const checks = {
    windows: {
      required: platform === "all" || platform === "win32" || platform === "windows",
      items: [
        {
          id: "WINDOWS_CERTIFICATE",
          description: "Windows Authenticode code-signing certificate (PFX or base64)",
          present: Boolean(env.WINDOWS_CERTIFICATE && env.WINDOWS_CERTIFICATE.trim().length > 0),
          remediation: "Configure organization code-signing certificate in CI secrets or Azure Key Vault / DigiCert ONE.",
        },
        {
          id: "WINDOWS_CERTIFICATE_PASSWORD",
          description: "Password for Windows code-signing certificate",
          present: Boolean(env.WINDOWS_CERTIFICATE_PASSWORD && env.WINDOWS_CERTIFICATE_PASSWORD.trim().length > 0),
          remediation: "Set WINDOWS_CERTIFICATE_PASSWORD secret matching the certificate PFX.",
        },
      ],
    },
    macos: {
      required: platform === "all" || platform === "darwin" || platform === "macos",
      items: [
        {
          id: "APPLE_CERTIFICATE",
          description: "Apple Developer ID Application certificate (.p12 base64)",
          present: Boolean(env.APPLE_CERTIFICATE && env.APPLE_CERTIFICATE.trim().length > 0),
          remediation: "Export Developer ID Application cert from Apple Developer portal and set APPLE_CERTIFICATE secret.",
        },
        {
          id: "APPLE_CERTIFICATE_PASSWORD",
          description: "Password for Developer ID .p12 certificate",
          present: Boolean(env.APPLE_CERTIFICATE_PASSWORD && env.APPLE_CERTIFICATE_PASSWORD.trim().length > 0),
          remediation: "Set APPLE_CERTIFICATE_PASSWORD secret in CI repository settings.",
        },
        {
          id: "APPLE_SIGNING_IDENTITY",
          description: "Common name of Developer ID certificate (e.g. Developer ID Application: ...)",
          present: Boolean(env.APPLE_SIGNING_IDENTITY && env.APPLE_SIGNING_IDENTITY.trim().length > 0),
          remediation: "Set APPLE_SIGNING_IDENTITY matching the exact name on your Developer ID cert.",
        },
        {
          id: "APPLE_ID",
          description: "Apple ID username for notarytool notarization",
          present: Boolean(env.APPLE_ID && env.APPLE_ID.trim().length > 0),
          remediation: "Set APPLE_ID secret with account email associated with Apple Developer Program.",
        },
        {
          id: "APPLE_PASSWORD",
          description: "App-specific password for Apple ID notarytool submission",
          present: Boolean(
            (env.APPLE_PASSWORD && env.APPLE_PASSWORD.trim().length > 0) ||
            (env.APPLE_APP_SPECIFIC_PASSWORD && env.APPLE_APP_SPECIFIC_PASSWORD.trim().length > 0)
          ),
          remediation: "Generate an app-specific password at appleid.apple.com and set APPLE_PASSWORD secret.",
        },
        {
          id: "APPLE_TEAM_ID",
          description: "10-character Apple Developer Team ID",
          present: Boolean(env.APPLE_TEAM_ID && env.APPLE_TEAM_ID.trim().length > 0),
          remediation: "Find Team ID in Apple Developer account membership and set APPLE_TEAM_ID secret.",
        },
      ],
    },
    updater: {
      required: true,
      items: [
        {
          id: "TAURI_SIGNING_PRIVATE_KEY",
          description: "Tauri updater private key for signing release manifests and binaries",
          present: Boolean(env.TAURI_SIGNING_PRIVATE_KEY && env.TAURI_SIGNING_PRIVATE_KEY.trim().length > 0),
          remediation: "Generate via `tauri signer generate` and store ONLY in CI secret TAURI_SIGNING_PRIVATE_KEY.",
        },
        {
          id: "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
          description: "Password protecting the Tauri updater private key",
          present: Boolean(
            env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD !== undefined &&
            env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD !== null
          ),
          remediation: "Set TAURI_SIGNING_PRIVATE_KEY_PASSWORD secret if key was generated with password.",
        },
      ],
    },
  };

  // Inspect tauri.conf.json for embedded public key and updater endpoint
  let tauriConf = null;
  let pubkeyInConf = "";
  let endpointsInConf = [];

  if (existsSync(TAURI_CONF_PATH)) {
    try {
      tauriConf = JSON.parse(readFileSync(TAURI_CONF_PATH, "utf8"));
      pubkeyInConf = tauriConf?.plugins?.updater?.pubkey || "";
      endpointsInConf = tauriConf?.plugins?.updater?.endpoints || [];
    } catch {
      // JSON parse error handled below
    }
  }

  const embeddedPubkey = Boolean(
    (pubkeyInConf && pubkeyInConf.trim().length > 0) ||
    (env.TAURI_UPDATER_PUBLIC_KEY && env.TAURI_UPDATER_PUBLIC_KEY.trim().length > 0)
  );

  checks.updater.items.push({
    id: "TAURI_UPDATER_PUBLIC_KEY",
    description: "Public key embedded in application config or injected during CI build",
    present: embeddedPubkey,
    remediation: "Embed public key in src-tauri/tauri.conf.json (plugins.updater.pubkey) or inject via TAURI_UPDATER_PUBLIC_KEY.",
  });

  const validEndpoint = Boolean(
    (endpointsInConf.length > 0 &&
      endpointsInConf.some((ep) => typeof ep === "string" && ep.startsWith("https://"))) ||
    (env.UPDATER_FEED_ENDPOINT && env.UPDATER_FEED_ENDPOINT.trim().startsWith("https://"))
  );

  checks.updater.items.push({
    id: "UPDATER_FEED_ENDPOINT",
    description: "HTTPS updater feed endpoint configured in tauri.conf.json",
    present: validEndpoint,
    remediation: "Set https:// release feed in src-tauri/tauri.conf.json plugins.updater.endpoints.",
  });

  const missing = [];
  const satisfied = [];

  for (const [category, catData] of Object.entries(checks)) {
    if (!catData.required) continue;
    for (const item of catData.items) {
      if (item.present) {
        satisfied.push({ category, ...item });
      } else {
        missing.push({ category, ...item });
      }
    }
  }

  const isProductionChannel = channel === "stable" || channel === "beta";
  const ok = allowPreview ? true : (isProductionChannel ? missing.length === 0 : true);

  return {
    ok,
    channel,
    platform,
    allowPreview,
    isProductionChannel,
    totalChecks: missing.length + satisfied.length,
    missingCount: missing.length,
    satisfiedCount: satisfied.length,
    missing,
    satisfied,
    summary: ok
      ? (allowPreview
          ? `[PREVIEW] Unsigned preview build allowed for channel '${channel}'.`
          : `[PASSED] All ${satisfied.length} release credentials and updater configs verified for channel '${channel}'.`)
      : `[BLOCKED] Release preflight failed for channel '${channel}': ${missing.length} credentials/configs missing.`,
  };
}

export function runCli() {
  const options = parseArgs();
  const result = evaluateReleasePreflight(options);

  if (options.json) {
    console.log(JSON.stringify(result, null, 2));
  } else {
    console.log("===============================================================");
    console.log(" ReflexDesk Release Preflight Verification");
    console.log(` Target Channel : ${result.channel}`);
    console.log(` Target Platform: ${result.platform}`);
    console.log("===============================================================");

    if (result.ok) {
      console.log(`\n${result.summary}\n`);
      for (const item of result.satisfied) {
        console.log(`  [OK] ${item.id}: ${item.description}`);
      }
      console.log("\nRelease preflight completed successfully.");
    } else {
      console.error(`\n${result.summary}\n`);
      console.error("The following required credentials are missing:\n");
      for (const item of result.missing) {
        console.error(`  [MISSING] ${item.id} (${item.category.toUpperCase()})`);
        console.error(`            Description: ${item.description}`);
        console.error(`            Remediation: ${item.remediation}\n`);
      }
      console.error("CRITICAL: Stable/Beta releases fail closed without authentic signing credentials.");
      console.error("See docs/RELEASE_SECURITY.md and docs/plans/17_SIGNING_UPDATER_RELEASES.md for details.");
    }
  }

  if (!result.ok) {
    process.exit(1);
  }
}

// Execute CLI when run directly
if (process.argv[1] && process.argv[1].endsWith("release-preflight.mjs")) {
  runCli();
}
