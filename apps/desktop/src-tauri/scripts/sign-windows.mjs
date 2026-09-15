import { spawnSync } from "node:child_process";
import path from "node:path";
import { pathToFileURL } from "node:url";

export function signingArguments(environment, file) {
  const thumbprint = environment.LOOFAH_WINDOWS_SIGNER_THUMBPRINT?.replaceAll(
    " ",
    "",
  ).toUpperCase();
  if (!thumbprint || !/^[0-9A-F]{40}$/.test(thumbprint)) {
    throw new Error(
      "LOOFAH_WINDOWS_SIGNER_THUMBPRINT must identify the provisioned signing certificate",
    );
  }
  let command;
  try {
    command = JSON.parse(environment.LOOFAH_WINDOWS_SIGN_COMMAND || "");
  } catch {
    throw new Error(
      "LOOFAH_WINDOWS_SIGN_COMMAND must be a JSON argument array containing %1",
    );
  }
  if (
    !Array.isArray(command) ||
    command.length < 2 ||
    command.some(
      (argument) => typeof argument !== "string" || argument.includes("\0"),
    ) ||
    !command[0] ||
    !command.slice(1).includes("%1")
  ) {
    throw new Error(
      "The signing command must include a separate %1 argument for the file",
    );
  }
  return {
    command: command[0],
    args: command
      .slice(1)
      .map((argument) => (argument === "%1" ? path.resolve(file) : argument)),
    thumbprint,
  };
}

export function signWindows(file) {
  if (process.platform !== "win32")
    throw new Error("Authenticode signing requires Windows");
  const { command, args, thumbprint } = signingArguments(process.env, file);
  const signed = spawnSync(command, args, { stdio: "inherit", shell: false });
  if (signed.error || signed.status !== 0)
    throw new Error("The configured Windows signing service failed");
  const powershell = path.join(
    process.env.SystemRoot || "C:\\Windows",
    "System32/WindowsPowerShell/v1.0/powershell.exe",
  );
  const verified = spawnSync(
    powershell,
    [
      "-NoLogo",
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "$signature = Get-AuthenticodeSignature -LiteralPath $env:LOOFAH_SIGNED_FILE; if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Thumbprint -ne $env:LOOFAH_EXPECTED_SIGNER -or $null -eq $signature.TimeStamperCertificate) { throw 'Authenticode signature, publisher, or timestamp validation failed' }",
    ],
    {
      env: {
        ...process.env,
        LOOFAH_SIGNED_FILE: path.resolve(file),
        LOOFAH_EXPECTED_SIGNER: thumbprint,
      },
      stdio: "inherit",
    },
  );
  if (verified.error || verified.status !== 0)
    throw new Error("Windows refused the signed artifact");
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  if (!process.argv[2]) throw new Error("A file to sign is required");
  signWindows(process.argv[2]);
}
