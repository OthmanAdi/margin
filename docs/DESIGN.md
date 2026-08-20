# Margin — design sketch

Grounded in `FEASIBILITY.md`. Nothing here depends on a capability that was not measured.

## The moment this exists for

You start a long `/goal` run. The agent works. Twenty minutes in it makes a call you like,
and later a call you do not. Today your options are: interrupt and lose the turn, or say
nothing and watch it compound.

Margin is the third option. You press one key. The agent keeps working, and it finds out.

## Shape

A second terminal pane, next to the agent, not wrapping it.

```
┌─ agent ────────────────────────┐┌─ margin ─────────────────────┐
│                                ││                              │
│  ● Thinking...                 ││  ·  12:04:02  thought        │
│                                ││     1.2k tokens              │
│  I'll check the transcript     ││                              │
│  format first.                 ││  ▸  12:04:11  said           │
│                                ││     "I'll check the tran…"   │
│  ⏺ Bash(grep -c thinking …)    ││                              │
│    ⎿ 0                         ││  ▾  12:04:19  Bash      👎   │
│                                ││     grep -c thinking …       │
│  That rules it out.            ││     ↳ wrong file, use debug  │
│                                ││                              │
│                                ││  ▸  12:04:31  said           │
│                                ││                              │
└────────────────────────────────┘└──────────────────────────────┘
     you are typing here              you glance here, press j/k/f/d
```

Left pane is untouched. Margin never intercepts a key the agent's terminal wanted, because
it is a different pane with its own focus. That is rule 2 satisfied by architecture rather
than by discipline.

### Keys

```
j / k      move the cursor down / up through moments
f          thumbs up      (favour)
d          thumbs down
D          thumbs down, then one line of why
g          jump to newest, and follow live
```

Rating is one key. The note is a deliberate second action, offered only when you want it.

### Moment cards

One card per addressable thing. From `FEASIBILITY.md` §1 these are:

| Card | Claude Code | Codex |
|---|---|---|
| `said` | `text` block, full prose | `agent_message`, full prose |
| `did` | `tool_use` + `tool_result` | `custom_tool_call` + output |
| `thought` | placeholder: time + token count | `agent_reasoning` summary text |

The `thought` card is honest about the gap. On Claude Code it shows what it can prove
(when it happened, how long it thought) and nothing it cannot. On Codex it shows the
summary line. Same card, same key, different fidelity, clearly labelled.

## Data flow

```
  harness writes JSONL          margin tails it
  ────────────────────          ──────────────────
  ~/.claude/projects/…    ───▶  parse → moments (uuid-keyed)
  ~/.codex/sessions/…     ───▶            │
                                          ▼
                                   you press f / d
                                          │
                                          ▼
                              ratings.jsonl  (local, append-only)
                                          │
                    ┌─────────────────────┼─────────────────────┐
                    ▼                     ▼                     ▼
            PostToolUse hook        SessionStart hook        export
            injects pending         replays your             eval cases,
            feedback into the       standing preferences     few-shot
            RUNNING turn            into the next session    examples
```

### Anchoring

A rating stores the identity of the moment, never an offset that can shift:

```json
{
  "ts": "2026-08-20T12:04:19.412Z",
  "harness": "claude-code",
  "session_id": "9c42ba52-…",
  "anchor": { "kind": "tool_use", "uuid": "6de117ee-…", "tool_use_id": "toolu_01CAF…" },
  "prompt_id": "127f92cf-…",
  "verdict": "down",
  "note": "wrong file, use debug",
  "digest": "sha256:…"
}
```

`uuid` and `tool_use_id` are stable per `FEASIBILITY.md` §1 and §2. `digest` hashes the
rated content so a rating can be detected as stale if the transcript is ever rewritten.

### Injection

The proven path from `FEASIBILITY.md` §2. A `PostToolUse` hook drains anything rated since
the last tool call and returns it:

```json
{ "hookSpecificOutput": {
    "hookEventName": "PostToolUse",
    "additionalContext": "[margin] The user marked your last Bash call unhelpful: \"wrong file, use debug\"." } }
```

`PostToolUse` fires constantly during real work, so the agent learns within one tool call.
The user typed nothing and the turn was never interrupted.

Two rules this must obey:

- **Deliver once.** A rating is drained exactly once. Re-injecting the same complaint every
  tool call would poison the context.
- **Stay quiet when there is nothing.** The hook emits no output when the queue is empty,
  so the common case costs nothing.

## Build order

Each step ends at something demonstrable, and each is a commit.

1. **Read.** Parse both harnesses' JSONL into a common `Moment`. Fixtures from real
   transcripts, committed. Proves the model survives both shapes.
2. **Watch.** Tail live, print moments as they land. Proves liveness and latency, with the
   number measured.
3. **Rate.** The TUI, keys, `ratings.jsonl`. At this point it is already useful alone.
4. **Inject.** The hook. This is the feature that makes it more than a diary, and it is the
   one already proven to work.
5. **Carry over.** `SessionStart` replay of standing preferences.
6. **Export.** Eval cases and few-shot examples out.

Steps 1 to 4 are the product. 5 and 6 are what make the ratings compound.

## Deliberately not doing

- Wrapping or patching either harness. Rule 3.
- A web UI. It would not be next to the terminal, which is the whole point.
- Cloud sync, accounts, teams. Rule 5.
- Grok Build and OpenCode until a transcript from each has been read. Claimed support for
  an unverified harness is a lie with a nice table around it.

## Open decisions for Ahmad

1. **Name.** `margin` is a placeholder, chosen because marginalia is exactly what this is:
   notes made alongside a running text without altering it. Alternatives: `aside`, `nudge`,
   `claque`. Renaming before the first push is free; after is not.
2. **Language.** Rust gives a fast single binary and matches the eval exporter he already
   has. TypeScript gives faster iteration and matches the harnesses' own ecosystem.
3. **Layout.** Separate pane the user arranges, or does Margin manage a split itself?
