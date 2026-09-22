import { createHash } from "node:crypto";
import { chmod, cp, mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { existsSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

const manifestPath = path.resolve("models/registry.json");
const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const crispRuntime = manifest.runtimes?.crispasr;
if (!crispRuntime) {
  throw new Error("models/registry.json missing canonical runtimes.crispasr manifest entry");
}

const VERSION = crispRuntime.version;
const BASE = crispRuntime.base_url;
const TARGETS = crispRuntime.targets;

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
  const stagingArchive = path.join(temp, target.archive + ".staging");
  const archivePath = path.join(temp, target.archive);
  const extractDir = path.join(temp, "extract");
  await mkdir(extractDir, { recursive: true });

  try {
    console.log("Downloading CrispASR " + VERSION + " (" + target.archive + ")...");
    const response = await fetch(BASE + "/" + target.archive);
    if (!response.ok) throw new Error("CrispASR download failed: HTTP " + response.status);

    const bytes = Buffer.from(await response.arrayBuffer());
    await writeFile(stagingArchive, bytes);

    // Rehash from disk to verify integrity of written archive
    const diskBytes = await readFile(stagingArchive);
    const digest = createHash("sha256").update(diskBytes).digest("hex");
    if (digest !== target.sha256) {
      throw new Error(
        "CrispASR checksum mismatch for " + target.archive
          + ": expected " + target.sha256 + ", got " + digest,
      );
    }

    // Atomic rename of staged archive
    await rename(stagingArchive, archivePath);
    await extractArchive(target, archivePath, extractDir);

    const binary = findFile(target.binary, extractDir);
    if (!binary) throw new Error(target.binary + " not found in " + target.archive);

    const sourceRuntimeDir = path.dirname(binary);
    const stagingDir = path.join(resources, target.outputDir + ".staging");
    const destinationDir = path.join(resources, target.outputDir);

    // Stage runtime directory
    await rm(stagingDir, { recursive: true, force: true });
    await cp(sourceRuntimeDir, stagingDir, { recursive: true });

    const stagingBinary = path.join(stagingDir, target.binary);
    if (process.platform !== "win32") await chmod(stagingBinary, 0o755);

    const smoke = spawnSync(stagingBinary, ["--version"], {
      cwd: stagingDir,
      encoding: "utf8",
      env: process.env,
    });
    if (smoke.status !== 0) {
      await rm(stagingDir, { recursive: true, force: true });
      throw new Error(
        "Prepared CrispASR runtime failed --version: "
          + String(smoke.stderr || smoke.stdout || "unknown error"),
      );
    }

    // Atomic activation with backup rollback
    const backupDir = path.join(resources, target.outputDir + ".old");
    await rm(backupDir, { recursive: true, force: true });
    if (existsSync(destinationDir)) {
      try {
        await rename(destinationDir, backupDir);
      } catch {
        await rm(destinationDir, { recursive: true, force: true });
      }
    }

    try {
      await rename(stagingDir, destinationDir);
      await rm(backupDir, { recursive: true, force: true });
    } catch (renameErr) {
      if (existsSync(backupDir) && !existsSync(destinationDir)) {
        try { await rename(backupDir, destinationDir); } catch {}
      }
      throw renameErr;
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

const tempMetadataPath = metadataPath + ".tmp";
await writeFile(
  tempMetadataPath,
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
      model_download: "managed by ModelManager; verified and cached",
    },
    null,
    2,
  ) + "\n",
);
await rename(tempMetadataPath, metadataPath);

console.log("Prepared pinned CrispASR runtime for " + platformKey);
