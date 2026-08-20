// Build a compact, structurally faithful fixture from a real harness transcript.
//
// Fixtures are committed so the parser has a regression target. They must stay
// small and must not carry anything private, so this:
//   - drops `attachment` entries (bulky, and they carry file contents)
//   - truncates thinking signatures to 32 chars (we only assert they exist)
//   - truncates any string over `maxStr` chars
//   - keeps every structural field the parser reads
//
// usage: node make_fixture.js <input.jsonl> <output.jsonl> [--harness claude-code|codex]

const fs = require("fs");

const [, , input, output, ...rest] = process.argv;
if (!input || !output) {
  console.error("usage: node make_fixture.js <input.jsonl> <output.jsonl> [--harness <name>]");
  process.exit(2);
}
const harness = (() => {
  const i = rest.indexOf("--harness");
  return i >= 0 ? rest[i + 1] : "claude-code";
})();

const MAX_STR = 300;
const DROP_TYPES = new Set(["attachment", "file-history-snapshot", "file-history-delta"]);

// Fixtures are committed to a public repo, so real home directories must not survive.
// Applies to every string, not just the obvious `cwd`, because paths turn up inside tool
// inputs, results, and prose.
const HOME = (process.env.USERPROFILE || process.env.HOME || "").replace(/\\/g, "\\\\");
const USER = (process.env.USERNAME || process.env.USER || "").trim();

function redactPaths(s) {
  let out = s;
  if (HOME) out = out.split(HOME).join("<home>");
  if (USER && USER.length > 2) {
    out = out.split(USER).join("<user>");
  }
  return out;
}

function shrink(value, key) {
  if (typeof value === "string") {
    if (key === "signature" || key === "encrypted_content") {
      return value.slice(0, 32) + (value.length > 32 ? "…<truncated>" : "");
    }
    const v = redactPaths(value);
    return v.length > MAX_STR ? v.slice(0, MAX_STR) + "…<truncated>" : v;
  }
  if (Array.isArray(value)) return value.map((v) => shrink(v));
  if (value && typeof value === "object") {
    const out = {};
    for (const [k, v] of Object.entries(value)) out[k] = shrink(v, k);
    return out;
  }
  return value;
}

const lines = fs.readFileSync(input, "utf8").split("\n").filter((l) => l.trim());
const kept = [];
let dropped = 0;

for (const line of lines) {
  let entry;
  try { entry = JSON.parse(line); } catch (e) { dropped++; continue; }
  const type = entry.type || (entry.payload && entry.payload.type);
  if (DROP_TYPES.has(type)) { dropped++; continue; }
  kept.push(JSON.stringify(shrink(entry)));
}

fs.writeFileSync(output, kept.join("\n") + "\n");

const bytes = fs.statSync(output).size;
console.log(`harness : ${harness}`);
console.log(`in      : ${lines.length} lines`);
console.log(`kept    : ${kept.length} lines  (dropped ${dropped})`);
console.log(`out     : ${output}  ${bytes} bytes`);
