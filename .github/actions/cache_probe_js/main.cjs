const fs = require("node:fs");
const label = process.env.INPUT_LABEL;
const value = process.env["INPUT_CACHE-WORKSPACE-CRATES"];
console.log(JSON.stringify({ phase: "main", label, value }));
fs.appendFileSync(process.env.GITHUB_STATE, `label=${label}\noriginal=${value}\n`);
