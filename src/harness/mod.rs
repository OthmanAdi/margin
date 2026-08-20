//! One parser per harness. They share `Moment` and nothing else.
//!
//! No trait, deliberately. Two implementations that never dispatch dynamically do not need
//! an interface; the third harness can introduce one if it actually disagrees.

pub mod claude_code;
pub mod codex;

use crate::moment::{Harness, Moment};

/// Parse a transcript for a known harness.
pub fn parse(harness: Harness, input: &str) -> Vec<Moment> {
    parse_at(harness, input, 0)
}

/// Parse a chunk beginning at absolute line `first_line`.
///
/// Claude Code ignores the offset because its entries carry their own `uuid`. Codex needs
/// it: its identities are line numbers, and a live tail hands over only the new lines, so
/// counting from zero per chunk makes ids collide across polls.
pub fn parse_at(harness: Harness, input: &str, first_line: usize) -> Vec<Moment> {
    match harness {
        Harness::ClaudeCode => claude_code::parse(input),
        Harness::Codex => codex::parse_at(input, first_line),
    }
}

/// Guess the harness from a transcript's own contents.
///
/// Path-based detection breaks the moment someone points margin at a copied file, and the
/// two formats are unambiguous: Claude Code writes bare `"type":"assistant"` entries, Codex
/// nests everything under `"payload"`.
pub fn detect(input: &str) -> Option<Harness> {
    for line in input.lines().take(50) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("payload").is_some() {
            return Some(Harness::Codex);
        }
        if v.get("uuid").is_some() && v.get("message").is_some() {
            return Some(Harness::ClaudeCode);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_harness_from_content_alone() {
        assert_eq!(
            detect(include_str!(
                "../../fixtures/claude-code/session-basic.jsonl"
            )),
            Some(Harness::ClaudeCode)
        );
        assert_eq!(
            detect(include_str!("../../fixtures/codex/session-basic.jsonl")),
            Some(Harness::Codex)
        );
        assert_eq!(
            detect(include_str!("../../fixtures/codex/session-reasoning.jsonl")),
            Some(Harness::Codex)
        );
    }

    #[test]
    fn declines_to_guess_on_unknown_input() {
        assert_eq!(detect(""), None);
        assert_eq!(detect("{\"hello\":1}"), None);
    }
}
