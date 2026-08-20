//! Q1/Q4 from the stress-test pass: how long does `harness::parse` take on a real
//! transcript, and how much memory does the resulting `Vec<Moment>` actually cost.
//!
//! Run: `cargo test --release --test perf_parse -- --ignored --nocapture`
//! Ignored by default: it reads a transcript path that only exists on this machine.

use margin::harness;
use margin::moment::{Harness, MomentKind};
use std::time::Instant;

const BIGGEST_REAL_TRANSCRIPT: &str =
    "C:/Users/OASRVA~1/AppData/Local/Temp/claude/C--Users-oasrvadmin-Documents/b8a0b270-4b1a-40b4-be66-601802526393/scratchpad/frozen-transcript.jsonl";
// Frozen copy: the live file at ~/.claude/projects/.../b8a0b270....jsonl is this very session's
// own transcript and keeps growing while these tests run, so a live path gives a different
// number every run. Snapshot taken 2026-08-20 ~14:54 local: 4,990,963 bytes, 2,600 lines.

#[test]
#[ignore = "reads a transcript path specific to this machine"]
fn parse_time_and_memory_on_the_biggest_real_transcript() {
    let text = std::fs::read_to_string(BIGGEST_REAL_TRANSCRIPT)
        .expect("the transcript named in the task should be readable");
    let bytes = text.len();
    let lines = text.lines().count();

    // One warm-up run so the timed run isn't paying for a first-touch page fault or a cold
    // allocator, which would flatter every other harness on the machine, not just this one.
    let warm = harness::parse(Harness::ClaudeCode, &text);
    println!("warm-up parse produced {} moments", warm.len());
    drop(warm);

    // Five timed runs: report the best (removes scheduler noise) and the average.
    let mut runs = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let moments = harness::parse(Harness::ClaudeCode, &text);
        runs.push((start.elapsed(), moments));
    }
    let best = runs.iter().map(|(d, _)| *d).min().unwrap();
    let avg: std::time::Duration =
        runs.iter().map(|(d, _)| *d).sum::<std::time::Duration>() / runs.len() as u32;
    let moments = &runs[0].1;
    let rateable = moments.iter().filter(|m| m.kind.rateable()).count();

    println!(
        "parse: {bytes} bytes ({:.2} MB), {lines} lines -> {} moments ({rateable} rateable)",
        bytes as f64 / 1_000_000.0,
        moments.len()
    );
    println!("timing over 5 runs: best {best:?}, avg {avg:?}");

    // Memory: stack size of every Moment struct, plus the heap bytes each owned String is
    // actually holding (capacity, not len, since that is what the allocator reserved).
    let stack = std::mem::size_of::<margin::Moment>() * moments.len();
    let heap: usize = moments
        .iter()
        .map(|m| {
            let id_bytes =
                m.id.harness.capacity() + m.id.session_id.capacity() + m.id.entry.capacity();
            let at_bytes = m.at.as_ref().map_or(0, String::capacity);
            let kind_bytes = match &m.kind {
                MomentKind::Asked { text } | MomentKind::Said { text } => text.capacity(),
                MomentKind::Did {
                    tool,
                    input,
                    output,
                    tool_use_id,
                    intent,
                } => {
                    tool.capacity()
                        + input.capacity()
                        + output.as_ref().map_or(0, String::capacity)
                        + tool_use_id.as_ref().map_or(0, String::capacity)
                        + intent.as_ref().map_or(0, String::capacity)
                }
                MomentKind::Thought { text, .. } => text.as_ref().map_or(0, String::capacity),
            };
            id_bytes + at_bytes + kind_bytes
        })
        .sum();
    let total = stack + heap;
    println!(
        "memory for {} moments: {stack} B stack (struct bodies) + {heap} B heap (string data) \
         = {total} B ({:.1} KB, {:.2} MB)",
        moments.len(),
        total as f64 / 1024.0,
        total as f64 / 1_000_000.0
    );
    println!(
        "that is {:.1}x the raw transcript size ({:.2} MB in -> {:.2} MB resident)",
        total as f64 / bytes as f64,
        bytes as f64 / 1_000_000.0,
        total as f64 / 1_000_000.0
    );
}

/// Q1, extrapolated: parse cost against synthetic transcripts several times the real
/// session's size, built by repeating the committed fixture. Portable (no machine-specific
/// path), so this one runs in a normal `cargo test`. Answers "does it stay linear as the
/// transcript keeps growing during the session."
#[test]
fn parse_time_scales_linearly_with_input_size() {
    let unit = include_str!("../fixtures/claude-code/session-basic.jsonl");
    println!(
        "unit fixture: {} bytes, {} lines",
        unit.len(),
        unit.lines().count()
    );

    for &reps in &[50usize, 500, 2000, 8000] {
        let text = unit.repeat(reps);
        let start = Instant::now();
        let moments = harness::parse(Harness::ClaudeCode, &text);
        let elapsed = start.elapsed();
        println!(
            "{reps:>5} reps -> {:>9} bytes ({:>6.2} MB), {:>7} moments, parsed in {elapsed:?} \
             ({:.1} MB/s)",
            text.len(),
            text.len() as f64 / 1_000_000.0,
            moments.len(),
            (text.len() as f64 / 1_000_000.0) / elapsed.as_secs_f64().max(1e-9)
        );
    }
}
