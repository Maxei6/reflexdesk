import { createHash } from "node:crypto";
import { chmod, cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { existsSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

const VERSION = "v0.8.35";
const BASE = "https://github.com/CrispStrobe/CrispASR/releases/download/" + VERSION;

const TARGETS = {
  "win32:x64": [
    {
      archive: "crispasr-windows-x86_64-cpu.zip",
      sha256: "fb812ef200ffcf7de55e9be900a39a440c83a3facb09d7df9020d5a3b56ff280",
      binary: "crispasr.exe",
      outputDir: "crispasr",
      format: "zip",
    },
    {
      archive: "crispasr-windows-x86_64-cpu-legacy.zip",
      sha256: "954abf152b522c5a4cf14f289e3a243de5ad8b76dc8e771912c9d0f352d62506",
      binary: "crispasr.exe",
      outputDir: "crispasr-legacy",
      format: "zip",
    },
  ],
  "linux:x64": [
    {
      archive: "crispasr-linux-x86_64.tar.gz",
      sha256: "2a9982f69c8ee714cab81d8697ef8fb76878ee76eeb878cf65ddce1e443f9ff9",
      binary: "crispasr",
      outputDir: "crispasr",
      format: "tar",
    },
    {
      archive: "crispasr-linux-x86_64-cpu-legacy.tar.gz",
      sha256: "37e5a1adf91b06c400a9da391cfa05aeec9e2aa7e393677c2227b1043a619c8d",
      binary: "crispasr",
      outputDir: "crispasr-legacy",
      format: "tar",
    },
  ],
  "linux:arm64": [
    {
      archive: "crispasr-linux-arm64.tar.gz",
      sha256: "5b1ae7df881d09bed899ad67e8cabb7241f5664c891a64071a952fb742747238",
      binary: "crispasr",
      outputDir: "crispasr",
      format: "tar",
    },
  ],
  "darwin:x64": [
    {
      archive: "crispasr-macos.tar.gz",
      sha256: "0fa1aca0f102c2ea428a357eef143843a19bc122020bfa71cc3b4f984d8ecdf8",
      binary: "crispasr",
      outputDir: "crispasr",
      format: "tar",
    },
  ],
  "darwin:arm64": [
    {
      archive: "crispasr-macos.tar.gz",
      sha256: "0fa1aca0f102c2ea428a357eef143843a19bc122020bfa71cc3b4f984d8ecdf8",
      binary: "crispasr",
      outputDir: "crispasr",
      format: "tar",
    },
  ],
};

const platformKey = process.platform + ":" + process.arch;
const targets = TARGETS[platformKey];
if (!targets) throw new Error("No pinned CrispASR runtime for " + platformKey);

const resources = path.resolve("src-tauri/resources");
const metadataPath = path.join(resources, "crispasr-runtime.json");
await mkdir(resources, { recursive: true });

function findFile(name, dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      const nested = findFile(name, full);
      if (nested) return nested;
    } else if (entry.name.toLowerCase() === name.toLowerCase()) {
      return full;
    }
  }
  return null;
}

async function verifyExisting() {
  if (!existsSync(metadataPath)) return false;

  try {
    const meta = JSON.parse(await readFile(metadataPath, "utf8"));
    if (meta.version !== VERSION || meta.platform !== platformKey) return false;

    return targets.every(function (target) {
      const binaryPath = path.join(resources, target.outputDir, target.binary);
      return existsSync(binaryPath)
        && meta.assets.some(function (asset) {
          return asset.archive === target.archive
            && asset.sha256 === target.sha256
            && asset.outputDir === target.outputDir;
        });
    });
  } catch {
    return false;
  }
}

async function extractArchive(target, archivePath, extractDir) {
  if (target.format === "zip") {
    const safeArchive = archivePath.replaceAll("'", "''");
    const safeOut = extractDir.replaceAll("'", "''");
    const command = "Expand-Archive -LiteralPath '" + safeArchive
      + "' -DestinationPath '" + safeOut + "' -Force";
    const result = spawnSync(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-Command", command],
      { stdio: "inherit" },
    );
    if (result.status !== 0) throw new Error("PowerShell failed to extract CrispASR");
    return;
  }

  const result = spawnSync("tar", ["-xzf", archivePath, "-C", extractDir], {
    stdio: "inherit",
  });
  if (result.status !== 0) throw new Error("tar failed to extract CrispASR");
}

async function downloadAndPrepare(target) {
  const temp = await mkdtemp(path.join(tmpdir(), "reflexdesk-crispasr-"));
  const archivePath = path.join(temp, target.archive);
  const extractDir = path.join(temp, "extract");
  await mkdir(extractDir, { recursive: true });

  try {
    console.log("Downloading CrispASR " + VERSION + " (" + target.archive + ")...");
    const response = await fetch(BASE + "/" + target.archive);
    if (!response.ok) throw new Error("CrispASR download failed: HTTP " + response.status);

    const bytes = Buffer.from(await response.arrayBuffer());
    const digest = createHash("sha256").update(bytes).digest("hex");
    if (digest !== target.sha256) {
      throw new Error(
        "CrispASR checksum mismatch for " + target.archive
          + ": expected " + target.sha256 + ", got " + digest,
      );
    }

    await writeFile(archivePath, bytes);
    await extractArchive(target, archivePath, extractDir);

    const binary = findFile(target.binary, extractDir);
    if (!binary) throw new Error(target.binary + " not found in " + target.archive);

    const sourceRuntimeDir = path.dirname(binary);
    const destinationDir = path.join(resources, target.outputDir);
    await rm(destinationDir, { recursive: true, force: true });
    await cp(sourceRuntimeDir, destinationDir, { recursive: true });

    const destinationBinary = path.join(destinationDir, target.binary);
    if (process.platform !== "win32") await chmod(destinationBinary, 0o755);

    const smoke = spawnSync(destinationBinary, ["--version"], {
      cwd: destinationDir,
      encoding: "utf8",
      env: process.env,
    });
    if (smoke.status !== 0) {
      throw new Error(
        "Prepared CrispASR runtime failed --version: "
          + String(smoke.stderr || smoke.stdout || "unknown error"),
      );
    }

    console.log("Prepared " + target.outputDir + " from " + target.archive);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}

if (await verifyExisting()) {
  console.log("CrispASR " + VERSION + " runtime already prepared for " + platformKey);
  process.exit(0);
}

for (const target of targets) await downloadAndPrepare(target);

await writeFile(
  metadataPath,
  JSON.stringify(
    {
      runtime: "CrispASR",
      version: VERSION,
      platform: platformKey,
      source_base: BASE,
      assets: targets.map(function (target) {
        return {
          archive: target.archive,
          sha256: target.sha256,
          outputDir: target.outputDir,
          binary: target.binary,
        };
      }),
      stt_model: "nvidia/nemotron-3.5-asr-streaming-0.6b",
      model_runtime: "cstr/nemotron-3.5-asr-streaming-GGUF / Q4_K",
      model_download: "first voice use; cached by CrispASR",
    },
    null,
    2,
  ) + "\n",
);

console.log("Prepared pinned CrispASR runtime for " + platformKey);
