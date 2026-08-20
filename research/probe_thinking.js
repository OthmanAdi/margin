const fs = require("fs");
const F = process.argv[2];
const lines = fs.readFileSync(F, "utf8").split("\n").filter(Boolean);

let empty = 0, readable = 0, totalChars = 0;
const samples = [];
for (const l of lines) {
  let j; try { j = JSON.parse(l); } catch (e) { continue; }
  const c = j.message && j.message.content;
  if (!Array.isArray(c)) continue;
  for (const b of c) {
    if (b.type !== "thinking") continue;
    const t = String(b.thinking || "");
    if (t.trim().length === 0) { empty++; }
    else {
      readable++; totalChars += t.length;
      if (samples.length < 3) samples.push({ uuid: j.uuid, ts: j.timestamp, len: t.length, head: t.slice(0, 130) });
    }
  }
}
console.log("thinking blocks -> readable:", readable, "| empty/redacted:", empty);
console.log("avg readable length:", readable ? Math.round(totalChars / readable) : 0, "chars");
console.log("");
for (const s of samples) {
  console.log("uuid:", s.uuid, "| ts:", s.ts, "| len:", s.len);
  console.log("  ", JSON.stringify(s.head));
}
