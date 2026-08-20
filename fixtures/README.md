# Fixtures

Real transcripts from real sessions, reduced for size and scrubbed of anything private.
These are the parser's regression target. When a harness changes its JSONL shape, the
conformance test against these files is how we find out.

Regenerate with the scripts in [`../research/`](../research/). Never hand-edit a fixture:
if it no longer matches reality, capture a new one.

## claude-code/

### `session-basic.jsonl` — 11 entries

Claude Code 2.1.233, captured 2026-08-20. This is the session that proved mid-run
injection: a `PostToolUse` hook pushed a simulated thumbs-down into a live turn and the
agent acted on it without stopping.

Covers every block type the parser reads:

| block | count | note |
|---|---|---|
| `text` | 2 | assistant prose, full |
| `tool_use` | 1 | `Bash`, with full input |
| `tool_result` | 1 | full |
| `thinking` | 2 | **empty string plus signature, as Claude Code actually writes it** |
| `user` | 2 | |

The `thinking` entries are the important ones. They are not damaged fixtures. Claude Code
genuinely persists thinking as `{"type":"thinking","thinking":"","signature":"…"}` with no
text. A parser that assumes readable thinking will pass on synthetic data and fail on every
real session. Signatures are truncated to 32 characters here; only their presence matters.

Built with:

```bash
node research/make_fixture.js <real-session>.jsonl fixtures/claude-code/session-basic.jsonl
```

`attachment`, `file-history-snapshot` and `file-history-delta` entries are dropped. They
are bulky and carry file contents.

## codex/

### `session-basic.jsonl` — 14 entries

Codex CLI 0.146.0 via `codex exec`, captured 2026-08-20. Covers `session_meta`,
`event_msg`, `response_item`, `agent_message`, `task_complete`.

**Contains no `agent_reasoning` events, and that is correct.** `codex exec` does not emit
reasoning summaries. Only interactive sessions do. A parser tested solely against this file
would wrongly conclude Codex has no reasoning.

### `session-reasoning.jsonl` — 155 entries

From an interactive Codex rollout, so it has the 78 `agent_reasoning` summary events that
`codex exec` never produces.

Two transformations were applied, both deliberate:

1. **Allowlisted event types only.** Rebuilt field by field rather than filtered, so no
   unexpected sibling field survives. Interactive rollouts contain the operator's real
   shell commands.
2. **Prose replaced with neutral text of comparable length.** Structure is byte-faithful to
   a real rollout; the words are not. The original content was private project work.
   Lengths are preserved to within a word so width and wrapping tests stay meaningful.

Built with:

```bash
node research/make_codex_reasoning_fixture.js <real-rollout>.jsonl fixtures/codex/session-reasoning.jsonl
```

## Two harness behaviours worth knowing before you debug a parser

Both cost real time to discover, and neither is documented anywhere.

1. **`model_reasoning_summary = "none"` silently removes all `agent_reasoning` events.**
   The rollout still looks healthy. Margin should detect this and say so, rather than
   showing an empty reasoning column and letting the user assume the model went quiet.

2. **`codex exec` never emits reasoning summaries regardless of that setting.** Reasoning
   cards are an interactive-session feature.
