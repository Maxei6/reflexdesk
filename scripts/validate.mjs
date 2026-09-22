import { readFile } from "node:fs/promises";
import { accessSync } from "node:fs";

const jsonFiles = [
  "package.json",
  "src-tauri/tauri.conf.json",
  "src-tauri/capabilities/default.json",
  "config/defaults.json",
  "config/harnesses.json",
  "models/registry.json",
];

for (const path of jsonFiles) JSON.parse(await readFile(path, "utf8"));

const required = [
  "index.html",
  "overlay.html",
  "src/main.js",
  "src/overlay.js",
  "src-tauri/src/lib.rs",
  "src-tauri/src/stt.rs",
  "docs/P0.md",
  "public/audio-worklet.js",
  "src-tauri/src/tray.rs",
  "src-tauri/src/process_supervisor.rs",
  "src-tauri/src/settings.rs",
  "src-tauri/src/lifecycle.rs",
  "scripts/prepare-stt-runtime.mjs",
  "docs/STT.md",
  "assets/reflexdesk-hero.png",
  "README.md",
  "assets/reflexdesk-icon.svg",
];
for (const path of required) accessSync(path);

console.log("ReflexDesk static validation passed");
