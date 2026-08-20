//! Codex rollout parser.
//!
//! Reads `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`.
//!
//! Shape, measured against CLI 0.146.0:
//!
//! ```jsonc
//! { "timestamp": "…", "type": "session_meta", "payload": { "id": "01a01e…", … } }
//! { "timestamp": "…", "type": "event_msg", "payload": { "type": "agent_reasoning", "text": "**Planning the parser change**" } }
//! { "timestamp": "…", "type": "event_msg", "payload": { "type": "agent_message", "message": "…" } }
//! { "timestamp": "…", "type": "response_item", "payload": { "type": "function_call", "name": "shell", "arguments": "…" } }
//! ```
//!
//! Unlike Claude Code, Codex does persist readable reasoning, but only the short summary
//! headlines its UI shows. The full chain sits in `reasoning.encrypted_content` and is not
//! recoverable. Two behaviours make reasoning vanish entirely, and both look like a healthy
//! rollout, so `reasoning_health` exists to tell them apart:
//!
//!   1. `model_reasoning_summary = "none"` suppresses every summary
//!   2. `codex exec` never emits them regardless of that setting

use crate::moment::{Harness, Moment, MomentId, MomentKind};
use serde_json::Value;

/// Why a rollout contains no reasoning, so the UI can say which rather than showing a blank
/// column and letting the user assume the model went quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningHealth {
    /// Summaries present.
    Present,
    /// The model reasoned, but summaries were suppressed. Detected by encrypted `reasoning`
    /// items with no matching `agent_reasoning` events.
    SuppressedBySetting,
    /// No sign of reasoning at all: a `codex exec` run, or a turn that needed none.
    NotEmitted,
}

pub fn reasoning_health(input: &str) -> ReasoningHealth {
    let mut summaries = 0usize;
    let mut encrypted = 0usize;
    for line in input.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        match payload_type(&v) {
            Some("agent_reasoning") => summaries += 1,
            Some("reasoning") => encrypted += 1,
            _ => {}
        }
    }
    match (summaries, encrypted) {
        (0, 0) => ReasoningHealth::NotEmitted,
        (0, _) => ReasoningHealth::SuppressedBySetting,
        _ => ReasoningHealth::Present,
    }
}

pub fn parse(input: &str) -> Vec<Moment> {
    let mut out = Vec::new();
    let mut seq = 0usize;
    let mut session_id = String::from("unknown");

    for (line_no, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };

        let at = v.get("timestamp").and_then(Value::as_str).map(str::to_string);
        let Some(payload) = v.get("payload") else { continue };

        // session_meta is typed at the entry level, not inside payload, unlike every other
        // event. Checking only payload.type silently loses the session id, which then
        // poisons every anchor in the file.
        if v.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(id) = payload
                .get("id")
                .or_else(|| payload.get("session_id"))
                .and_then(Value::as_str)
            {
                session_id = id.to_string();
            }
            continue;
        }

        let Some(ptype) = payload.get("type").and_then(Value::as_str) else { continue };

        // Codex events carry no per-event id, so identity is the line number. Rollouts are
        // append-only and never rewritten, which is what makes that safe here.
        let entry = format!("L{line_no}");

        let kind = match ptype {
            "agent_reasoning" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                MomentKind::Thought { bytes: text.len(), text: Some(text.to_string()) }
            }
            "agent_message" => {
                let text = payload
                    .get("message")
                    .or_else(|| payload.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                MomentKind::Said { text: text.to_string() }
            }
            "user_message" => {
                let text = payload
                    .get("message")
                    .or_else(|| payload.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                MomentKind::Asked { text: text.to_string() }
            }
            "function_call" | "custom_tool_call" => {
                let tool = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let input = payload
                    .get("arguments")
                    .or_else(|| payload.get("input"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                MomentKind::Did {
                    tool,
                    input,
                    output: None,
                    tool_use_id: payload.get("call_id").and_then(Value::as_str).map(str::to_string),
                }
            }
            _ => continue,
        };

        out.push(Moment { id: MomentId::new(Harness::Codex, &session_id, entry, 0), seq, at, kind });
        seq += 1;
    }

    // session_meta is the first line, so anything parsed before it would have been stamped
    // "unknown". Backfill rather than requiring a second pass over the file.
    for m in &mut out {
        if m.id.session_id == "unknown" {
            m.id.session_id = session_id.clone();
        }
    }
    out
}

fn payload_type(v: &Value) -> Option<&str> {
    v.get("payload")?.get("type")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC: &str = include_str!("../../fixtures/codex/session-basic.jsonl");
    const REASONING: &str = include_str!("../../fixtures/codex/session-reasoning.jsonl");

    #[test]
    fn parses_both_real_fixtures() {
        assert!(!parse(BASIC).is_empty(), "basic fixture produced no moments");
        assert!(!parse(REASONING).is_empty(), "reasoning fixture produced no moments");
    }

    /// Codex does what Claude Code cannot: readable reasoning.
    #[test]
    fn codex_reasoning_is_readable() {
        let thoughts: Vec<_> = parse(REASONING)
            .into_iter()
            .filter_map(|m| match m.kind {
                MomentKind::Thought { text, .. } => text,
                _ => None,
            })
            .collect();

        assert!(thoughts.len() > 50, "expected many reasoning summaries, got {}", thoughts.len());
        assert!(thoughts.iter().all(|t| !t.trim().is_empty()));
    }

    /// The two ways reasoning disappears must be distinguishable, or the UI cannot tell the
    /// user which knob to turn.
    #[test]
    fn reasoning_health_separates_absent_from_suppressed() {
        assert_eq!(reasoning_health(REASONING), ReasoningHealth::Present);
        // codex exec: no summaries and no encrypted reasoning items either
        assert_eq!(reasoning_health(BASIC), ReasoningHealth::NotEmitted);
        // summaries off, but the model still reasoned
        let suppressed = r#"{"timestamp":"t","type":"response_item","payload":{"type":"reasoning","encrypted_content":"x"}}"#;
        assert_eq!(reasoning_health(suppressed), ReasoningHealth::SuppressedBySetting);
    }

    #[test]
    fn session_id_is_backfilled_onto_every_moment() {
        let moments = parse(REASONING);
        assert!(!moments.is_empty());
        assert!(
            moments.iter().all(|m| m.id.session_id != "unknown"),
            "some moments were left without a session id"
        );
    }

    #[test]
    fn ids_are_unique() {
        let moments = parse(REASONING);
        let mut ids: Vec<_> = moments.iter().map(|m| m.id.to_string()).collect();
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), before, "moment ids collided");
    }

    #[test]
    fn a_torn_final_line_does_not_lose_the_lines_before_it() {
        let torn = format!("{BASIC}\n{{\"timestamp\":\"t\",\"payload\":{{\"ty");
        assert_eq!(parse(&torn).len(), parse(BASIC).len());
    }

    #[test]
    fn garbage_input_yields_nothing_rather_than_panicking() {
        assert!(parse("").is_empty());
        assert!(parse("not json\n{]\n").is_empty());
        assert_eq!(reasoning_health(""), ReasoningHealth::NotEmitted);
    }
}
