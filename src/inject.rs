//! Turning ratings into something a running agent actually acts on.
//!
//! The mechanism is proven (`docs/FEASIBILITY.md` section 2): a hook returning
//! `hookSpecificOutput.additionalContext` reaches a live turn and the agent responds to it
//! without the user typing and without the turn being interrupted.
//!
//! The mechanism is the easy half. The wording is the hard half, because a naive dump has
//! four predictable failure modes:
//!
//! | failure | what it looks like | design answer |
//! |---|---|---|
//! | treats it as a user turn | agent stops and replies "thanks for the feedback" | say explicitly this is not a turn and needs no reply |
//! | over-correction | one thumbs-down on a grep and the agent abandons a sound plan | scope the signal to the moment, ask for adjustment not restart |
//! | sycophancy | agent starts narrating to fish for approval | forbid seeking approval, forbid mentioning the signal |
//! | context poisoning | the same complaint re-injected every tool call | deliver once, enforced by the store |
//!
//! Positive and negative are deliberately separated. "Keep doing X" and "stop doing Y" are
//! different instructions and blur into noise when interleaved.

use crate::moment::MomentKind;
use crate::ratings::{Rating, Verdict};

/// How many ratings ride in one injection.
///
/// Past a handful the agent starts weighing them as a list to work through rather than a
/// signal to absorb, and the newest, most relevant one gets buried. Overflow is not lost:
/// it stays pending and lands at the next tool call.
pub const MAX_PER_INJECTION: usize = 6;

/// When it is worth interrupting nothing to say something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// After a tool call completes. The main path: frequent during real work, and the agent
    /// is between actions rather than mid-thought.
    PostToolUse,
    /// The agent is about to finish. Last chance to land feedback, and the one that matters
    /// during an unattended `/goal` run where nobody is about to type anything.
    Stop,
    /// Folded into the human's next turn.
    UserPromptSubmit,
}

impl Trigger {
    pub fn hook_event_name(self) -> &'static str {
        match self {
            Trigger::PostToolUse => "PostToolUse",
            Trigger::Stop => "Stop",
            Trigger::UserPromptSubmit => "UserPromptSubmit",
        }
    }
}

/// Render the block of text handed to the agent.
///
/// Returns None when there is nothing pending, and the caller must then emit no output at
/// all. Staying silent in the common case is what keeps this free: a hook that always says
/// something trains the agent to skim past it.
pub fn render(ratings: &[Rating], trigger: Trigger) -> Option<String> {
    if ratings.is_empty() {
        return None;
    }

    // Newest first: if the cap bites, the most recent judgment is the one that survives.
    let mut ordered: Vec<&Rating> = ratings.iter().collect();
    ordered.sort_by(|a, b| b.at.cmp(&a.at));
    let shown: Vec<&Rating> = ordered.into_iter().take(MAX_PER_INJECTION).collect();

    let (up, down): (Vec<&Rating>, Vec<&Rating>) =
        shown.iter().partition(|r| r.verdict == Verdict::Up);

    let mut s = String::new();
    s.push_str("<margin-signal>\n");
    s.push_str(
        "The user reacted to specific moments of this run, from a side channel. \
         This is a signal, not a message in the conversation.\n\n",
    );

    if !up.is_empty() {
        s.push_str("APPROVED, do more of this:\n");
        for r in &up {
            push_item(&mut s, r);
        }
        s.push('\n');
    }

    if !down.is_empty() {
        s.push_str("REJECTED, do less of this:\n");
        for r in &down {
            push_item(&mut s, r);
        }
        s.push('\n');
    }

    s.push_str("How to use this:\n");
    s.push_str(
        "- Infer the general behaviour behind each reaction, not just the single moment. \
         The moment is an example of a preference, not the whole of it.\n",
    );
    s.push_str(
        "- Apply it from here on. Adjust your current approach; do not restart work that \
         is already sound.\n",
    );
    s.push_str("- Do not reply to this, do not thank the user, do not mention that you received it.\n");
    s.push_str("- Do not seek further approval or narrate in the hope of more of it.\n");

    if trigger == Trigger::Stop {
        s.push_str(
            "- You are about to finish. If a rejection above means the work is not actually \
             done, keep going instead of stopping.\n",
        );
    }

    s.push_str("</margin-signal>");
    Some(s)
}

fn push_item(s: &mut String, r: &Rating) {
    let when = short_time(&r.at);
    let what = r.preview.as_deref().unwrap_or("<no preview captured>");
    s.push_str(&format!("  - [{when}] {what}\n"));
    if let Some(note) = r.note.as_deref().filter(|n| !n.trim().is_empty()) {
        s.push_str(&format!("    the user said: \"{}\"\n", note.trim()));
    }
}

/// `2026-08-20T12:04:19.412Z` becomes `12:04:19`. Falls back to the raw string rather than
/// dropping a timestamp we failed to recognise.
fn short_time(rfc3339: &str) -> &str {
    rfc3339
        .split_once('T')
        .map(|(_, t)| t.split(['.', 'Z', '+']).next().unwrap_or(t))
        .unwrap_or(rfc3339)
}

/// The JSON a hook prints on stdout for Claude Code to pick up.
pub fn hook_output(context: &str, trigger: Trigger) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": trigger.hook_event_name(),
            "additionalContext": context,
        }
    })
    .to_string()
}

/// A short label for the card, used when a rating has no preview of its own.
pub fn describe(kind: &MomentKind) -> String {
    match kind {
        MomentKind::Said { .. } => "said something".into(),
        MomentKind::Asked { .. } => "the user's message".into(),
        MomentKind::Did { tool, .. } => format!("the {tool} call"),
        MomentKind::Thought { .. } => "a step of reasoning".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moment::{Harness, MomentId};

    fn r(entry: &str, verdict: Verdict, at: &str, note: Option<&str>, preview: &str) -> Rating {
        Rating {
            moment: MomentId::new(Harness::ClaudeCode, "sess", entry, 0),
            verdict,
            note: note.map(str::to_string),
            at: at.to_string(),
            preview: Some(preview.to_string()),
        }
    }

    #[test]
    fn nothing_pending_means_nothing_is_said() {
        assert!(render(&[], Trigger::PostToolUse).is_none());
    }

    #[test]
    fn approvals_and_rejections_are_kept_apart() {
        let ratings = vec![
            r("a", Verdict::Up, "2026-08-20T12:04:11Z", None, "checked the format before writing the parser"),
            r("b", Verdict::Down, "2026-08-20T12:04:19Z", Some("wrong file, use debug"), "Bash(grep -c thinking)"),
        ];
        let out = render(&ratings, Trigger::PostToolUse).unwrap();

        let approved = out.find("APPROVED").unwrap();
        let rejected = out.find("REJECTED").unwrap();
        assert!(approved < rejected, "approvals should come first");

        // each item lands in its own section
        let up_section = &out[approved..rejected];
        assert!(up_section.contains("checked the format"));
        assert!(!up_section.contains("grep -c thinking"));

        assert!(out.contains("wrong file, use debug"), "the note is the durable artifact");
        assert!(out.contains("12:04:19"), "timestamps are shortened, not dropped");
    }

    /// Guards against the four failure modes named at the top of this module.
    #[test]
    fn every_guard_clause_is_present() {
        let out = render(&[r("a", Verdict::Down, "2026-08-20T12:00:00Z", None, "x")], Trigger::PostToolUse)
            .unwrap();
        assert!(out.contains("not a message in the conversation"), "must not read as a user turn");
        assert!(out.contains("Do not reply"), "must not stop to acknowledge");
        assert!(out.contains("do not restart work that is already sound"), "must not over-correct");
        assert!(out.contains("Do not seek further approval"), "must not become sycophantic");
        assert!(out.contains("Infer the general behaviour"), "must generalise, not overfit");
    }

    #[test]
    fn the_stop_trigger_adds_the_keep_going_clause() {
        let one = [r("a", Verdict::Down, "2026-08-20T12:00:00Z", None, "x")];
        assert!(render(&one, Trigger::Stop).unwrap().contains("keep going instead of stopping"));
        assert!(!render(&one, Trigger::PostToolUse).unwrap().contains("keep going instead of stopping"));
    }

    #[test]
    fn overflow_keeps_the_newest_and_drops_the_stalest() {
        let many: Vec<Rating> = (0..MAX_PER_INJECTION + 3)
            .map(|i| {
                r(
                    &format!("m{i}"),
                    Verdict::Up,
                    &format!("2026-08-20T12:00:{:02}Z", i),
                    None,
                    &format!("moment number {i}"),
                )
            })
            .collect();

        let out = render(&many, Trigger::PostToolUse).unwrap();
        let shown = out.matches("moment number").count();
        assert_eq!(shown, MAX_PER_INJECTION);
        assert!(out.contains(&format!("moment number {}", MAX_PER_INJECTION + 2)), "newest must survive");
        assert!(!out.contains("moment number 0"), "stalest should be the one dropped");
    }

    #[test]
    fn hook_output_is_the_shape_claude_code_expects() {
        let json: serde_json::Value =
            serde_json::from_str(&hook_output("hello", Trigger::PostToolUse)).unwrap();
        assert_eq!(json["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(json["hookSpecificOutput"]["additionalContext"], "hello");
    }

    #[test]
    fn a_malformed_timestamp_is_shown_rather_than_dropped() {
        assert_eq!(short_time("2026-08-20T12:04:19.412Z"), "12:04:19");
        assert_eq!(short_time("2026-08-20T12:04:19+02:00"), "12:04:19");
        assert_eq!(short_time("whenever"), "whenever");
    }
}
