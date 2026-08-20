//! Claude Code transcript parser.
//!
//! Reads `~/.claude/projects/<slug>/<session-id>.jsonl`, appended live during a session.
//!
//! Shape, measured against 2.1.233 rather than taken from documentation:
//!
//! ```jsonc
//! { "type": "assistant",
//!   "uuid": "6de117ee-…", "parentUuid": "f3629c09-…",
//!   "timestamp": "2026-08-20T07:03:51.196Z", "sessionId": "…",
//!   "message": { "role": "assistant", "model": "claude-opus-5", "content": [
//!       { "type": "thinking", "thinking": "", "signature": "CAISlRoKowEIEBgCKkA…" },
//!       { "type": "text",     "text": "…" },
//!       { "type": "tool_use", "id": "toolu_01…", "name": "Bash", "input": { … } } ] } }
//! ```
//!
//! The thinking block is the thing to know about. `thinking` is always an empty string and
//! the content sits in a 4.5 KB signature that cannot be read back. 0 readable blocks out
//! of 253 across three sessions. See `docs/FEASIBILITY.md` section 3.

use crate::moment::{Harness, Moment, MomentId, MomentKind};
use serde_json::Value;

/// Parse a whole transcript. Unparseable lines are skipped, never fatal: we are reading a
/// file another process is actively appending to, so the last line is routinely torn.
pub fn parse(input: &str) -> Vec<Moment> {
    let mut out = Vec::new();
    let mut seq = 0usize;
    // tool_use and its tool_result arrive in separate entries; remember where each call
    // landed so the result can be attached to it rather than becoming its own card.
    let mut pending: Vec<(String, usize)> = Vec::new();

    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        parse_entry(&v, &mut out, &mut seq, &mut pending);
    }
    out
}

fn parse_entry(
    v: &Value,
    out: &mut Vec<Moment>,
    seq: &mut usize,
    pending: &mut Vec<(String, usize)>,
) {
    let entry_type = v.get("type").and_then(Value::as_str).unwrap_or_default();
    if !matches!(entry_type, "assistant" | "user") {
        return; // metadata: mode, ai-title, attachment, file-history-*, queue-operation
    }

    let session_id = v
        .get("sessionId")
        .or_else(|| v.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let uuid = v.get("uuid").and_then(Value::as_str).unwrap_or("unknown");
    let at = v
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);

    let Some(message) = v.get("message") else {
        return;
    };
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or(entry_type);

    match message.get("content") {
        // A plain-string content is how a simple user turn is written.
        Some(Value::String(text)) => {
            if role == "user" && !text.trim().is_empty() {
                push(
                    out,
                    seq,
                    session_id,
                    uuid,
                    0,
                    at.clone(),
                    MomentKind::Asked { text: text.clone() },
                );
            }
        }
        Some(Value::Array(blocks)) => {
            for (i, block) in blocks.iter().enumerate() {
                let block_idx = i as u16;
                match block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "thinking" => {
                        let text = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        // Size of the signature is the only honest measure of how much
                        // thinking happened, since the text itself is never written.
                        let bytes = block
                            .get("signature")
                            .and_then(Value::as_str)
                            .map_or(0, str::len);
                        let text = if text.trim().is_empty() {
                            None
                        } else {
                            Some(text.to_string())
                        };
                        push(
                            out,
                            seq,
                            session_id,
                            uuid,
                            block_idx,
                            at.clone(),
                            MomentKind::Thought { text, bytes },
                        );
                    }
                    "text" => {
                        let text = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if text.trim().is_empty() {
                            continue;
                        }
                        let kind = if role == "user" {
                            MomentKind::Asked {
                                text: text.to_string(),
                            }
                        } else {
                            MomentKind::Said {
                                text: text.to_string(),
                            }
                        };
                        push(out, seq, session_id, uuid, block_idx, at.clone(), kind);
                    }
                    "tool_use" => {
                        let tool = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let tool_use_id =
                            block.get("id").and_then(Value::as_str).map(str::to_string);
                        let input = summarise_tool_input(block.get("input"));
                        let intent = block
                            .get("input")
                            .and_then(|i| i.get("description"))
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        if let Some(id) = &tool_use_id {
                            pending.push((id.clone(), out.len()));
                        }
                        push(
                            out,
                            seq,
                            session_id,
                            uuid,
                            block_idx,
                            at.clone(),
                            MomentKind::Did {
                                tool,
                                input,
                                output: None,
                                tool_use_id,
                                intent,
                            },
                        );
                    }
                    "tool_result" => {
                        let for_id = block.get("tool_use_id").and_then(Value::as_str);
                        let text = tool_result_text(block);
                        if let Some(idx) = for_id.and_then(|id| take_pending(pending, id)) {
                            if let MomentKind::Did { output, .. } = &mut out[idx].kind {
                                *output = Some(text);
                            }
                        }
                        // A result whose call was never seen is dropped rather than shown
                        // as an orphan card. It happens when a tail starts mid-session.
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn push(
    out: &mut Vec<Moment>,
    seq: &mut usize,
    session_id: &str,
    uuid: &str,
    block: u16,
    at: Option<String>,
    kind: MomentKind,
) {
    out.push(Moment {
        id: MomentId::new(Harness::ClaudeCode, session_id, uuid, block),
        seq: *seq,
        at,
        kind,
    });
    *seq += 1;
}

fn take_pending(pending: &mut Vec<(String, usize)>, id: &str) -> Option<usize> {
    let pos = pending.iter().position(|(pid, _)| pid == id)?;
    Some(pending.remove(pos).1)
}

/// Tool inputs are arbitrary JSON. Prefer the field a human would recognise, so a card
/// reads `Bash(echo hello)` rather than `Bash({"command":"echo hello","description":…})`.
fn summarise_tool_input(input: Option<&Value>) -> String {
    let Some(v) = input else { return String::new() };
    // `description` is deliberately absent: it is the agent's own human summary and is
    // carried separately as `intent`, which the row prefers. Leaving it here made the
    // fallback indistinguishable from the preferred path. `prompt` is absent because it is a
    // multi-paragraph instruction block, exactly the raw dump a row must never show.
    for key in ["command", "pattern", "file_path", "path", "query", "url"] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            if !s.trim().is_empty() {
                return s.to_string();
            }
        }
    }
    match v {
        Value::String(s) => s.clone(),
        Value::Object(map) if map.is_empty() => String::new(),
        other => other.to_string(),
    }
}

fn tool_result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../fixtures/claude-code/session-basic.jsonl");

    #[test]
    fn parses_the_real_fixture() {
        let moments = parse(FIXTURE);
        assert!(
            !moments.is_empty(),
            "fixture produced no moments; the format has drifted"
        );
    }

    /// The finding this whole project had to design around. If this test ever starts
    /// failing because thinking has text, that is good news and the UI should be revisited.
    #[test]
    fn claude_code_thinking_is_never_readable() {
        let moments = parse(FIXTURE);
        let thoughts: Vec<_> = moments
            .iter()
            .filter_map(|m| match &m.kind {
                MomentKind::Thought { text, bytes } => Some((text, *bytes)),
                _ => None,
            })
            .collect();

        assert!(
            !thoughts.is_empty(),
            "fixture should contain thinking blocks"
        );
        for (text, bytes) in &thoughts {
            assert!(
                text.is_none(),
                "thinking text is unexpectedly readable: {text:?}"
            );
            assert!(
                *bytes > 0,
                "a thinking block should still report signature size"
            );
        }
    }

    #[test]
    fn tool_result_attaches_to_its_call_instead_of_becoming_a_card() {
        let moments = parse(FIXTURE);
        let dids: Vec<_> = moments
            .iter()
            .filter(|m| matches!(m.kind, MomentKind::Did { .. }))
            .collect();
        assert_eq!(dids.len(), 1, "fixture has exactly one tool call");

        let MomentKind::Did {
            tool,
            input,
            output,
            tool_use_id,
            intent,
        } = &dids[0].kind
        else {
            unreachable!()
        };
        assert_eq!(tool, "Bash");
        assert_eq!(input, "echo hello");
        assert!(tool_use_id.as_deref().unwrap().starts_with("toolu_"));
        assert!(
            output.as_deref().unwrap().contains("hello"),
            "result should be attached"
        );
        // The agent's own description is kept. It is a better row label than anything
        // recoverable by parsing the command, and it was previously discarded because the
        // input summariser read `command` first and stopped there.
        assert_eq!(intent.as_deref(), Some("Run echo hello command"));
    }

    #[test]
    fn ids_are_unique_and_stable_across_reparses() {
        let a = parse(FIXTURE);
        let b = parse(FIXTURE);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(
                x.id, y.id,
                "the same transcript must yield the same anchors"
            );
        }
        let mut ids: Vec<_> = a.iter().map(|m| m.id.to_string()).collect();
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), before, "moment ids collided");
    }

    #[test]
    fn a_torn_final_line_does_not_lose_the_lines_before_it() {
        // The normal state of a file being appended to by another process.
        let torn = format!("{FIXTURE}\n{{\"type\":\"assistant\",\"message\":{{\"cont");
        assert_eq!(parse(&torn).len(), parse(FIXTURE).len());
    }

    #[test]
    fn garbage_input_yields_nothing_rather_than_panicking() {
        assert!(parse("").is_empty());
        assert!(parse("not json\n{]\n").is_empty());
        assert!(parse("{\"type\":\"assistant\"}").is_empty());
    }
}
