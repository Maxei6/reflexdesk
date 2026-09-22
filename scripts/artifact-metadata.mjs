import { execSync } from "node:child_process";
import { existsSync, statSync, readFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { createHash } from "node:crypto";

/**
 * Generates structured artifact metadata for a built binary or installer.
 * Format: { commit_sha, platform, arch, channel, sha256, filename, size_bytes, built_at, signature_present }
 */
export function generateArtifactMetadata({
  filePath,
  channel = "stable",
  platform = process.platform,
  arch = process.arch,
  commitSha = null
} = {}) {
  let resolvedSha = commitSha;
  if (!resolvedSha) {
    try {
      resolvedSha = execSync("git rev-parse HEAD", { encoding: "utf8" }).trim();
    } catch {
      resolvedSha = "unknown";
    }
  }

  let sha256 = null;
  let sizeBytes = 0;
  let filename = "unknown";

  if (filePath && existsSync(filePath)) {
    const data = readFileSync(filePath);
    sha256 = createHash("sha256").update(data).digest("hex");
    sizeBytes = statSync(filePath).size;
    filename = filePath.split(/[/\\]/).pop();
  }

  return {
    commit_sha: resolvedSha,
    platform: normalizePlatform(platform),
    arch: normalizeArch(arch),
    channel: normalizeChannel(channel),
    sha256,
    filename,
    size_bytes: sizeBytes,
    built_at: new Date().toISOString(),
    is_preview: channel !== "stable" && channel !== "beta"
  };
}

function normalizePlatform(p) {
  if (p === "win32" || p === "windows") return "windows";
  if (p === "darwin" || p === "macos") return "macos";
  if (p === "linux") return "linux";
  return p;
}

function normalizeArch(a) {
  if (a === "x64" || a === "x86_64") return "x86_64";
  if (a === "arm64" || a === "aarch64") return "aarch64";
  return a;
}

function normalizeChannel(c) {
  const lower = String(c).toLowerCase();
  if (lower === "nightly" || lower === "beta" || lower === "stable") return lower;
  return "preview";
}

// CLI entrypoint if run directly
if (process.argv[1] && resolve(process.argv[1]) === resolve(new URL(import.meta.url).pathname.replace(/^\/([A-Z]:)/, "$1"))) {
  const args = process.argv.slice(2);
  let filePath = null;
  let channel = "stable";

  for (const arg of args) {
    if (arg.startsWith("--file=")) filePath = arg.split("=")[1];
    else if (arg.startsWith("--channel=")) channel = arg.split("=")[1];
    else if (!arg.startsWith("--")) filePath = arg;
  }

  const meta = generateArtifactMetadata({ filePath, channel });
  console.log(JSON.stringify(meta, null, 2));
}
