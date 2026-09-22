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
  "README.md",
  "assets/reflexdesk-icon.svg",
];
for (const path of required) accessSync(path);

console.log("ReflexDesk static validation passed");
