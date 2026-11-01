import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const version = "0.160.0";
const root = fileURLToPath(new URL("../", import.meta.url));
const resources = path.join(root, "resources/codex");
const catalogHash =
  "fd219bd9f061278275f528939f82f54d2eb97df4b25c23b022adbe48813d920b";
const source = `https://raw.githubusercontent.com/openai/codex/rust-v${version}`;

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
const stamp = `${version}:${catalogHash}:models-only:1`;
const stampPath = path.join(resources, "version");
if ((await readFile(stampPath, "utf8").catch(() => "")) === stamp) {
  try {
    await Promise.all([
      readFile(path.join(resources, "models.json")),
      readFile(path.join(resources, "LICENSE")),
      readFile(path.join(resources, "NOTICE")),
    ]);
    process.exit(0);
  } catch {}
}

const catalog = JSON.parse(
  await download(`${source}/codex-rs/models-manager/models.json`, catalogHash),
);
// Feature flags alone do not remove model-defined tools in the supported Codex runtime.
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
