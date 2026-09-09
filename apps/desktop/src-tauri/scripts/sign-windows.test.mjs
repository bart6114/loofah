import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import { signingArguments } from "./sign-windows.mjs";

test("stable signing fails closed without a provisioned signer", () => {
  assert.throws(() => signingArguments({}, "app.exe"), /THUMBPRINT/);
  assert.throws(
    () =>
      signingArguments(
        { LOOFAH_WINDOWS_SIGNER_THUMBPRINT: "a".repeat(40) },
        "app.exe",
      ),
    /JSON/,
  );
});

test("signer receives a literal file argument without shell interpolation", () => {
  const file = "build with spaces/$literal & name.exe";
  const result = signingArguments(
    {
      LOOFAH_WINDOWS_SIGNER_THUMBPRINT: "a".repeat(40),
      LOOFAH_WINDOWS_SIGN_COMMAND: JSON.stringify(["signer", "sign", "%1"]),
    },
    file,
  );
  assert.deepEqual(result.args, ["sign", path.resolve(file)]);
  assert.equal(result.thumbprint, "A".repeat(40));
  assert.throws(
    () =>
      signingArguments(
        {
          LOOFAH_WINDOWS_SIGNER_THUMBPRINT: "a".repeat(40),
          LOOFAH_WINDOWS_SIGN_COMMAND: JSON.stringify(["signer", "sign %1"]),
        },
        file,
      ),
    /separate %1/,
  );
});
