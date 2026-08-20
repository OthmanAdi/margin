# Margin

Rate your coding agent while it runs. One keystroke, no interruption.

> Status: research complete, design drafted, implementation not started.
> The core mechanic is proven on this machine. See [docs/FEASIBILITY.md](docs/FEASIBILITY.md).

## The problem

You are watching an agent work. It has a good thought. It makes a bad call. Right now your
only options are to interrupt it, losing the turn, or to say nothing and watch the mistake
compound for another twenty minutes.

There is no way to say "that one, yes" or "that one, no" while it keeps working.

## What Margin does

A pane beside your agent, listing every moment it produces: what it said, what it did, what
it thought. You move with `j` and `k`, and press `f` or `d`.

The agent does not stop. It finds out at its next tool call, and adjusts.

```
·  12:04:02  thought    1.2k tokens
▸  12:04:11  said       "I'll check the transcript format first."
▾  12:04:19  Bash  👎   grep -c thinking …
                        ↳ wrong file, use debug
```

## Why it is not just a diary

Ratings are not filed away. They do three things:

- **Steer the current run.** A thumbs-down reaches the agent within one tool call, through
  a hook, with no prompt typed and no turn interrupted. This is measured, not theorised.
- **Carry into the next session.** What you consistently downvote becomes a standing
  preference, replayed at session start.
- **Become eval data.** Rated moments export as eval cases and few-shot examples.

## Supported harnesses

| | messages | tool calls | reasoning | mid-run steering |
|---|---|---|---|---|
| Claude Code (terminal) | full | full | timing only | yes |
| Claude Code (headless) | full | full | full | yes |
| Codex | full | full | summaries | planned |
| Grok Build | not yet verified | | | |
| OpenCode | not yet verified | | | |

Claude Code does not persist the text of its thinking, only a signature, so a `thought`
card there shows when it happened and how long it took rather than pretending to quote it.
The full reasoning behind that limitation, and the three ways around it, is in
[docs/FEASIBILITY.md](docs/FEASIBILITY.md) §3.

Grok Build and OpenCode are listed as unverified because no transcript from either has been
read yet. They will be marked supported when that changes, not before.

## Reproducing the research

Every claim in the feasibility document comes from a script in [`research/`](research/),
run against real session files on a real machine.

```bash
node research/probe_thinking.js   ~/.claude/projects/<slug>/<session>.jsonl
node research/probe_transcript.js ~/.claude/projects/<slug>/<session>.jsonl
node research/probe_codex.js      ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl
```

## Documents

- [docs/FEASIBILITY.md](docs/FEASIBILITY.md) — what is actually possible, measured
- [docs/DESIGN.md](docs/DESIGN.md) — the shape, the data flow, the build order
- [CLAUDE.md](CLAUDE.md) — working rules for anyone, human or agent, touching this repo

## License

Not yet chosen. See DESIGN.md open decisions.
