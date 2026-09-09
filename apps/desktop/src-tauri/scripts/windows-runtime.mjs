import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync } from "node:fs";
import path from "node:path";

export function stageWindowsRuntime(tauriDir, artifactDirs, target) {
  if (process.platform !== "win32") return;
  const architecture = target.startsWith("aarch64") ? "arm64" : "x64";
  const vswhere = path.join(
    process.env["ProgramFiles(x86)"] || "C:\\Program Files (x86)",
    "Microsoft Visual Studio/Installer/vswhere.exe",
  );
  const result = spawnSync(
    vswhere,
    ["-latest", "-products", "*", "-property", "installationPath"],
    { encoding: "utf8" },
  );
  if (result.error || result.status !== 0 || !result.stdout.trim()) {
    throw new Error(
      "Visual Studio C++ redistributable files could not be located",
    );
  }
  const root = path.join(result.stdout.trim(), "VC/Redist/MSVC");
  const versions = readdirSync(root)
    .filter((name) => /^\d/.test(name))
    .sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
  const runtime = versions
    .map((version) => path.join(root, version, architecture))
    .filter((directory) => existsSync(directory))
    .flatMap((directory) =>
      readdirSync(directory)
        .filter((name) => /^Microsoft\.VC\d+\.CRT$/i.test(name))
        .map((name) => path.join(directory, name)),
    )[0];
  if (!runtime)
    throw new Error(`Visual C++ ${architecture} CRT is missing from ${root}`);

  const output = path.join(tauriDir, "resources/windows-runtime");
  mkdirSync(output, { recursive: true });
  for (const directory of [...artifactDirs, runtime]) {
    if (!existsSync(directory)) continue;
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (!entry.isFile() || !entry.name.toLowerCase().endsWith(".dll"))
        continue;
      copyFileSync(
        path.join(directory, entry.name),
        path.join(output, entry.name),
      );
    }
  }
  if (!existsSync(path.join(output, "vcruntime140.dll"))) {
    throw new Error("Windows runtime staging did not produce vcruntime140.dll");
  }
  return output;
}
