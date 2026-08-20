codex gpt-5.6-sol/ultra  read-only  dir=C:/Users/oasrvadmin/CLEANROOM/work/margin
done in 1,483.3s  exit=0  tokens in/out=7638568/38235
SESSION: 01a01f2f-aa60-72e2-a046-a4d7b18a71ad
LOG: C:\Users\oasrvadmin\CLEANROOM\logs\20260820-144011-review-correctness.jsonl

I found several real correctness failures. The first four can silently attach feedback to the wrong moment or agent, which is the tool's worst failure mode. Line numbers reflect the working tree at the end of review; it was being edited concurrently.

## Critical

1. **Codex live polling reuses `MomentId`s and moves ratings to later moments.**  
   [src/ui.rs:113](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:113), [src/harness/codex.rs:58](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/codex.rs:58), [src/harness/codex.rs:98](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/codex.rs:98)

   Trigger: poll these lines separately:

   ```json
   {"type":"event_msg","payload":{"type":"agent_message","message":"A"}}
   {"type":"event_msg","payload":{"type":"agent_message","message":"B"}}
   ```

   Each `absorb()` parses only its new batch. `codex::parse()` resets `session_id` to `unknown` and enumeration to zero, so both become `codex/unknown/L0#0`. The second overwrites the first at `ui.rs:122-123`, retaining the first card's verdict/note key. While entering a `D` note, the note intended for A can therefore be saved against B.

   Default non-replay mode also skips the existing `session_meta`; replay learns the real session initially but downgrades to `unknown` on the next append.

   Minimal fix: use persistent Codex parser state carrying the canonical session ID and absolute position across polls, seeded by scanning `session_meta` even when starting at EOF. Prefer native payload/call IDs and refuse rateable `unknown` identities.

2. **Transcript replacement/truncation mixes sessions and can inject an A rating into B.**  
   [src/tail.rs:53](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/tail.rs:53), [src/tail.rs:61](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/tail.rs:61), [src/ui.rs:121](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:121), [src/ui.rs:163](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:163)

   Two triggers:

   - At offset 1,000, replace the path with a different 1,000-byte file, or truncate and regrow it past 1,000 before polling. Size-only detection either returns "unchanged" or seeks into the middle of the new file, losing its prefix and session metadata.
   - Replace session A with a shorter session B. Tailer resets to zero, but does not tell `App`; the UI retains A's cards while switching its single `Store` to B. Rating an old A card writes its A `MomentId` into B's directory, and B's hook does not reject it.

   Codex's physical `L<n>` identities compound this: a same-session rewrite can make an old `S/L1` rating identify entirely different content.

   Minimal fix: track file identity plus a committed-prefix witness and return an explicit reset/generation event. On reset, atomically clear/rebuild moments, parser state, selection, note mode, ratings, and store. `Store::record` and hook reads should reject records whose harness/session differs from the bound store.

3. **Delivery is keyed by moment, so a concurrent correction is permanently suppressed.**  
   [src/ratings.rs:66](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ratings.rs:66), [src/ratings.rs:156](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ratings.rs:156), [src/main.rs:195](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:195)

   Interleaving:

   1. A has an `Up` rating.
   2. Hook snapshots it.
   3. TUI appends `Down(A)`.
   4. Hook records only "moment A delivered" and injects the stale approval.
   5. Every later `pending()` skips both ratings because A is in `HashSet<MomentId>`.

   Sequentially re-rating any already-delivered moment fails the same way, despite the UI promising delivery.

   Minimal fix: assign every rating an immutable revision/event ID and make `Delivery` reference that revision. Collapse the ratings log to the latest revision per moment first, then suppress only when that exact latest revision was delivered.

4. **A subagent or another process sharing the session can consume the rating.**  
   [src/main.rs:172](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:172)

   Trigger: rate a root-agent moment in session S, then let an Explore subagent finish a tool first. Its hook payload contains the same `session_id` plus `agent_id`, but `hook()` reads only `session_id`; it injects the root rating into the subagent and marks it delivered. The root never receives it.

   Claude's hook schema explicitly provides `agent_id`/`agent_type` when hooks fire inside subagents, and resuming one session in two terminals can interleave both processes into the same transcript. [Hook inputs](https://code.claude.com/docs/en/agent-sdk/hooks), [session behavior](https://code.claude.com/docs/en/sessions).

   Minimal fix: immediately ignore payloads containing `agent_id` until subagent support exists. Longer term, store branch/process identity and correlate ratings with `prompt_id`, `tool_use_id`, transcript lineage, and `transcript_path`.

## High

5. **Pending-read and delivery-mark are not an atomic claim, so parallel hooks duplicate feedback.**  
   [src/ratings.rs:156](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ratings.rs:156), [src/main.rs:195](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:195)

   H1 and H2 can both read rating R before either marks it, then both print R. This is a normal path because `PostToolUse` fires concurrently for parallel tool calls. [Claude Code hooks reference](https://code.claude.com/docs/en/hooks).

   Additionally, `mark_delivered(...).ok()` ignores disk-full/permission failures and still prints, causing repetition on every later hook.

   Minimal fix: implement a cross-process-locked `claim_pending(limit)` transaction that selects and persists exact rating revisions before returning them. Emit nothing if claiming fails. Using `PostToolBatch` would reduce parallel invocations but does not replace atomic claiming.

6. **More than five pending ratings are marked delivered without being rendered.**  
   [src/inject.rs:70](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/inject.rs:70), [src/inject.rs:79](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/inject.rs:79), [src/main.rs:203](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:203)

   Six pending ratings make `render()` include only the newest five, but `main` marks all six IDs delivered. The omitted rating never appears later; the new human-visible notice also counts it as delivered.

   Minimal fix: have selection/claiming return the exact capped batch, and render, mark, and count only that batch.

7. **UI event coalescing drops ratings, and a nearby race can rate a moment not displayed at keypress.**  
   [src/ui.rs:317](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:317), [src/ui.rs:145](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:145), [src/ui.rs:163](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:163)

   - Queue `FileChanged`, then `f` or `d`. `while rx.try_recv().is_ok()` consumes the key without dispatching it.
   - If the key arrives just after that drain while M1 remains displayed, polling can append M2 and following selects M2. The queued key is then handled against mutable selection M2, even though M1 was displayed when pressed.

   Minimal fix: coalesce only file notifications. Preserve every key/quit event, and attach the last-rendered `MomentId` plus preview/generation to rating key events rather than resolving mutable selection later.

8. **The durable precise ID is discarded from the injected feedback.**  
   [src/ui.rs:176](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:176), [src/inject.rs:93](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/inject.rs:93), [src/harness/claude_code.rs:215](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/claude_code.rs:215)

   Two different Edit calls on `src/lib.rs` both summarize as `Edit(src/lib.rs)`. Rating the first after the second produces an injection containing only that ambiguous preview and the **keypress time**, not the moment time, UUID, block, or tool-use ID. The agent can naturally bind it to the second/current call.

   Minimal fix: persist and render the moment timestamp, native entry/block ID, `tool_use_id`, and content/input digest. Include discriminating tool input; label reaction time separately.

9. **Default discovery can silently attach to a different working directory's agent.**  
   [src/discover.rs:127](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/discover.rs:127)

   If the current directory has any old Claude transcript, it always wins over a newer Codex session in that directory. If it has none, `all_sessions(...).next()` can choose the newest Codex session from an entirely different directory.

   This contradicts the CLI promise that the default is the newest session "for this directory" and can make every rating target the wrong run.

   Minimal fix: read Codex's persisted `session_meta.payload.cwd`, compare matching sessions across both harnesses, and require `--session` rather than silently falling back elsewhere.

10. **Store corruption and I/O errors are interpreted as an empty log.**  
    [src/ratings.rs:198](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ratings.rs:198), [src/ratings.rs:211](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ratings.rs:211)

    Concrete failures:

    - A torn multibyte preview makes `read_to_string` reject the whole ratings file, hiding every earlier valid rating.
    - The same error or a permission failure on `delivered.jsonl` makes delivery appear empty, resurrecting every historical rating.
    - A crash leaving `{"moment":{"harn` without a newline causes the next valid append to concatenate onto it; both records become one rejected line.

    Minimal fix: parse newline-delimited byte slices with `serde_json::from_slice`; only `NotFound` should mean empty. Propagate other I/O errors, fail closed in the hook, and repair/truncate an unterminated suffix under the writer lock before appending.

11. **Hostile identities are normalized into collisions instead of being rejected.**  
    [src/harness/claude_code.rs:57](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/claude_code.rs:57), [src/harness/claude_code.rs:92](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/claude_code.rs:92), [src/ratings.rs:223](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ratings.rs:223), [src/tail.rs:90](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/tail.rs:90)

    Examples:

    - Two Claude assistant lines missing `uuid` both become `S/unknown#0`; the later content overwrites the rated earlier card.
    - One entry with rateable blocks at indices 0 and 65,536 wraps both to `block == 0`.
    - Session IDs `a/b` and `a?b` both map to directory `a_b`; case-only IDs also collide on Windows.
    - Distinct invalid UTF-8 UUID bytes can both become `?` through lossy decoding.

    Minimal fix: reject missing/empty stable IDs, use a checked wider block index, use collision-free encoding/hashing for store directories, validate every loaded record against the store identity, and reject completed invalid UTF-8 lines. Equal IDs with changed content must be flagged as collisions rather than blindly overwritten.

## Medium / low

12. **Tool results are systematically dropped during live parsing.**  
    [src/harness/claude_code.rs:26](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/claude_code.rs:26), [src/harness/claude_code.rs:171](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/claude_code.rs:171), [src/harness/codex.rs:142](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/codex.rs:142)

    Claude correlation state exists only within one `parse()` call. A tool call and its result arriving on different 250 ms polls cannot correlate. Codex always creates `output: None` and has no output arm; the committed call/output pair demonstrates this at [fixture line 13](/C:/Users/oasrvadmin/CLEANROOM/work/margin/fixtures/codex/session-basic.jsonl:13) and [line 14](/C:/Users/oasrvadmin/CLEANROOM/work/margin/fixtures/codex/session-basic.jsonl:14).

    Minimal fix: persistent correlation state keyed by `tool_use_id`/`call_id`, with output records updating the existing stable `Did` moment.

13. **Valid reasoning blocks and contradictory roles are dropped or mis-attributed.**  
    [src/harness/claude_code.rs:94](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/claude_code.rs:94), [src/harness/claude_code.rs:71](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/harness/claude_code.rs:71)

    A documented `{"type":"redacted_thinking","data":"..."}` block hits the wildcard and produces no thought card. [`redacted_thinking` is a valid distinct block type](https://platform.claude.com/docs/en/about-claude/models/extended-thinking-models).

    Separately, an outer `type:"user"` with inner `role:"assistant"` becomes rateable `Said`, mis-attributing human content as agent output.

    Minimal fix: typed, validated entry/block enums; map `redacted_thinking` to `Thought { text: None, bytes: data.len() }` and reject outer-type/role disagreement.

14. **`from_end` can start inside an in-flight record.**  
    [src/tail.rs:41](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/tail.rs:41)

    Start watching while EOF is `{"type":"assistant","message":`. The stored offset is raw EOF; once the writer completes the line, Tailer emits only the suffix, which the parser rejects permanently.

    Minimal fix: rewind to the incomplete line's beginning, or keep explicit `discard_until_newline` state if pre-attach partial records are intentionally excluded.

15. **An unterminated hostile line causes quadratic rereading and eventual OOM abort.**  
    [src/tail.rs:73](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/tail.rs:73)

    Append a very large record in chunks without `\n`. Offset never advances, so every poll rereads and reallocates the entire growing tail.

    Minimal fix: maintain a separate read cursor and bounded pending buffer, cap record size, and surface a visible malformed-record error.

16. **Smaller UI state errors remain.**

    - [src/ui.rs:186](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:186): `D` with "wrong file", followed by bare `f`, displays approval with the stale rejection note. Remove `notes[key]` when the new rating has no note.
    - [src/ui.rs:351](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:351): a new moment arriving while a note is typed can leave selection on history with `following == true`; the next append unexpectedly jumps. Recompute following after restoring the target.
    - [src/ui.rs:192](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:192): Codex says the agent will hear the rating, although [the hook hard-codes Claude](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:188). Make the status harness-aware.
    - [src/main.rs:295](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:295): `sent = all.len() - pending.len()` is invalid because `pending()` deduplicates re-ratings. Two undelivered ratings of one moment already display "1 sent." Count delivery revisions directly.
    - [src/ui.rs:584](/C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:584): entering note mode in a terminal three rows or shorter underflows in debug builds. Use saturating/clamped geometry.

## Verified fine

- Ordinary append-only incomplete-line handling does not duplicate bytes; the offset remains before the partial line, and split multibyte characters survive.
- With exactly one TUI and one non-overlapping hook, they append to different files, so their raw writes do not contend.
- No finite malformed JSON/text input reaches a production parser panic; JSON/type access is guarded, and Claude's result index remains valid. The hostile availability failure is the unbounded-line OOM path above.
- CRLF stripping, Unicode preview clipping, selection bounds checks, and the Windows key press/release filter are sound.
- Current working-tree code reloads persisted rating marks at startup/store rebinding; that earlier concern is fixed.
- Marking before stdout is an explicit at-most-once tradeoff, so I did not report that crash window itself. Ignoring a failed mark is the actual duplication bug.

I made no workspace changes. Tests were not run because the provided filesystem was read-only.
