import fs from "node:fs";
import path from "node:path";

function pe(file) {
  const data = fs.readFileSync(file);
  const header = data.readUInt32LE(0x3c);
  if (data.toString("ascii", header, header + 4) !== "PE\0\0")
    throw new Error(`Invalid PE: ${file}`);
  const optional = header + 24;
  const sections = optional + data.readUInt16LE(header + 20);
  const directory =
    optional + (data.readUInt16LE(optional) === 0x20b ? 112 : 96);
  const ranges = Array.from(
    { length: data.readUInt16LE(header + 6) },
    (_, i) => {
      const offset = sections + i * 40;
      return {
        address: data.readUInt32LE(offset + 12),
        size: Math.max(
          data.readUInt32LE(offset + 8),
          data.readUInt32LE(offset + 16),
        ),
        raw: data.readUInt32LE(offset + 20),
      };
    },
  );
  const rva = (address) => {
    const section = ranges.find(
      (s) => address >= s.address && address < s.address + s.size,
    );
    if (!section) throw new Error(`Invalid RVA ${address} in ${file}`);
    return section.raw + address - section.address;
  };
  const string = (address) => {
    const start = rva(address);
    return data.toString("ascii", start, data.indexOf(0, start));
  };
  const exports = new Set();
  if (data.readUInt32LE(directory)) {
    const table = rva(data.readUInt32LE(directory));
    const count = data.readUInt32LE(table + 24);
    if (count > 0) {
      const names = rva(data.readUInt32LE(table + 32));
      for (let i = 0; i < count; i++)
        exports.add(string(data.readUInt32LE(names + i * 4)));
    }
  }
  const imports = [];
  const importAddress = data.readUInt32LE(directory + 8);
  if (importAddress) {
    const wide = data.readUInt16LE(optional) === 0x20b;
    for (
      let entry = rva(importAddress);
      data.readUInt32LE(entry + 12);
      entry += 20
    ) {
      const dll = string(data.readUInt32LE(entry + 12));
      const names = [];
      const table = data.readUInt32LE(entry) || data.readUInt32LE(entry + 16);
      for (let thunk = rva(table); ; thunk += wide ? 8 : 4) {
        const value = wide
          ? data.readBigUInt64LE(thunk)
          : BigInt(data.readUInt32LE(thunk));
        if (!value) break;
        if (!(value & (wide ? 0x8000000000000000n : 0x80000000n)))
          names.push(string(Number(value) + 2));
      }
      imports.push({ dll, names });
    }
  }
  return {
    imports,
    exports,
    machine: data.readUInt16LE(header + 4).toString(16),
  };
}

const executable = path.resolve(process.argv[2]);
const directories = [
  path.dirname(executable),
  path.dirname(path.dirname(executable)),
  path.join(process.env.SystemRoot || "C:\\Windows", "System32"),
  ...(process.env.PATH || "").split(path.delimiter),
];
const find = (name) =>
  directories
    .map((dir) => path.join(dir, name))
    .find((file) => fs.existsSync(file));
const visited = new Set();
let failures = 0;
function inspect(file, depth = 0) {
  if (visited.has(file.toLowerCase())) return;
  visited.add(file.toLowerCase());
  const binary = pe(file);
  console.log(`${file} (machine ${binary.machine})`);
  for (const entry of binary.imports) {
    if (/^(api|ext)-ms-/i.test(entry.dll)) continue;
    const dependency = find(entry.dll);
    if (!dependency) {
      console.error(`MISSING DLL: ${entry.dll}, imported by ${file}`);
      failures++;
      continue;
    }
    const exports = pe(dependency).exports;
    for (const name of entry.names) {
      if (!exports.has(name)) {
        console.error(
          `MISSING ENTRY POINT: ${name} in ${dependency}, imported by ${file}`,
        );
        failures++;
      }
    }
    if (
      depth < 2 &&
      !dependency
        .toLowerCase()
        .startsWith((process.env.SystemRoot || "C:\\Windows").toLowerCase())
    )
      inspect(dependency, depth + 1);
  }
}
inspect(executable);
process.exitCode = failures ? 1 : 0;
