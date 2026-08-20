// Proves the injection path: a hook can push feedback into a RUNNING session
// without the user typing anything and without interrupting the agent.
const fs = require("fs");
let raw = "";
try { raw = fs.readFileSync(0, "utf8"); } catch (e) { }

// record exactly what the harness handed us, so we know what an overlay can anchor to
try {
  const dir = process.env.FB_PROBE_DIR;
  if (dir) fs.appendFileSync(dir + "/hook-payload.jsonl", raw.trim() + "\n");
} catch (e) { }

let evt = {};
try { evt = JSON.parse(raw); } catch (e) { }

// simulate: the user just hit thumbs-down on the previous step from a separate UI
const pending = process.env.FB_PENDING;
if (pending) {
  process.stdout.write(JSON.stringify({
    hookSpecificOutput: {
      hookEventName: evt.hook_event_name || "PostToolUse",
      additionalContext: pending
    }
  }));
}
process.exit(0);
