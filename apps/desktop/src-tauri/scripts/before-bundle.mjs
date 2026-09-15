import { spawn, spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { stageWindowsRuntime } from "./windows-runtime.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));

function runScript(scriptName) {
  return new Promise((resolve, reject) => {
    const child = spawn("bash", [path.join(scriptDir, scriptName)], {
      stdio: "inherit",
    });

    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) {
        resolve();
        return;
      }

      reject(new Error(`${scriptName} exited with code ${code ?? "unknown"}`));
    });
  });
}

if (process.platform === "win32") {
  const tauriDir = path.resolve(scriptDir, "..");
  const host = spawnSync("rustc", ["-vV"], { encoding: "utf8" }).stdout?.match(
    /^host: (.+)$/m,
  )?.[1];
  const target = process.env.TAURI_ENV_TARGET_TRIPLE ?? host;
  if (!target) throw new Error("Unable to determine the Windows build target");
  const targetDir = path.resolve(
    tauriDir,
    process.env.CARGO_TARGET_DIR ?? "target",
  );
  stageWindowsRuntime(
    tauriDir,
    [path.join(targetDir, ...(target === host ? [] : [target]), "release")],
    target,
  );
  process.exit(0);
}

if (process.platform === "darwin") {
  await runScript("compile-icons.sh");
}

await runScript("fix-dylib.sh");
