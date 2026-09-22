import { readFile } from "node:fs/promises";
import { accessSync } from "node:fs";

const jsonFiles = [
  "package.json",
  "src-tauri/tauri.conf.json",
  "src-tauri/capabilities/main.json",
  "src-tauri/capabilities/overlay.json",
  "config/defaults.json",
  "config/harnesses.json",
  "models/registry.json",
  "tests/matrix.json",
  "schemas/skill.schema.json",
  "tests/soak/thresholds.json",
  "tests/soak/matrix.json",
  "tests/soak/blockers.json",
];

for (const path of jsonFiles) JSON.parse(await readFile(path, "utf8"));

const required = [
  "index.html",
  "overlay.html",
  "src/main.js",
  "src/overlay.js",
  "src-tauri/src/lib.rs",
  "src-tauri/src/stt.rs",
  "src-tauri/src/policy.rs",
  "src-tauri/src/redaction.rs",
  "src-tauri/src/security.rs",
  "src-tauri/src/transcript.rs",
  "src-tauri/src/model_manager.rs",
  "src-tauri/src/process/mod.rs",
  "src-tauri/permissions/reflexdesk.toml",
  "src-tauri/build.rs",
  "docs/P0.md",
  "docs/plans/README.md",
  "docs/plans/00_MASTER_ROADMAP.md",
  "docs/RELEASE_SECURITY.md",
  "docs/THREAT_MODEL.md",
  "public/audio-worklet.js",
  "src-tauri/src/tray.rs",
  "src-tauri/src/process_supervisor.rs",
  "src-tauri/src/settings.rs",
  "src-tauri/src/secrets.rs",
  "src-tauri/src/lifecycle.rs",
  "src-tauri/src/skills.rs",
  "scripts/prepare-stt-runtime.mjs",
  "docs/STT.md",
  "assets/reflexdesk-hero.png",
  "README.md",
  "assets/reflexdesk-icon.svg",
  "tests/soak/thresholds.json",
  "tests/soak/matrix.json",
  "tests/soak/blockers.json",
  "scripts/soak-collect.mjs",
  "scripts/soak-entry-check.mjs",
  ".github/workflows/soak.yml",
];
for (const path of required) accessSync(path);

console.log("ReflexDesk static validation passed");
