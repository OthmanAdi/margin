// Codex only emits `agent_reasoning` summary events in interactive sessions.
// `codex exec` produces none, so a reasoning fixture has to come from a real
// interactive rollout. Those rollouts also contain the operator's actual shell
// commands, which must not be committed.
//
// This keeps only the event types the parser reads and drops every payload that
// could carry command text, paths, or environment references.
//
// usage: node make_codex_reasoning_fixture.js <rollout.jsonl> <out.jsonl>

const fs = require("fs");
const [, , input, output] = process.argv;
if (!input || !output) {
  console.error("usage: node make_codex_reasoning_fixture.js <rollout.jsonl> <out.jsonl>");
  process.exit(2);
}

// allowlist, not blocklist: anything not named here never reaches the fixture
const KEEP = new Set(["session_meta", "agent_reasoning", "agent_message", "task_complete", "token_count"]);
const MAX = 240;

const clip = (s) => (typeof s === "string" && s.length > MAX ? s.slice(0, MAX) + "…<truncated>" : s);

// The fixture is committed to a public repo. Structure must stay byte-faithful to a
// real rollout, but the prose is whatever the operator happened to be working on, so
// it is replaced with neutral text of comparable shape. Length is preserved to within
// a word so width and wrapping tests stay meaningful.
const NEUTRAL_REASONING = [
  "**Planning the parser change**",
  "**Checking the fixture format**",
  "**Reviewing edge cases in slugify**",
  "**Deciding between two approaches**",
  "**Verifying the test actually fails first**",
  "**Reading the transcript schema**",
];
const NEUTRAL_MESSAGE = [
  "I'll start by reading the fixture to confirm the field names before writing the parser.",
  "Two edge cases stand out: an empty string, and input that is entirely punctuation.",
  "The test passes now. Next I'll handle the torn-last-line case when tailing a live file.",
  "That rules out the polling approach. Switching to the watcher and re-measuring.",
];

let rIdx = 0, mIdx = 0;
const neutralReasoning = () => NEUTRAL_REASONING[rIdx++ % NEUTRAL_REASONING.length];
const neutralMessage = () => NEUTRAL_MESSAGE[mIdx++ % NEUTRAL_MESSAGE.length];

const out = [];
let seen = 0, keptCount = 0;

for (const line of fs.readFileSync(input, "utf8").split("\n")) {
  if (!line.trim()) continue;
  seen++;
  let e; try { e = JSON.parse(line); } catch (err) { continue; }

  const p = e.payload || e;
  if (!KEEP.has(p.type)) continue;

  // rebuild rather than filter, so no unexpected sibling field survives
  const payload = { type: p.type };
  if (p.type === "agent_reasoning") payload.text = clip(neutralReasoning());
  if (p.type === "agent_message") payload.message = clip(neutralMessage());
  if (p.type === "session_meta") {
    payload.id = p.id;
    payload.timestamp = p.timestamp;
    payload.cwd = "<redacted>";
    payload.originator = p.originator;
    payload.cli_version = p.cli_version;
  }
  if (p.type === "token_count") payload.info = p.info ? { total_token_usage: p.info.total_token_usage } : undefined;

  const entry = { timestamp: e.timestamp, type: e.type, payload };
  out.push(JSON.stringify(entry));
  keptCount++;
}

fs.writeFileSync(output, out.join("\n") + "\n");
console.log(`in   : ${seen} lines`);
console.log(`kept : ${keptCount} lines (allowlisted types only)`);
console.log(`out  : ${output}  ${fs.statSync(output).size} bytes`);
