import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const tauri = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const root = path.resolve(tauri, "../../..");
const version = JSON.parse(
  readFileSync(path.join(root, "package.json"), "utf8"),
).version;
const target = process.env.RELEASE_TARGET;
const architecture = {
  "x86_64-pc-windows-msvc": "x86_64",
  "aarch64-pc-windows-msvc": "aarch64",
}[target];
if (!architecture || process.env.RELEASE_TAG !== `v${version}`)
  throw new Error("Release version or architecture mismatch");
const output = path.join(root, "artifacts/windows-release");
mkdirSync(output, { recursive: true });
const bundle = path.join(tauri, "target/release/bundle/nsis");
const installers = readdirSync(bundle).filter((name) =>
  name.endsWith("-setup.exe"),
);
if (installers.length !== 1) throw new Error("Expected one NSIS installer");
const installer = `Loofah_${version}_${architecture}-setup.exe`;
const signature = readFileSync(
  path.join(bundle, `${installers[0]}.sig`),
  "utf8",
).trim();
if (!signature) throw new Error("Updater signature missing");
for (const suffix of ["", ".sig"])
  copyFileSync(
    path.join(bundle, installers[0] + suffix),
    path.join(output, installer + suffix),
  );
const cli = path.join(root, "artifacts/cli");
mkdirSync(cli, { recursive: true });
copyFileSync(
  path.join(root, "target/release/loof.exe"),
  path.join(cli, "loof.exe"),
);
for (const name of readdirSync(path.join(tauri, "resources/windows-runtime"))) {
  if (name.toLowerCase().endsWith(".dll"))
    copyFileSync(
      path.join(tauri, "resources/windows-runtime", name),
      path.join(cli, name),
    );
}
const check = spawnSync(path.join(cli, "loof.exe"), ["--version"], {
  encoding: "utf8",
});
if (check.error || check.status !== 0 || !check.stdout.includes(version))
  throw new Error("Bundled CLI failed version smoke test");
const archive = `loof-${version}-${target}.zip`;
const powershell = path.join(
  process.env.SystemRoot || "C:\\Windows",
  "System32/WindowsPowerShell/v1.0/powershell.exe",
);
const zip = spawnSync(
  powershell,
  [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    "$ErrorActionPreference = 'Stop'; Compress-Archive -Path (Join-Path $env:LOOFAH_CLI_DIR '*') -DestinationPath $env:LOOFAH_CLI_ZIP",
  ],
  {
    env: {
      ...process.env,
      LOOFAH_CLI_DIR: cli,
      LOOFAH_CLI_ZIP: path.join(output, archive),
    },
    stdio: "inherit",
  },
);
if (zip.error || zip.status !== 0)
  throw new Error("CLI archive creation failed");
for (const name of [installer, archive]) {
  const hash = createHash("sha256")
    .update(readFileSync(path.join(output, name)))
    .digest("hex");
  writeFileSync(path.join(output, `${name}.sha256`), `${hash}  ${name}\n`);
}
writeFileSync(
  path.join(output, `platform-${architecture}.json`),
  JSON.stringify(
    {
      version,
      pub_date: new Date().toISOString(),
      platforms: {
        [`windows-${architecture}`]: {
          signature,
          url: `https://github.com/${process.env.RELEASE_REPOSITORY}/releases/download/v${version}/${installer}`,
        },
      },
    },
    null,
    2,
  ),
);
