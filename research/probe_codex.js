const fs = require("fs");
const F = process.argv[2];
const lines = fs.readFileSync(F, "utf8").split("\n").filter(Boolean);

let reasoning = 0, reasoningChars = 0, agentMsg = 0;
const samples = [];
const shapes = new Set();

for (const l of lines) {
  let j; try { j = JSON.parse(l); } catch (e) { continue; }
  const p = j.payload || j;
  const t = p.type;

  if (t === "agent_reasoning") {
    reasoning++;
    const txt = String(p.text || "");
    reasoningChars += txt.length;
    if (samples.length < 2 && txt.trim()) {
      samples.push({ kind: "agent_reasoning", ts: j.timestamp, len: txt.length, head: txt.slice(0, 200) });
    }
    shapes.add("agent_reasoning:" + Object.keys(p).join(","));
  }
  if (t === "agent_message") {
    agentMsg++;
    const txt = String(p.message || p.text || "");
    if (samples.length < 4 && txt.trim()) {
      samples.push({ kind: "agent_message", ts: j.timestamp, len: txt.length, head: txt.slice(0, 160) });
    }
    shapes.add("agent_message:" + Object.keys(p).join(","));
  }
  if (t === "reasoning") {
    shapes.add("reasoning:" + Object.keys(p).join(","));
  }
}
console.log("agent_reasoning events:", reasoning, "| total readable chars:", reasoningChars);
console.log("agent_message events:", agentMsg);
console.log("\nshapes:");
for (const s of shapes) console.log("  " + s);
console.log("\nsamples:");
for (const s of samples) {
  console.log(`  [${s.kind}] ts=${s.ts} len=${s.len}`);
  console.log("    " + JSON.stringify(s.head));
}
