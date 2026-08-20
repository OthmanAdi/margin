//! The one model both harnesses collapse into.
//!
//! A `Moment` is anything a human might want to react to: something the agent said, did,
//! or thought. Ratings anchor to `MomentId`, so that identity has to survive a reparse of
//! the same transcript, and has to keep meaning the same thing after the file grows.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    ClaudeCode,
    Codex,
}

impl Harness {
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::ClaudeCode => "claude-code",
            Harness::Codex => "codex",
        }
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable identity for one rateable moment.
///
/// `entry` is the harness's own id for the transcript entry: Claude Code's `uuid`, or a
/// synthesised `<session>:<line>` for Codex, whose events carry no per-event id. `block`
/// disambiguates several blocks inside one entry, since a single assistant message can hold
/// a thought, some prose, and a tool call at once.
///
/// Deliberately not a line offset. Transcripts are append-only today, but a rating that
/// silently retargets if a line is ever inserted is worse than a rating that goes missing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MomentId {
    pub harness: String,
    pub session_id: String,
    pub entry: String,
    pub block: u16,
}

impl MomentId {
    pub fn new(harness: Harness, session_id: impl Into<String>, entry: impl Into<String>, block: u16) -> Self {
        Self { harness: harness.as_str().to_string(), session_id: session_id.into(), entry: entry.into(), block }
    }
}

impl fmt::Display for MomentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}#{}", self.harness, self.session_id, self.entry, self.block)
    }
}

/// What kind of thing happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MomentKind {
    /// The human's turn. Not rateable, but shown for context.
    Asked { text: String },

    /// The agent's prose.
    Said { text: String },

    /// A tool call. `output` is None until the result lands, which is a real state during a
    /// live tail: the call is visible before it has returned.
    Did {
        tool: String,
        input: String,
        output: Option<String>,
        tool_use_id: Option<String>,
    },

    /// Reasoning.
    ///
    /// `text` is None when the harness did not persist it, which is always the case on
    /// Claude Code. That is the honest representation and the UI depends on it: a card with
    /// no text renders as timing plus size, never as an empty quote. See
    /// `docs/FEASIBILITY.md` section 3.
    Thought { text: Option<String>, bytes: usize },
}

impl MomentKind {
    /// Whether a human can meaningfully rate this.
    pub fn rateable(&self) -> bool {
        !matches!(self, MomentKind::Asked { .. })
    }

    pub fn label(&self) -> &'static str {
        match self {
            MomentKind::Asked { .. } => "asked",
            MomentKind::Said { .. } => "said",
            MomentKind::Did { .. } => "did",
            MomentKind::Thought { .. } => "thought",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Moment {
    pub id: MomentId,
    /// Order of first appearance in the transcript. Display order, never identity.
    pub seq: usize,
    /// RFC3339 as the harness wrote it. Kept verbatim so a malformed timestamp cannot drop
    /// an otherwise good moment.
    pub at: Option<String>,
    pub kind: MomentKind,
}

impl Moment {
    /// One-line summary for a card, collapsed to a single line and clipped to `width`.
    pub fn preview(&self, width: usize) -> String {
        let raw = match &self.kind {
            MomentKind::Asked { text } | MomentKind::Said { text } => text.clone(),
            MomentKind::Did { tool, input, .. } => format!("{tool}({input})"),
            MomentKind::Thought { text: Some(t), .. } => t.clone(),
            MomentKind::Thought { text: None, bytes } => format!("<not persisted, {bytes} B>"),
        };
        clip(&collapse_ws(&raw), width)
    }
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    out.trim().to_string()
}

/// Clip by chars, not bytes, so a multi-byte character is never split.
fn clip(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let n = s.chars().count();
    if n <= width {
        return s.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(width - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Moment {
        Moment {
            id: MomentId::new(Harness::ClaudeCode, "s", "e", 0),
            seq: 0,
            at: None,
            kind: MomentKind::Said { text: text.to_string() },
        }
    }

    #[test]
    fn preview_collapses_newlines_and_runs_of_space() {
        let m = said("line one\n\n  line   two");
        assert_eq!(m.preview(80), "line one line two");
    }

    #[test]
    fn preview_clips_with_ellipsis() {
        let m = said("abcdefghij");
        assert_eq!(m.preview(5), "abcd…");
        assert_eq!(m.preview(10), "abcdefghij");
        assert_eq!(m.preview(1), "…");
        assert_eq!(m.preview(0), "");
    }

    #[test]
    fn preview_never_splits_a_multibyte_char() {
        // Clipping by bytes here would produce invalid UTF-8 and panic.
        let m = said("héllo wörld ✅ ünïcode");
        let out = m.preview(8);
        assert_eq!(out.chars().count(), 8);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn unpersisted_thought_says_so_rather_than_showing_an_empty_quote() {
        let m = Moment {
            id: MomentId::new(Harness::ClaudeCode, "s", "e", 0),
            seq: 0,
            at: None,
            kind: MomentKind::Thought { text: None, bytes: 4524 },
        };
        assert_eq!(m.preview(80), "<not persisted, 4524 B>");
    }

    #[test]
    fn the_human_turn_is_not_rateable() {
        assert!(!MomentKind::Asked { text: "hi".into() }.rateable());
        assert!(MomentKind::Said { text: "hi".into() }.rateable());
        assert!(MomentKind::Thought { text: None, bytes: 0 }.rateable());
    }

    #[test]
    fn moment_id_round_trips_through_json() {
        let id = MomentId::new(Harness::Codex, "sess-1", "entry-9", 2);
        let back: MomentId = serde_json::from_str(&serde_json::to_string(&id).unwrap()).unwrap();
        assert_eq!(id, back);
        assert_eq!(id.to_string(), "codex/sess-1/entry-9#2");
    }
}
