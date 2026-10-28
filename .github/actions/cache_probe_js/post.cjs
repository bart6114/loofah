const label = process.env.STATE_label;
const original = process.env.STATE_original;
const value = process.env["INPUT_CACHE-WORKSPACE-CRATES"];
console.log(JSON.stringify({ phase: "post", label, original, value }));
