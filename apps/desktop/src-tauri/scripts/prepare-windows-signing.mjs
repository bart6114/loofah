import { writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { signingArguments } from "./sign-windows.mjs";

signingArguments(process.env, "preflight.exe");
if (!process.env.RUNNER_TEMP) throw new Error("RUNNER_TEMP is required");
writeFileSync(
  path.join(process.env.RUNNER_TEMP, "loofah-signing.json"),
  JSON.stringify({
    bundle: {
      windows: {
        signCommand: {
          cmd: process.execPath,
          args: [
            fileURLToPath(new URL("./sign-windows.mjs", import.meta.url)),
            "%1",
          ],
        },
      },
    },
  }),
);
