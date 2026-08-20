#!/usr/bin/env bash
# Show a margin signal arriving in a live agent, in the agent's own words.
#
# live_proof.sh proves the behaviour changes. This one is for watching: it asks the agent to
# quote back anything that appeared in its context which the user did not type, so the
# injected block is visible rather than merely inferred.
#
# usage: bash research/see_it.sh <output-dir>
set -uo pipefail

OUT="${1:?usage: see_it.sh <output-dir>}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
MARGIN="$REPO/target/release/margin.exe"
[ -f "$MARGIN" ] || MARGIN="$HOME/bin/margin.exe"
WORK="$OUT/work"
export MARGIN_HOME="$OUT/margin-home"

rm -rf "$OUT"; mkdir -p "$WORK" "$MARGIN_HOME"
for d in one two three four five; do mkdir -p "$WORK/$d"; echo hello > "$WORK/$d/a.txt"; done

cat > "$OUT/settings.json" <<JSON
{
  "hooks": {
    "PostToolUse": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "\"$MARGIN\" hook PostToolUse" } ] }
    ]
  }
}
JSON

slug() { echo "$1" | sed -E 's/[^A-Za-z0-9.-]/-/g'; }
WORK_WIN="$(cd "$WORK" && pwd -W 2>/dev/null | sed 's|/|\\|g')"
PROJ="$HOME/.claude/projects/$(slug "$WORK_WIN")"
mkdir -p "$PROJ"
BEFORE="$(ls "$PROJ"/*.jsonl 2>/dev/null | wc -l)"

PROMPT='Count the .txt files in each of these subdirectories, one shell command each, in order: one, two, three, four, five.
Then, as the very last thing, answer this exactly: did any text appear in your context during this task that I did not type? If yes, quote it verbatim inside a fenced code block. If no, say NOTHING APPEARED.'

echo "== launching agent =="
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

TRANSCRIPT=""; SESSION=""
for _ in $(seq 1 200); do
  if [ "$(ls "$PROJ"/*.jsonl 2>/dev/null | wc -l)" -gt "$BEFORE" ]; then
    TRANSCRIPT="$(ls -t "$PROJ"/*.jsonl 2>/dev/null | head -1)"
    SESSION="$(basename "$TRANSCRIPT" .jsonl)"
    break
  fi
  sleep 0.3
done
[ -z "$SESSION" ] && { echo "FAIL: no transcript"; kill $AGENT_PID 2>/dev/null; exit 1; }
echo "session: $SESSION"

for _ in $(seq 1 200); do
  [ "$(grep -oE '"name":"(Bash|PowerShell)"' "$TRANSCRIPT" 2>/dev/null | wc -l)" -ge 1 ] && break
  sleep 0.3
done

STORE="$MARGIN_HOME/claude-code/$SESSION"
mkdir -p "$STORE"
cat > "$STORE/ratings.jsonl" <<JSON
{"moment":{"harness":"claude-code","session_id":"$SESSION","entry":"live","block":0},"verdict":"down","note":"stop counting with Get-ChildItem; use [System.IO.Directory]::GetFiles(path, '*.txt').Length","at":"$(date -u +%Y-%m-%dT%H:%M:%SZ)","preview":"the command used to count files in directory one","subject":"did:PowerShell"}
JSON
echo "== rating recorded mid-run =="

wait $AGENT_PID
echo

echo "== hook heartbeat (proves the hook actually ran) =="
if [ -f "$STORE/hook-seen" ]; then echo "  hook-seen present"; else echo "  NO heartbeat: the hook never ran"; fi

echo
echo "== delivered =="
cat "$STORE/delivered.jsonl" 2>/dev/null || echo "  nothing delivered"

echo
echo "== the agent's own words =="
cat "$OUT/answer.txt"
