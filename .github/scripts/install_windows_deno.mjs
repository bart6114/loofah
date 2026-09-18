import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";

const checksums = {
  arm64: "acb014afe2299847764e232b4993e162e3946cdeec36603e3f1a0b548cd1ea55",
  x64: "15e5300b0ba3c3695a7621d90160a746ec9e710228cee639afa9d580f6e3cd11",
};
if (
  process.platform !== "win32" ||
  !checksums[process.arch] ||
  !process.env.RUNNER_TEMP ||
  !process.env.GITHUB_PATH
) {
  throw new Error(
    "This installer requires a native Windows x64 or ARM64 GitHub runner",
  );
}
const architecture = process.arch === "arm64" ? "aarch64" : "x86_64";
const directory = path.join(process.env.RUNNER_TEMP, "loofah-deno");
mkdirSync(directory, { recursive: true });
const response = await fetch(
  `https://github.com/denoland/deno/releases/download/v2.9.6/deno-${architecture}-pc-windows-msvc.zip`,
);
if (!response.ok) throw new Error(`Deno download failed: ${response.status}`);
const bytes = Buffer.from(await response.arrayBuffer());
if (
  createHash("sha256").update(bytes).digest("hex") !== checksums[process.arch]
)
  throw new Error("Deno archive checksum mismatch");
const archive = path.join(directory, "deno.zip");
writeFileSync(archive, bytes);
// Avoid setup-deno's nested PowerShell/.NET ZIP extraction, which crashes on ARM runners.
const tar = path.join(
  process.env.SystemRoot || "C:\\Windows",
  "System32/tar.exe",
);
for (const [program, args] of [
  [tar, ["-xf", archive, "-C", directory]],
  [path.join(directory, "deno.exe"), ["--version"]],
]) {
  const result = spawnSync(program, args, { stdio: "inherit" });
  if (result.error || result.status !== 0)
    throw new Error("Deno installation or startup failed");
}
appendFileSync(process.env.GITHUB_PATH, `${directory}\n`);
