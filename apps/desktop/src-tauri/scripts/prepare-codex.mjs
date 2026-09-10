import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmod,
  mkdir,
  readFile,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const version = "0.154.0";
const root = fileURLToPath(new URL("../", import.meta.url));
const resources = path.join(root, "resources/codex");
const binary = path.join(root, "binaries/loofah-codex-aarch64-apple-darwin");
const binaryHash =
  "344310a0a591c1b192e04feff304321a69907c9498baaac331ca7e16ebcef9d7";
const catalogHash =
  "f3b8104396daf6381bed9d7c4b154a8664f9b5089b44b01a9d54f421566ec9a7";
const release = `https://github.com/openai/codex/releases/download/rust-v${version}`;
const source = `https://raw.githubusercontent.com/openai/codex/rust-v${version}`;

if (process.platform !== "darwin" || process.arch !== "arm64") {
  throw new Error("The bundled ChatGPT runtime requires Apple Silicon macOS.");
}

async function download(url, sha256) {
  const response = await fetch(url);
  if (!response.ok)
    throw new Error(`Codex download failed: ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (sha256 && createHash("sha256").update(bytes).digest("hex") !== sha256) {
    throw new Error("Codex download checksum mismatch");
  }
  return bytes;
}

await mkdir(resources, { recursive: true });
await mkdir(path.dirname(binary), { recursive: true });
const stamp = `${version}:${binaryHash}:${catalogHash}:1`;
const stampPath = path.join(resources, "version");
if ((await readFile(stampPath, "utf8").catch(() => "")) === stamp) {
  try {
    await Promise.all([
      readFile(binary),
      readFile(path.join(resources, "models.json")),
    ]);
    process.exit(0);
  } catch {}
}

const archive = path.join(resources, "codex.tar.gz");
try {
  await writeFile(
    archive,
    await download(`${release}/codex-aarch64-apple-darwin.tar.gz`, binaryHash),
  );
  execFileSync("tar", [
    "-xzf",
    archive,
    "-C",
    resources,
    "codex-aarch64-apple-darwin",
  ]);
  await rename(path.join(resources, "codex-aarch64-apple-darwin"), binary);
  await chmod(binary, 0o755);
  const catalog = JSON.parse(
    await download(
      `${source}/codex-rs/models-manager/models.json`,
      catalogHash,
    ),
  );
  // Feature flags alone do not remove model-defined tools in this pinned runtime.
  for (const model of catalog.models) {
    Object.assign(model, {
      apply_patch_tool_type: null,
      shell_type: "disabled",
      experimental_supported_tools: [],
      tool_mode: "traditional",
      supports_search_tool: false,
      include_skills_usage_instructions: false,
      include_apps_usage_instructions: false,
      include_plugin_usage_instructions: false,
    });
  }
  await writeFile(path.join(resources, "models.json"), JSON.stringify(catalog));
  for (const name of ["LICENSE", "NOTICE"]) {
    await writeFile(
      path.join(resources, name),
      await download(`${source}/${name}`),
    );
  }
  await writeFile(stampPath, stamp);
} finally {
  await rm(archive, { force: true });
}
