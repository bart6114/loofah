import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import wrangler from "wrangler";

const { unstable_dev, unstable_splitSqlQuery: splitSqlQuery } = wrangler;

test(
  "workerd password profile and R2 multipart upload, restart, immutable retry and atomic commit",
  { timeout: 120_000 },
  async () => {
    const worker = await unstable_dev(
      fileURLToPath(new URL("./worker.ts", import.meta.url)),
      {
        config: fileURLToPath(new URL("./wrangler.json", import.meta.url)),
        local: true,
        persist: false,
        logLevel: "error",
        experimental: {
          disableExperimentalWarning: true,
          disableDevRegistry: true,
          watch: false,
        },
      },
    );
    try {
      const directory = new URL("../migrations/", import.meta.url);
      for (const file of readdirSync(directory)
        .filter((name) => name.endsWith(".sql"))
        .sort()) {
        const statements = splitSqlQuery(
          readFileSync(new URL(file, directory), "utf8"),
        );
        const result = await worker.fetch("/initialize", {
          method: "POST",
          body: JSON.stringify(statements),
        });
        assert.equal(result.status, 200, await result.text());
      }
      const auth = await worker.fetch("/auth-health");
      assert.equal(auth.status, 200, await auth.clone().text());
      assert.deepEqual(await auth.json(), { session: null });
      const pairing = await worker.fetch("/pairing-health");
      assert.equal(pairing.status, 200, await pairing.clone().text());
      assert.deepEqual(await pairing.json(), { upgraded: true });
      const password = await worker.fetch("/password");
      assert.equal(password.status, 200, await password.clone().text());
      const benchmark = await password.json();
      assert.equal(benchmark.valid, true);
      assert.equal(benchmark.invalid, false);
      console.log(
        `workerd password hash ${benchmark.hashMs} ms; verify ${benchmark.verifyMs} ms`,
      );
      const upload = await worker.fetch("/objects");
      assert.equal(upload.status, 200, await upload.clone().text());
      assert.deepEqual(await upload.json(), {
        bytes: 8 * 1024 * 1024 + 17,
        usage: 8 * 1024 * 1024 + 21,
        rejected: true,
      });
    } finally {
      await worker.stop();
    }
  },
);
