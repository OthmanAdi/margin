const fs = require("fs");
const F = process.argv[2];
const lines = fs.readFileSync(F, "utf8").split("\n").filter(Boolean);
const types = {};
let thinking = 0, textblocks = 0, tooluse = 0, toolres = 0;
let sampleThink = null, sampleAsst = null, sampleTool = null;

for (const l of lines) {
  let j; try { j = JSON.parse(l); } catch (e) { continue; }
  types[j.type] = (types[j.type] || 0) + 1;
  const c = j.message && j.message.content;
  if (Array.isArray(c)) for (const b of c) {
    if (b.type === "thinking") { thinking++; if (!sampleThink) sampleThink = { entry: j, block: b }; }
    if (b.type === "text") { textblocks++; if (!sampleAsst) sampleAsst = { entry: j, block: b }; }
    if (b.type === "tool_use") { tooluse++; if (!sampleTool) sampleTool = { entry: j, block: b }; }
    if (b.type === "tool_result") toolres++;
  }
}
console.log("total lines:", lines.length);
console.log("entry types:", JSON.stringify(types));
console.log("blocks -> thinking:", thinking, "text:", textblocks, "tool_use:", tooluse, "tool_result:", toolres);

if (sampleThink) {
  console.log("\n=== THINKING BLOCK ===");
  console.log("block keys:", Object.keys(sampleThink.block).join(","));
  console.log("readable text:", JSON.stringify(String(sampleThink.block.thinking || "").slice(0, 200)));
  console.log("has signature:", !!sampleThink.block.signature);
  console.log("entry uuid:", sampleThink.entry.uuid, "ts:", sampleThink.entry.timestamp);
}
if (sampleAsst) {
  console.log("\n=== ASSISTANT TEXT BLOCK ===");
  console.log("entry keys:", Object.keys(sampleAsst.entry).join(","));
  console.log("uuid:", sampleAsst.entry.uuid, "parentUuid:", sampleAsst.entry.parentUuid);
  console.log("ts:", sampleAsst.entry.timestamp);
  console.log("text:", JSON.stringify(String(sampleAsst.block.text || "").slice(0, 160)));
  const m = sampleAsst.entry.message || {};
  console.log("message keys:", Object.keys(m).join(","));
  console.log("model:", m.model, "| msg id:", m.id);
}
if (sampleTool) {
  console.log("\n=== TOOL_USE BLOCK ===");
  console.log("block keys:", Object.keys(sampleTool.block).join(","));
  console.log("name:", sampleTool.block.name, "| id:", sampleTool.block.id);
}
