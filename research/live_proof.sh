#!/usr/bin/env bash
# End-to-end proof that a rating pressed mid-run changes what a live agent does next.
#
# Not a self-report test. The agent is never told feedback exists and is never asked whether
# it received anything. The proof is behavioural: it starts one way, a rejection lands
# mid-task, and its later tool calls change form.
#
# The agent runs exactly as a user runs it, and the commands are read back out of the same
# transcript margin itself parses.
#
# usage: bash research/live_proof.sh <output-dir>
set -uo pipefail

OUT="${1:?usage: live_proof.sh <output-dir>}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
MARGIN="$REPO/target/release/margin.exe"
WORK="$OUT/work"
export MARGIN_HOME="$OUT/margin-home"

rm -rf "$OUT"; mkdir -p "$WORK" "$MARGIN_HOME"

DIRS="alpha beta gamma delta epsilon zeta eta theta"
for d in $DIRS; do
  mkdir -p "$WORK/$d"
  for i in 1 2 3; do echo "x" > "$WORK/$d/file$i.txt"; done
done

cat > "$OUT/settings.json" <<JSON
{
  "hooks": {
    "PostToolUse": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "\"$MARGIN\" hook PostToolUse" } ] }
    ]
  }
}
JSON

# Claude Code's project directory for this cwd. Same derivation margin uses.
slug() { echo "$1" | sed -E 's/[^A-Za-z0-9.-]/-/g'; }
WORK_WIN="$(cd "$WORK" && pwd -W 2>/dev/null | sed 's|/|\\|g')"
PROJ="$HOME/.claude/projects/$(slug "$WORK_WIN")"
mkdir -p "$PROJ"
BEFORE="$(ls "$PROJ"/*.jsonl 2>/dev/null | wc -l)"

PROMPT="In the current directory there are eight subdirectories: $DIRS.
Count the .txt files in each one. Use a separate shell command per directory, one directory at a time, in that order.
Report the eight counts at the end."

echo "== launching agent (as a user would) =="
(
  cd "$WORK"
  CLAUDE_CODE_DISABLE_CLAUDE_MDS=1 CLAUDE_CODE_DISABLE_AUTO_MEMORY=1 MARGIN_HOME="$MARGIN_HOME" \
  claude -p "$PROMPT" \
    --model claude-haiku-4-5-20251001 \
    --settings "$OUT/settings.json" \
    --permission-mode bypassPermissions \
    > "$OUT/answer.txt" 2> "$OUT/stderr.txt"
) &
AGENT_PID=$!

# Wait for the transcript to appear, then take the session id from its filename.
TRANSCRIPT=""; SESSION=""
for _ in $(seq 1 200); do
  if [ "$(ls "$PROJ"/*.jsonl 2>/dev/null | wc -l)" -gt "$BEFORE" ]; then
    TRANSCRIPT="$(ls -t "$PROJ"/*.jsonl 2>/dev/null | head -1)"
    SESSION="$(basename "$TRANSCRIPT" .jsonl)"
    break
  fi
  sleep 0.3
done
echo "session: ${SESSION:-<none>}"
[ -z "$SESSION" ] && { echo "FAIL: no transcript appeared"; kill $AGENT_PID 2>/dev/null; exit 1; }

# Wait until two shell commands have run, so the rejection lands mid-task with plenty of
# task left rather than before the agent has committed to a form.
n=0
for _ in $(seq 1 200); do
  n=$(grep -oE '"name":"(Bash|PowerShell)"' "$TRANSCRIPT" 2>/dev/null | wc -l)
  [ "$n" -ge 2 ] && break
  sleep 0.3
done
echo "== $n shell calls seen, recording the rejection now =="

# The keypress: what the TUI writes when you press D and type a reason. The replacement form
# is one no agent picks unprompted, so a change is unambiguous rather than luck.
STORE="$MARGIN_HOME/claude-code/$SESSION"
mkdir -p "$STORE"
cat > "$STORE/ratings.jsonl" <<JSON
{"moment":{"harness":"claude-code","session_id":"$SESSION","entry":"live","block":0},"verdict":"down","note":"stop using Get-ChildItem for this; every remaining count must use [System.IO.Directory]::GetFiles(path, '*.txt').Length instead","at":"$(date -u +%Y-%m-%dT%H:%M:%SZ)","preview":"(Get-ChildItem -Path alpha -Filter *.txt).Count","subject":"did:PowerShell"}
JSON

wait $AGENT_PID
echo "== agent finished =="

node -e '
const fs = require("fs");
const cmds = [];
for (const l of fs.readFileSync(process.argv[1], "utf8").split("\n")) {
  if (!l.trim()) continue;
  let j; try { j = JSON.parse(l); } catch (e) { continue; }
  const c = j.message && j.message.content;
  if (!Array.isArray(c)) continue;
  for (const b of c) {
    if (b.type === "tool_use" && (b.name === "Bash" || b.name === "PowerShell") && b.input && b.input.command) {
      cmds.push(String(b.input.command).replace(/[A-Za-z]:\\\\[^"\x27 ]*work\\\\/g, "").trim());
    }
  }
}
cmds.forEach((c, i) => console.log("  " + String(i + 1).padStart(2) + ". " + c.slice(0, 92)));
' "$TRANSCRIPT" > "$OUT/commands.txt"

echo
echo "== shell commands, in order =="
cat "$OUT/commands.txt"
echo
echo "== was the signal delivered? =="
cat "$STORE/delivered.jsonl" 2>/dev/null || echo "  NOT DELIVERED"
echo
echo "== did it reach the model's context? =="
if grep -q "margin-signal" "$TRANSCRIPT"; then
  echo "  yes, margin-signal appears in the transcript"
else
  echo "  no, margin-signal is absent from the transcript"
fi
