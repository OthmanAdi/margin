//! Print the text an agent actually receives, so the wording can be reviewed by eye.
//! `cargo run --example preview_injection`

use margin::inject::{render, Trigger};
use margin::moment::{Harness, MomentId};
use margin::ratings::{Rating, Verdict};

fn main() {
    let ratings = vec![
        Rating {
            moment: MomentId::new(Harness::ClaudeCode, "sess", "6de117ee", 0),
            verdict: Verdict::Up,
            note: None,
            at: "2026-08-20T12:04:11Z".into(),
            preview: Some("I'll check the transcript format first before writing the parser.".into()),
        },
        Rating {
            moment: MomentId::new(Harness::ClaudeCode, "sess", "85b72af4", 2),
            verdict: Verdict::Down,
            note: Some("wrong file, the thinking is not in debug logs".into()),
            at: "2026-08-20T12:04:19Z".into(),
            preview: Some("Bash(grep -c thinking ~/.claude/debug/*.txt)".into()),
        },
    ];

    for trigger in [Trigger::PostToolUse, Trigger::Stop] {
        println!("=== {} ===", trigger.hook_event_name());
        println!("{}\n", render(&ratings, trigger).unwrap());
    }
}
