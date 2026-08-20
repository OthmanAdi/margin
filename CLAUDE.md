# Margin — working rules

Read `docs/FEASIBILITY.md` before proposing anything. It contains measured facts about
Claude Code and Codex internals. If a plan contradicts it, the plan is wrong.

## What this is

A rating layer for running AI coding agents. You watch the agent work, and when a thought
or an output is good or bad, you press one key. The agent does not stop. The rating is
anchored to that exact moment, and it can be fed back into the run.

## The one job

Rate a specific moment, without interrupting, in one keystroke.

Everything else is optional. When a feature would make that one job slower or less
certain, the feature loses. This is not a dashboard, an analytics product, or a chat UI.

## Non-negotiables

1. **One keystroke.** Rating costs a single key. No mode switch, no mouse, no form, no
   confirmation. The moment it needs two deliberate actions, people stop using it, and the
   product is dead.
2. **Never steal focus.** The agent's terminal keeps the cursor. If our UI ever intercepts
   a key the harness wanted, we broke the user's primary tool.
3. **Never touch the harness.** No patching Claude Code, no wrapping its binary, no
   injecting into its process. We read files it already writes and use hooks it already
   supports. Anything else breaks on their next release and is our fault.
4. **Degrade loudly, never silently.** If a transcript format changes and we parse zero
   entries, say so on screen. A feedback tool that quietly records nothing is worse than
   no tool.
5. **Local by default.** Ratings are the user's judgments about their own work. Nothing
   leaves the machine without an explicit, separate action.

## Engineering rules

- **Reuse before writing.** Check the stdlib, then an installed dependency, then write
  code. A new dependency needs a reason in the commit message.
- **Every non-trivial behaviour ships one runnable check.** The smallest thing that fails
  if the logic breaks. Parsers get a fixture from a real transcript, committed.
- **Transcript parsers are versioned and conformance-tested.** One fixture per harness per
  known format version. A drift failure must name the harness and the field that moved.
- **No speculative abstraction.** One implementation means no interface. Two harnesses do
  not justify a plugin architecture until the third one actually disagrees.
- **Measure before optimising, and say the number.** "Fast" is not a claim, it is a
  benchmark with a figure attached.
- **Never claim something works without running it.** If it was not executed, the wording
  is "should" and it is flagged.

## Commit rules

- Conventional commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `perf:`.
- One logical change per commit. A commit that needs "and" in its subject is two commits.
- Subject in the imperative, under 72 characters.
- The body says **why**, not what. The diff already says what.
- Every commit builds and its checks pass. No "wip", no broken intermediate states.
- **Author is Ahmad only. Never add `Co-Authored-By`.** Contributors are credited in
  CHANGELOG and CONTRIBUTORS.md.

## Prose rules

For anything a human other than Ahmad will read (README, docs, release notes, issues, PR
text): no dashes used as pauses. Commas, colons, parentheses, or a new sentence. Compound
hyphens like "well-architected" are fine. Tone is matter-of-fact. No marketing voice, no
"In today's fast-paced world", no "Great question". Code blocks and CLI flags are exempt.

## Definition of done

A change is done when it runs on this machine, its check passes, the commit is clean, and
the claim made about it in the commit body is true.
