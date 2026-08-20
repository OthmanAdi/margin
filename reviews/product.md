codex gpt-5.6-sol/ultra  read-only  dir=C:/Users/oasrvadmin/CLEANROOM/work/margin
done in 430.4s  exit=0  tokens in/out=543678/13264
SESSION: 01a01f2f-ac14-72a2-adfa-ca8e4fd0069e
LOG: C:\Users\oasrvadmin\CLEANROOM\logs\20260820-144012-review-product.jsonl

Blunt verdict: this is demoable plumbing, not yet a usable interaction. The proof shows that a correctly written rating record can influence one Claude run. It does not show that a human can reliably create that record with the advertised gesture.

Impact order:

1. The focus/input model invalidates the core promise.
2. Rating keys can be lost or applied to the wrong moment.
3. Session lifecycle makes every run a manual reattachment exercise.
4. The UI claims delivery it cannot verify.
5. The README sells unfinished and unproved behavior as current product behavior.

## 1. Real first-time session

1. The user runs `cargo install margin`, then `margin install --write`.

   If their existing Claude settings are malformed, the installer silently treats them as empty JSON and overwrites them, despite claiming to merge safely ([main.rs:233](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:233), [main.rs:238](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:238)). That is a nasty first impression for a feedback tool.

2. They open a second pane and run `margin watch`.

   If Claude has not produced a transcript yet, margin exits with "no session found" ([main.rs:99](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:99)). So margin cannot simply be parked before work starts.

3. They start the agent, make it do enough work to create a transcript, and retry.

   Margin now tails from EOF by default ([ui.rs:182](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:182)). Everything that happened before `margin watch` is hidden. The first useful screen can therefore say "Waiting for the agent" while a conversation is already visible next door. The README never tells the user about `--replay`.

4. They switch focus back to the agent pane and work. A bad moment appears in margin.

   To rate it, they must:

   - switch focus to margin;
   - possibly press `k` several times;
   - press `d`;
   - switch focus back.

   That is not one keystroke. Margin does not steal focus automatically; it demands focus manually.

5. While they are switching, two races exist:

   - A new moment automatically moves selection to the newest row ([ui.rs:113](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:113)). Their `d` can rate a different moment from the one they looked at.
   - If a transcript change is queued first, the event loop drains every queued signal, including keyboard events ([ui.rs:279](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:279)). Their `d` can simply disappear.

6. If they follow the README's proof example-"press `d`, type a reason"-lowercase `d` records immediately. It does not enter note mode. The following text is treated as browsing hotkeys or ignored; only uppercase `D` opens the note prompt ([README.md:79](C:/Users/oasrvadmin/CLEANROOM/work/margin/README.md:79), [ui.rs:349](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:349)).

7. If the rating is saved, the UI says "the agent hears it at its next tool call" ([ui.rs:153](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:153)). All it actually knows is that a local append succeeded. It has not verified that the hook is installed, fired, or delivered anything. For Codex, that message is definitely false.

8. On the next agent session, margin remains bound to the old transcript. It must be quit and relaunched. When reopened, its `verdicts` and `notes` maps start empty, so existing ratings visually disappear and the header returns to "0 rated" ([ui.rs:195](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:195)).

That is where users give up: either at the mysteriously empty first screen, the pane-switch tax, the first swallowed/mistargeted rating, or the first time the agent does not visibly react despite margin claiming delivery.

## 2. Where "one keystroke, never steal focus" breaks

The design has confused "a separate pane does not intercept the agent's keys" with "the agent pane keeps focus." Those are mutually incompatible here.

Margin receives keys only through `crossterm::event::read()` in its own terminal ([ui.rs:219](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:219)). A non-focused terminal pane does not receive `f` or `d`. The design document nevertheless declares the focus rule satisfied merely because the panes are separate ([DESIGN.md:33](C:/Users/oasrvadmin/CLEANROOM/work/margin/docs/DESIGN.md:33)).

Even after focusing margin, the interaction is unreliable:

- File activity can discard the key.
- Auto-follow can retarget the key.
- Selecting an older moment costs navigation keys.
- The behaviorally demonstrated rejection requires `Shift+D`, a sentence, and Enter-not one tap.

So the implementation has a one-key `rate()` function. It does not have a one-keystroke user workflow.

## 3. Single highest-impact day-to-day gap

A trustworthy, focus-preserving input path bound to the exact moment the user saw.

That is the missing product. Without it, every rating pays a pane-switch tax and can be lost or misapplied. Nothing else matters until that is true.

If that is counted as part of question 2, the next-largest gap is session lifecycle: margin cannot wait for the next session, switch transcripts, or remain parked all day. It behaves like a per-demo viewer, not a persistent companion.

## 4. README overclaims

Ranked by severity:

1. **"One keystroke, no interruption."** False end to end for the focus and event-loop reasons above ([README.md:5](C:/Users/oasrvadmin/CLEANROOM/work/margin/README.md:5)).

2. **The proof is presented as proof of the interaction.** It is not. The proof writes `ratings.jsonl` directly rather than exercising the TUI ([live_proof.sh:86](C:/Users/oasrvadmin/CLEANROOM/work/margin/research/live_proof.sh:86)). It proves injection plumbing. It also uses a detailed prescriptive note, while explicitly admitting that a bare rejection is not proved reliable ([PROOF.md:118](C:/Users/oasrvadmin/CLEANROOM/work/margin/docs/PROOF.md:118)).

3. **"Ratings do three things."** Carry-over and eval export are described in the present tense and drawn as active data flows ([README.md:64](C:/Users/oasrvadmin/CLEANROOM/work/margin/README.md:64)), then admitted to be unbuilt at the bottom ([README.md:199](C:/Users/oasrvadmin/CLEANROOM/work/margin/README.md:199)). There is no export command or SessionStart installation path in `main.rs`.

4. **"Delivered exactly once."** The implementation is at-most-once. It marks ratings delivered before printing the hook response and explicitly accepts losing them in that gap ([main.rs:172](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:172)). Worse, rendering caps an injection at five ratings, but the hook marks every pending rating delivered, so overflow is silently discarded ([inject.rs:75](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/inject.rs:75), [main.rs:167](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/main.rs:167)).

5. **"The agent finds out . and adjusts."** That is asserted after a local file write. Adjustment is model behavior, not something the code guarantees. Codex injection is not implemented, yet Codex gets the same success message.

6. **"margin opens a pane."** It does not. It initializes a TUI in whichever pane the user created ([ui.rs:254](C:/Users/oasrvadmin/CLEANROOM/work/margin/src/ui.rs:254)). The README itself later tells the user to create that pane.

7. **"Every moment / what it thought."** Terminal Claude thoughts are opaque timing-and-size placeholders. They are still selectable and rateable even though the user cannot know what they contain.

## 5. What I would cut

- Cut Codex from the rateable product until ratings can actually steer Codex. Rate-to-nowhere is precisely the diary the README attacks.
- Cut carry-over and eval export from the current product story. They are separate future jobs, and they are not built.
- Cut opaque Claude thought placeholders from the selectable/rateable stream. `<not persisted, 4524 B>` is not something a human can judge.
- Hide the `snapshot` command; it is repository maintenance, not user functionality.
- If scope must shrink further, cut positive ratings before detailed rejection. There is no behavioral evidence that a bare `f` meaningfully improves a live run.

Do not cut `D` plus a reason. Ironically, that is the only interaction the evidence supports. Cut the claim that it is equivalent to a bare one-key rating.

The credible v1 is much narrower: reliably reject one exact Claude Code moment during the current run. The current build does not yet do that reliably.
