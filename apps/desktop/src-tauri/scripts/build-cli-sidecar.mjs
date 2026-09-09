import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const tauriDir = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const repoDir = path.resolve(tauriDir, "../../..");

function run(command, args) {
  const result = spawnSync(command, args, { cwd: repoDir, encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stderr ?? "");
    throw new Error(`${command} exited with ${result.status}`);
  }
  return result.stdout;
}

const host = run("rustc", ["-vV"]).match(/^host: (.+)$/m)?.[1];
if (!host) throw new Error("Unable to determine Rust host target");
const target = process.argv[2] ?? host;
const suffix = target.includes("windows") ? ".exe" : "";
run("cargo", [
  "build",
  "--locked",
  "--release",
  "-p",
  "loof-cli",
  ...(target === host ? [] : ["--target", target]),
]);
const targetDir = process.env.CARGO_TARGET_DIR ?? path.join(repoDir, "target");
const built = path.join(
  targetDir,
  ...(target === host ? [] : [target]),
  "release",
  `loof${suffix}`,
);
for (const dir of ["binaries", "resources/cli"]) {
  mkdirSync(path.join(tauriDir, dir), { recursive: true });
  copyFileSync(built, path.join(tauriDir, dir, `loof-${target}${suffix}`));
}
