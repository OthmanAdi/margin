//! Q5 from the stress-test pass: `draw_moments` in `src/ui.rs` builds a fresh `Vec<ListItem>`
//! from every moment on every call to `draw`, which runs at least once per 250ms idle tick
//! (more often when keys or file changes arrive). `draw_moments` and `App` are private to
//! `src/ui.rs`, so this file mirrors the item-construction closure and the small styling
//! helpers verbatim against the real, public `Moment` / ratatui types, rather than the
//! private function itself.
//!
//! Mirrors `src/ui.rs` `draw_moments`, lines 404-465 (as of this pass). The helpers
//! (`clock`, `verdict_glyph`, `verdict_style`, `kind_style`, `body_style`) are copied
//! unchanged from lines 566-605; they are a few lines of match statements each, not the
//! thing under test.

use margin::moment::{Harness, Moment, MomentId, MomentKind};
use margin::ratings::Verdict;
use ratatui::backend::TestBackend;
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
use ratatui::Terminal;
use std::collections::HashMap;
use std::time::Instant;

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const WARN: Color = Color::Yellow;
const GOOD: Color = Color::Green;
const BAD: Color = Color::Red;

fn clock(at: Option<&str>) -> String {
    at.and_then(|s| s.split_once('T'))
        .map(|(_, t)| t.split(['.', 'Z', '+']).next().unwrap_or(t).to_string())
        .unwrap_or_else(|| "--:--:--".into())
}

fn verdict_glyph(v: Option<Verdict>) -> &'static str {
    match v {
        Some(Verdict::Up) => "+",
        Some(Verdict::Down) => "-",
        None => " ",
    }
}

fn verdict_style(v: Option<Verdict>) -> Style {
    match v {
        Some(Verdict::Up) => Style::new().fg(GOOD).bold(),
        Some(Verdict::Down) => Style::new().fg(BAD).bold(),
        None => Style::new(),
    }
}

fn kind_style(kind: &MomentKind) -> Style {
    match kind {
        MomentKind::Said { .. } => Style::new().fg(Color::White),
        MomentKind::Did { .. } => Style::new().fg(ACCENT),
        MomentKind::Thought { .. } => Style::new().fg(Color::Magenta),
        MomentKind::Asked { .. } => Style::new().fg(DIM),
    }
}

fn body_style(kind: &MomentKind) -> Style {
    match kind {
        MomentKind::Thought { text: None, .. } => Style::new().fg(DIM).italic(),
        MomentKind::Asked { .. } => Style::new().fg(DIM),
        _ => Style::new(),
    }
}

/// Verbatim mirror of the `.map()` closure inside `draw_moments`.
fn build_items(
    moments: &[Moment],
    verdicts: &HashMap<String, Verdict>,
    notes: &HashMap<String, String>,
    width: usize,
) -> Vec<ListItem<'static>> {
    moments
        .iter()
        .map(|m| {
            let key = m.id.to_string();
            let verdict = verdicts.get(&key).copied();
            let mut spans = vec![
                Span::styled(
                    format!(" {} ", verdict_glyph(verdict)),
                    verdict_style(verdict),
                ),
                Span::styled(
                    format!("{:<9}", clock(m.at.as_deref())),
                    Style::new().fg(DIM),
                ),
                Span::styled(format!("{:<8}", m.kind.label()), kind_style(&m.kind)),
                Span::styled(m.preview(width.max(20)), body_style(&m.kind)),
            ];
            if let Some(note) = notes.get(&key) {
                spans.push(Span::styled(
                    format!("  ({note})"),
                    Style::new().fg(WARN).italic(),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect()
}

/// Realistic mixed transcript: cycles through all four kinds, a fifth of moments rated, a
/// tenth of the down-rated ones carrying a note, some timestamps missing, matching the
/// shapes the real UI actually has to handle rather than a uniform easy case.
fn make_session(
    n: usize,
) -> (
    Vec<Moment>,
    HashMap<String, Verdict>,
    HashMap<String, String>,
) {
    let mut moments = Vec::with_capacity(n);
    let mut verdicts = HashMap::new();
    let mut notes = HashMap::new();
    for i in 0..n {
        let kind = match i % 4 {
            0 => MomentKind::Asked {
                text: format!("do the thing number {i}, and also handle the edge case"),
            },
            1 => MomentKind::Said {
                text: format!(
                    "moment {i}: here is a paragraph of agent prose long enough to actually \
                     need clipping to the card width, which is the realistic case, not a \
                     five character string"
                ),
            },
            2 => MomentKind::Did {
                tool: "Bash".into(),
                input: format!("cargo test --release --test perf_render -- case_{i}"),
                output: Some(format!("ok, {i} passed")),
                tool_use_id: Some(format!("toolu_{i:08}")),
                intent: Some(format!("run perf case {i}")),
            },
            _ => MomentKind::Thought {
                text: None,
                bytes: 3000 + i,
            },
        };
        let at = if i % 7 == 0 {
            None
        } else {
            Some(format!(
                "2026-08-20T12:{:02}:{:02}.000Z",
                (i / 60) % 60,
                i % 60
            ))
        };
        let m = Moment {
            id: MomentId::new(
                Harness::ClaudeCode,
                "session-abc123",
                format!("entry-{i:08}"),
                0,
            ),
            seq: i,
            at,
            kind,
        };
        let key = m.id.to_string();
        if i % 5 == 0 {
            let v = if i % 2 == 0 {
                Verdict::Up
            } else {
                Verdict::Down
            };
            verdicts.insert(key.clone(), v);
            if v == Verdict::Down && i % 10 == 0 {
                notes.insert(key, "wrong approach, see the earlier note".to_string());
            }
        }
        moments.push(m);
    }
    (moments, verdicts, notes)
}

/// Q5a: the N-dependent part. `draw_moments` maps over ALL moments every call regardless of
/// scroll position (no `.skip()`/`.take()` before the item vec is built), so this is the
/// real per-frame cost, not an artificially scaled-down one.
#[test]
fn list_item_construction_cost_at_realistic_and_stress_sizes() {
    let width = 96usize.saturating_sub(24); // matches a common terminal width in draw_moments
    println!("moments -> cost to build the full Vec<ListItem> for one frame");
    for &n in &[100usize, 535, 1_000, 2_500, 5_000, 10_000] {
        let (moments, verdicts, notes) = make_session(n);
        // warm-up
        let _ = build_items(&moments, &verdicts, &notes, width);
        let start = Instant::now();
        let items = build_items(&moments, &verdicts, &notes, width);
        let elapsed = start.elapsed();
        let per_sec_at_4hz = elapsed.as_secs_f64() * 4.0 * 1000.0; // idle tick is 4 draws/sec
        println!(
            "{n:>6} moments -> {elapsed:>10?} to build {} items \
             ({per_sec_at_4hz:.2} ms/sec spent on this at the idle 250ms tick rate)",
            items.len()
        );
    }
}

/// Q5b: the full frame, including ratatui's own buffer render, at the size named in the
/// task (535 rateable moments; using ~4x that as total moments since Asked/Thought/Did
/// entries are not all rateable but are still in the list). Confirms whether the List
/// widget's own render cost is bounded by the viewport (visible rows) rather than by N.
#[test]
fn full_frame_render_cost_including_ratatui_buffer_diff() {
    for &(n, cols, rows) in &[(2_000usize, 120u16, 50u16), (2_000, 120, 12)] {
        let (moments, verdicts, notes) = make_session(n);
        let width = cols.saturating_sub(24) as usize;
        let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut list_state = ListState::default();
        list_state.select(Some(n.saturating_sub(1)));

        // warm-up frame
        terminal
            .draw(|f| {
                let items = build_items(&moments, &verdicts, &notes, width);
                let list = List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded),
                );
                f.render_stateful_widget(list, f.area(), &mut list_state);
            })
            .unwrap();

        let start = Instant::now();
        terminal
            .draw(|f| {
                let items = build_items(&moments, &verdicts, &notes, width);
                let list = List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded),
                );
                f.render_stateful_widget(list, f.area(), &mut list_state);
            })
            .unwrap();
        let elapsed = start.elapsed();
        println!(
            "{n} moments, {cols}x{rows} terminal ({rows} visible rows) -> full frame \
             (build + ratatui render) in {elapsed:?}"
        );
    }
}
