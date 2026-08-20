# Feasibility: can you rate a running agent without interrupting it?

Measured on 2026-08-20 against Claude Code 2.1.233 and Codex CLI 0.146.0 on Windows 11.
Every claim below was produced by a script in `research/`, not by reading documentation.
Re-runnable.

## Verdict up front

**Yes, and the hard half is the half that works.** The mechanic Ahmad described (react to a
specific moment, agent keeps running, no interruption) is real and I have it running.

One capability is genuinely missing, and it is in the surface he named first. Details in
§3. It changes what the product can store, not whether it works.

---

## 1. Can a separate process see the agent's output live?

Yes, on both harnesses, by tailing a JSONL file the harness already writes.

**Claude Code** writes `~/.claude/projects/<slug>/<session-id>.jsonl`, appended during the
session. Confirmed live: the file for the running session was modified at 12:16 while the
clock read 12:17.

Each entry carries what an anchor needs:

```
parentUuid, isSidechain, message, requestId, type, uuid, timestamp,
effort, session_id, userType, entrypoint, cwd, sessionId, version, gitBranch
```

Content arrives as typed blocks. From one real session, 891 entries:

| Block | Count | Readable |
|---|---|---|
| `text` (assistant prose) | 16 | yes, in full |
| `tool_use` (name + full input) | 115 | yes, in full |
| `tool_result` | 114 | yes, in full |
| `thinking` | 71 | **no, see §3** |

`uuid` is stable per entry and `parentUuid` chains them, so any message or tool call is
addressable forever.

**Codex** writes `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`. Same idea, richer:

| Event | Count | Readable |
|---|---|---|
| `agent_message` | 69 | yes, in full |
| `agent_reasoning` | 274 | **yes** |
| `custom_tool_call` / `_output` | 147 / 147 | yes |
| `reasoning` (full chain) | 245 | no, `encrypted_content` |

Codex persists its reasoning *summaries* as plain text, averaging ~45 characters:

```
"**Planning urgent Claude activity catch-up**"
```

Those are exactly the lines Codex renders in its UI. Individually addressable, and
meaningful enough to rate.

## 2. Can feedback reach the agent mid-run, without the user typing?

**Yes. This is proven, not projected.** A hook returning
`hookSpecificOutput.additionalContext` injects text into the live turn.

Test: a `PostToolUse` hook emitted a simulated thumbs-down carrying a token. The agent,
mid-turn, replied:

> Yes, I received a mid-run feedback message. The feedback indicated: **Correction token:
> ZEBRA7739.**

No prompt was submitted. The turn was not interrupted. The agent simply knew.

The hook payload gives everything needed to correlate a rating to a moment:

```
session_id, transcript_path, cwd, prompt_id, permission_mode,
hook_event_name, tool_name, tool_input, tool_response, tool_use_id, duration_ms
```

`tool_use_id` is a stable identity per tool call; `prompt_id` identifies the turn.

Available hook events on Claude Code: `PreToolUse`, `PostToolUse`, `UserPromptSubmit`,
`Stop`, `SubagentStop`, `SessionStart`, `SessionEnd`, `Notification`, `PreCompact`,
`PermissionRequest`. Injection fields: `additionalContext`, `systemMessage`, `decision`,
`continue`, `stopReason`, `suppressOutput`.

Codex has its own hook system (`features.hooks`, currently `false` in this config) plus
`codex mcp-server`, `codex app-server` and `codex remote-control`.

`PostToolUse` fires constantly during real work, so injected feedback lands within one
tool call. That is the non-interrupting delivery channel.

## 3. The one real gap: Claude Code thinking text is not persisted

All 71 thinking blocks in the live session, and 177 in another, and 5 in a third, were
**empty strings with a 4.5 KB cryptographic signature**:

```json
{"type":"thinking","thinking":"","signature":"CAISlRoKowEIEBgCKkAOIDCQBnZW..."}
```

Readable thinking blocks found across three sessions: **0 of 253.**

It is not hiding elsewhere. Checked and ruled out:

- `~/.claude/debug/*.txt` — 0 occurrences of thinking content
- `~/.claude/transcripts/` — older `ses_*` format, same story
- `~/.claude/telemetry/` — failed-event payloads only

The binary shows why. The TUI renders thinking from **in-memory** history:

```js
Lt?.type === "assistant" && Lt.message.content[0]?.type === "thinking"
```

It exists at render time and is stripped before the transcript is written.

There is no thinking hook. (`"Thinking"` appears in the binary twice: as a spinner word
next to "Thundering" and "Tinkering", and as the UI label that flips to "Thought". I chased
it; it is not an event.)

**Where thinking IS available:** the streaming interface. Running headless with
`--output-format stream-json --include-partial-messages` produced 9 `thinking_delta`
events carrying plain text:

```
idx: 0 | "The"
idx: 0 | " user is asking me to determine whether "
idx: 0 | "91 is prime."
```

So the text exists on the wire. It is only the interactive TUI that does not persist it.

### What this actually costs

Less than it first appears, because of an asymmetry worth stating plainly:

**The agent still has its own thinking in its context.** So a thumbs-down on a thought
steers correctly in-session even if our tool never stored a single character of it. "The
user marked your reasoning at 12:04:11 as unhelpful" is fully actionable to the agent.

What is lost is the *durable record*. Reviewing your ratings a week later, a Claude Code
thought reads as a placeholder, not prose.

Three honest options, in order of preference:

1. **Rate positionally, capture a note.** The card shows `thought · 12:04:11 · 1.2k tokens`.
   Thumbs-down optionally takes one line of text. The note is the durable artifact, which
   is what a human would have written anyway.
2. **Full fidelity where the wire is available** — Codex now, Claude Code headless/SDK
   runs now, Claude Code TUI if upstream ever persists it. Same UI, degrades per surface.
3. **PTY-scrape the TUI.** Rejected. Reconstructing prose from an alt-screen ANSI
   fullscreen TUI breaks on every release. Not worth owning.

## 4. Per-harness support

| | messages | tool calls | reasoning text | mid-run injection |
|---|---|---|---|---|
| **Claude Code (TUI)** | full | full | **positional only** | **yes, proven** |
| **Claude Code (headless / SDK)** | full | full | full | yes |
| **Codex (CLI + app)** | full | full | summaries, readable | via hooks / app-server |
| Grok Build | unverified | unverified | unverified | unverified |
| OpenCode | unverified | unverified | unverified | unverified |

Grok Build and OpenCode were not tested. Neither is installed here, and I will not claim
support for a harness I have not read a transcript from.

## 5. Is it actually helpful, or just a nice demo?

Asked directly, so answered directly.

**The weak version is a diary.** Thumbs collected into a local file that nothing consumes
is journaling. It feels productive and changes nothing. If that is all it does, it is not
worth building.

**Three things make it real, and all three are reachable:**

1. **Mid-run steering.** Proven in §2. During a `/goal` run the agent works unattended for
   many turns; today the only correction available is Escape, which destroys the turn.
   Thumbs-down plus one line, landing at the next tool call, is a genuinely new control
   surface. This is the strongest reason to build it.

2. **A preference corpus that is yours.** Rated moments export as eval cases and few-shot
   examples. Ahmad already has a verified eval corpus (6 golden datasets, 27 evaluators),
   so the consumer for this data already exists.

3. **Session-boundary carry-over.** A `SessionStart` hook can replay "here is what he
   consistently downvotes" into the next session. That is a personal style profile
   assembled from real judgments rather than a hand-written CLAUDE.md.

**The honest risk is not technical, it is behavioural.** Rating requires attention at the
exact moment attention is elsewhere. Most feedback UIs die here. The mitigation is that
rating must cost one keystroke and never steal focus. If it ever needs a mouse, a mode
switch, or a form, it will not be used. That constraint should drive the whole design.

**Second risk: transcript formats are undocumented and will drift.** Both harnesses can
change their JSONL shape in any release. The parser must be defensive and version-tagged,
with a conformance test per harness that fails loudly rather than silently reading nothing.

## 6. What is proven vs assumed

Proven, with a re-runnable script:

- Claude Code transcript is live-appended and fully addressable by `uuid`
- Claude Code thinking is unrecoverable from disk (0/253 across three sessions)
- Claude Code thinking is fully recoverable from the stream interface
- Mid-run injection into a live turn works and the agent acts on it
- Hook payloads carry `tool_use_id` and `prompt_id` for stable anchoring
- Codex persists readable reasoning summaries and full messages

Assumed, still to verify:

- Codex desktop app writes the same JSONL as the CLI, live
- Codex hooks can inject like Claude Code hooks (`features.hooks` is off here)
- Terminal-render latency of an overlay next to a fullscreen TUI
- Grok Build and OpenCode: everything
