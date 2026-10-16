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

test("Azure signing pins the durable profile rather than a daily certificate", () => {
  const profileEku =
    "1.3.6.1.4.1.311.97.990309390.766961637.194916062.941502583";
  const environment = {
    LOOFAH_WINDOWS_SIGNER_EKU: profileEku,
    LOOFAH_WINDOWS_SIGN_COMMAND: JSON.stringify(["signer", "%1"]),
  };
  const result = signingArguments(environment, "app.exe");
  assert.equal(result.profileEku, profileEku);
  assert.equal(result.thumbprint, undefined);
  for (const value of [
    "1.3.6.1.4.1.311.97.1.0",
    "1.3.6.1.5.5.7.3.3",
    "invalid",
  ]) {
    assert.throws(
      () =>
        signingArguments(
          { ...environment, LOOFAH_WINDOWS_SIGNER_EKU: value },
          "app.exe",
        ),
      /specific Azure Public Trust/,
    );
  }
  assert.throws(
    () =>
      signingArguments(
        { ...environment, LOOFAH_WINDOWS_SIGNER_THUMBPRINT: "invalid" },
        "app.exe",
      ),
    /THUMBPRINT/,
  );
});
