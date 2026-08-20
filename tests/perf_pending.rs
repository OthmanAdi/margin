//! Q3 from the stress-test pass: `Store::pending` in `src/ratings.rs` re-reads and
//! reparses both `ratings.jsonl` and `delivered.jsonl` from disk on every call, and
//! `main.rs`'s `hook()` calls it on every `PostToolUse` event, i.e. every tool call the
//! agent makes. Unlike Q2/Q5, `Store` and `pending()` are fully public, so this exercises
//! the real production function directly rather than a mirror of it.

use margin::moment::{Harness, MomentId};
use margin::ratings::{Rating, Store, Verdict};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

fn tmp_root(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "margin-perf-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&base).unwrap();
    base
}

fn rating(i: usize, verdict: Verdict) -> Rating {
    Rating {
        moment: MomentId::new(
            Harness::ClaudeCode,
            "session-abc123",
            format!("entry-{i:08}"),
            0,
        ),
        verdict,
        note: None,
        at: "2026-08-20T12:00:00Z".to_string(),
        preview: Some(format!(
            "preview of moment {i}: a realistic clipped line of agent output, about this long"
        )),
        subject: Some("said".to_string()),
    }
}

/// Q3a: the common steady state. Every rating the human ever made has already been
/// delivered (the hook caught up on an earlier call), so every subsequent tool call's
/// `pending()` should return empty -- but must still read and reparse both full files to
/// find that out. Measures the per-call cost at the size the task names (535 rateable
/// moments means at most 535 ratings) and the cost of paying it on every one of many tool
/// calls across a session.
#[test]
fn pending_cost_steady_state_all_delivered() {
    for &n in &[535usize, 2_000] {
        let root = tmp_root(&format!("delivered-{n}"));
        let store = Store::for_session(&root, "claude-code", "sess");
        for i in 0..n {
            store.record(&rating(i, Verdict::Up)).unwrap();
        }
        // mark_delivered takes the ratings themselves, so a delivery references the exact
        // revision that went out rather than just the moment.
        let sent: Vec<_> = (0..n).map(|i| rating(i, Verdict::Up)).collect();
        store.mark_delivered(&sent, "2026-08-20T12:00:01Z").unwrap();

        let ratings_bytes = fs::metadata(root.join("claude-code/sess/ratings.jsonl"))
            .map(|m| m.len())
            .unwrap_or(0);
        let delivered_bytes = fs::metadata(root.join("claude-code/sess/delivered.jsonl"))
            .map(|m| m.len())
            .unwrap_or(0);

        let calls = 1_000usize; // stand-in for 1000 tool calls in a long session
        let start = Instant::now();
        for _ in 0..calls {
            let p = store.pending().unwrap();
            assert!(p.is_empty(), "steady state should have nothing pending");
        }
        let elapsed = start.elapsed();
        println!(
            "{n:>5} ratings, all delivered ({ratings_bytes}B + {delivered_bytes}B on disk): \
             {calls} pending() calls (1 per tool call) = {elapsed:?} total, \
             {:.1} microseconds/call",
            elapsed.as_secs_f64() * 1_000_000.0 / calls as f64
        );
        fs::remove_dir_all(&root).ok();
    }
}

/// Q3b: worst case for a single call -- nothing delivered yet, so every rating is parsed
/// AND pushed through the `latest.iter_mut().find()` de-duplication scan.
#[test]
fn pending_cost_worst_case_nothing_delivered() {
    for &n in &[535usize, 2_000, 10_000] {
        let root = tmp_root(&format!("undelivered-{n}"));
        let store = Store::for_session(&root, "claude-code", "sess");
        for i in 0..n {
            store.record(&rating(i, Verdict::Up)).unwrap();
        }

        let start = Instant::now();
        let pending = store.pending().unwrap();
        let elapsed = start.elapsed();
        assert_eq!(pending.len(), n);
        println!("{n:>6} ratings, 0 delivered: one pending() call = {elapsed:?}");
        fs::remove_dir_all(&root).ok();
    }
}

/// Q3c: the full production loop, at the scale this machine's real transcript suggests
/// (~2358 lines already in one session). Simulates one tool call at a time: `pending()` is
/// called every time (as the real hook does), and whenever it finds something, those ids
/// are immediately marked delivered (again, as the real hook does). A rating is recorded
/// every 15th tool call, which is generous for a human keystroke-rating an agent.
#[test]
fn full_session_simulation_pending_called_every_tool_call() {
    let tool_calls = 2_500usize;
    let root = tmp_root("session-sim");
    let store = Store::for_session(&root, "claude-code", "sess");

    let start = Instant::now();
    let mut total_pending_seen = 0usize;
    for call in 0..tool_calls {
        if call % 15 == 0 {
            store.record(&rating(call, Verdict::Up)).unwrap();
        }
        let pending = store.pending().unwrap();
        if !pending.is_empty() {
            total_pending_seen += pending.len();
            store.mark_delivered(&pending, "2026-08-20T12:00:01Z").unwrap();
        }
    }
    let elapsed = start.elapsed();
    println!(
        "{tool_calls} tool calls, a rating every 15th ({} ratings total), \
         pending() + mark_delivered on every call: {elapsed:?} total \
         ({:.1} microseconds/call), {total_pending_seen} pending-rating sightings",
        tool_calls / 15,
        elapsed.as_secs_f64() * 1_000_000.0 / tool_calls as f64
    );
    fs::remove_dir_all(&root).ok();
}
