import { hashPassword } from "better-auth/crypto";
import { Buffer } from "node:buffer";
import { scrypt, timingSafeEqual } from "node:crypto";

export const password = {
  hash: hashPassword,
  async verify({ hash, password }: { hash: string; password: string }) {
    const parts = /^([0-9a-f]{32}):([0-9a-f]{128})$/.exec(hash);
    if (!parts) return false;
    // Better Auth 1.7.5 / utils 0.4.2 uses these parameters, but compares with ===.
    // Keep its hash format and cost while replacing only the verification comparison.
    const actual = await new Promise<Buffer>((resolve, reject) => {
      scrypt(
        password.normalize("NFKC"),
        parts[1],
        64,
        { N: 16384, r: 16, p: 1, maxmem: 128 * 16384 * 16 * 2 },
        (error, key) => {
          if (error) reject(error);
          else resolve(key);
        },
      );
    });
    try {
      return timingSafeEqual(actual, Buffer.from(parts[2], "hex"));
    } finally {
      actual.fill(0);
    }
  },
};
