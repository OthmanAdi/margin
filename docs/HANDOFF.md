# Handoff

Everything a person or agent needs to pick this up cold. Written 2026-08-20, at 34 commits
and 86 tests.

Read in this order: this file, then [PROOF.md](PROOF.md), then [FEASIBILITY.md](FEASIBILITY.md).
[../CLAUDE.md](../CLAUDE.md) holds the rules that constrain every decision here.

## What margin is

A pane beside your coding agent listing every moment it produces. You press one key to
approve or reject one of them. The agent does not stop; it finds out at its next tool call
and adjusts.

It never wraps, patches or launches the harness. It reads the transcript the harness already
writes and uses the hooks it already supports.

## State

Working and used daily on this machine: Claude Code transcript parsing, live tailing, the
rating TUI, mid-run injection, the status-line segment, three view levels.

Not built: session-boundary carry-over of standing preferences, eval export, Codex
injection. Codex is read-only, and `docs/FEASIBILITY.md` explains why.

## The five facts that constrain the design

Each one was measured, and a script in [`../research/`](../research/) reproduces it. Do not
redesign around any of them without re-running the script first.

1. **Claude Code never persists thinking text.** 0 readable blocks out of 253 across three
   sessions: every one is an empty string plus a 4.5 KB signature. A `thought` row therefore
   says how much reasoning happened, never what it was. It *is* readable over
   `--output-format stream-json`, so the limit is the interactive surface, not the model.

2. **`--output-format stream-json` silently drops hook `additionalContext`.** The hook runs,
   its output is accepted, the rating records as delivered, and the block never reaches the
   model. Works fine interactively and with plain `-p`. Cost three test runs to find. Any
   test of hook behaviour must drive the agent the way a person does and read the transcript
   file, not the stream.

3. **Claude Code's shell tools carry a human-written `description`.** Five to ten words of
   intent, sitting next to the command. It is a better row label than anything recoverable by
   parsing the command, and it is free. `MomentKind::Did.intent`.

4. **Hook commands must use forward slashes.** They are handed to a shell that may be bash,
   which treats a backslash as an escape, so `C:\Users\me\bin\margin.exe` arrives as
   `C:Usersmebinmargin.exe`. Every tool call then fails with "command not found", silently,
   because a hook failure does not stop the agent. This cost most of a day.

5. **Settings are read at session start.** A hook installed mid-session is inert until the
   next restart. `margin install` says so, and the hook writes a `hook-seen` heartbeat so the
   UI can distinguish "not wired up" from "not loaded yet".

## Three Windows traps baked into the code

- **crossterm fires Press *and* Release per keystroke.** Without the `KeyEventKind::Press`
  filter every rating double-fires, and only on Windows.
- **Tail with a plain `File`.** Rust's default share mode already reads a file the harness
  holds open. Adding share-mode or locking calls is how this breaks.
- **notify watches the parent directory, never the file**, so it cannot contend with the
  harness's write handle.

## The invariant everything else serves

**A rating must land on the moment the user was looking at.** `CLAUDE.md` calls a wrong
target the worst failure this tool has, and most of the hard bugs have been variations on it:

| bug | how it attached feedback to the wrong thing |
|---|---|
| auto-follow moved the cursor | a moment arriving between glance and keypress retargeted it |
| the drain loop discarded key events | the keypress vanished entirely |
| Codex ids counted from zero per poll | a new moment overwrote an old one and inherited its verdict |
| delivery keyed by moment | a correction was suppressed forever as "already delivered" |
| the capped batch marked everything delivered | ratings past the cap retired unseen |
| a subagent hook answered | a subagent ate the main agent's rating |
| collapse ran after the delivered filter | one moment gave up a revision per hook call, forever |

All fixed, each with a regression test. When touching selection, filtering or delivery, ask
which of these you are about to reintroduce.

## Layout

```
src/moment.rs        the one model both harnesses collapse into; MomentId is the anchor
src/harness/         one parser per harness, no trait, no dispatch
src/humanize.rs      turning a tool call into something a human can judge in a second
src/tail.rs          following a file another process is appending to
src/ratings.rs       append-only store; pending = ratings - delivered
src/inject.rs        the wording, which is the hard half
src/ui.rs            the pane
src/snapshot.rs      renders the real UI to SVG for the README
src/discover.rs      finding the session to watch
```

Two processes touch the store: the TUI on a keypress, and the hook inside the agent's own
process. Neither locks. Each appends to its own log and `pending` is derived.

## Commands

```bash
margin watch [--replay]      the pane
margin sessions [--all]      what margin can see
margin install --write --statusline
margin hook <event>          called by a hook, not a human
margin snapshot              regenerate the README images
```

Hook events handled: `PostToolUse` (the main path), `Stop`, `UserPromptSubmit`, `PreCompact`
(flush before the context is summarised), `SessionEnd` (record what missed its window).

## Verify it still works

```bash
cargo test                                    # 86, and CI runs them on three platforms
cargo clippy --all-targets -- -D warnings     # CI treats warnings as errors
bash research/live_proof.sh /tmp/proof        # a rating changing a live agent's behaviour
bash research/see_it.sh /tmp/see              # the agent quoting the signal back
cargo run -- snapshot                         # CI fails if docs/img is stale
```

The images in the README are generated from the real widget tree, so a UI change that forgets
to regenerate them breaks the build rather than shipping a stale picture.

## What I would do next, in order

1. **Session carry-over.** A `SessionStart` hook replaying standing preferences is the
   feature that makes ratings compound instead of expiring with the session. Everything it
   needs already exists.
2. **Codex injection.** Read `FEASIBILITY.md` first: Codex's hook system is off by default in
   this config and `codex mcp-server` may be the better route.
3. **Eval export.** The store is already append-only and anchored, so this is mostly a
   formatting job.
4. **Verify Grok Build and OpenCode**, or drop them from the README. They are listed as
   unverified because no transcript from either has been read, and that should stay honest.

## Things deliberately not done

- Wrapping or patching a harness.
- A web UI. It would not be next to the terminal, which is the whole point.
- Cloud sync, accounts, teams.
- Claiming support for a harness whose transcript nobody has read.
