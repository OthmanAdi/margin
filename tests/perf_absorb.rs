//! Q2 from the stress-test pass: `App::absorb` in `src/ui.rs` scans every existing moment
//! (`self.moments.iter_mut().find(|x| x.id == m.id)`) for each newly-parsed moment before
//! deciding whether it is an update or a new push. `App` and `absorb` are private to
//! `src/ui.rs`, so this file cannot call the real method directly; instead it mirrors the
//! exact loop body (verbatim, see `absorb_scan` below) against the real, public `Moment` /
//! `MomentId` types, so the comparison cost being measured is the real one.
//!
//! Mirrors `src/ui.rs` `App::absorb`, lines 87-116 (as of this pass):
//! ```ignore
//! for m in fresh {
//!     match self.moments.iter_mut().find(|x| x.id == m.id) {
//!         Some(existing) => *existing = m,
//!         None => { .. ; self.moments.push(m); }
//!     }
//! }
//! ```

use margin::moment::{Harness, Moment, MomentId, MomentKind};
use std::time::{Duration, Instant};

/// Verbatim mirror of the matching logic in `App::absorb`. The session-id/store-rebuild
/// side effect in the `None` branch is irrelevant to the scan cost and is omitted.
fn absorb_scan(moments: &mut Vec<Moment>, fresh: Vec<Moment>) {
    for m in fresh {
        match moments.iter_mut().find(|x| x.id == m.id) {
            Some(existing) => *existing = m,
            None => moments.push(m),
        }
    }
}

/// A moment with a realistically-sized body, so string comparisons and allocations cost
/// what they would in production rather than what they cost for `""`.
fn make_moment(i: usize) -> Moment {
    Moment {
        id: MomentId::new(
            Harness::ClaudeCode,
            "session-abc123",
            format!("entry-{i:08}"),
            0,
        ),
        seq: i,
        at: Some("2026-08-20T12:00:00.000Z".to_string()),
        kind: MomentKind::Said {
            text: format!(
                "moment number {i}, a realistic line of agent prose so the id comparison and \
                 the eventual clone cost what production actually pays for it"
            ),
        },
    }
}

/// Q2a: does absorbing ONE new moment get slower as the session accumulates more of them.
/// Each size builds a fresh vec of that many distinct existing moments, then times exactly
/// one `absorb_scan` call adding one guaranteed-new (worst case: full-scan) moment.
#[test]
fn single_absorb_call_cost_grows_with_existing_moment_count() {
    println!("existing moments -> cost of absorbing ONE more (guaranteed miss, full scan)");
    for &n in &[100usize, 500, 1_000, 2_000, 5_000, 10_000, 20_000] {
        let mut moments: Vec<Moment> = (0..n).map(make_moment).collect();
        let fresh = vec![make_moment(n)]; // new id, not present -> forces a full scan
        let start = Instant::now();
        absorb_scan(&mut moments, fresh);
        let elapsed = start.elapsed();
        println!("{n:>7} existing -> {elapsed:>12?} for the one new moment");
    }
}

/// Q2b: the realistic pattern is not one absorb call on a prebuilt vec, it is hundreds of
/// absorb calls across a session, each one scanning however big the vec has grown to. This
/// simulates the worst case for that shape: exactly one new moment revealed per tick, so
/// every one of the N insertions pays for a full scan of everything before it. Total
/// comparisons should come out close to N*(N-1)/2 if the O(n) per-call claim is right.
#[test]
fn cumulative_session_cost_one_moment_per_tick() {
    for &total in &[535usize, 2_500, 10_000] {
        let mut moments: Vec<Moment> = Vec::new();
        let start = Instant::now();
        let mut last_call: Duration = Duration::ZERO;
        for i in 0..total {
            let call_start = Instant::now();
            absorb_scan(&mut moments, vec![make_moment(i)]);
            last_call = call_start.elapsed();
        }
        let elapsed = start.elapsed();
        let predicted_comparisons = (total as u128 * (total as u128 - 1)) / 2;
        println!(
            "grow 0 -> {total:>6} one-at-a-time: total {elapsed:>12?}, last insert alone \
             {last_call:>10?}, ~{predicted_comparisons} id comparisons predicted by O(n^2/2)"
        );
    }
}

/// Q2c: same total moment count, but arriving in small realistic batches (a tool call
/// producing 2-4 moments per absorb, rather than exactly one) instead of one at a time.
/// Batching changes how many times `absorb_scan` itself is called, not how many elements
/// get scanned in total, so this should land close to the one-at-a-time number for the same
/// final size, not meaingfully cheaper.
#[test]
fn cumulative_session_cost_realistic_batches() {
    let total = 2_500usize;
    let mut moments: Vec<Moment> = Vec::new();
    let mut i = 0usize;
    let start = Instant::now();
    let mut batch_no = 0usize;
    while i < total {
        let batch_size = 1 + (batch_no % 4); // 1..=4, mimics a tool call's handful of moments
        let end = (i + batch_size).min(total);
        let fresh: Vec<Moment> = (i..end).map(make_moment).collect();
        absorb_scan(&mut moments, fresh);
        i = end;
        batch_no += 1;
    }
    let elapsed = start.elapsed();
    println!(
        "grow 0 -> {total} in {batch_no} batches of 1-4: total {elapsed:?} \
         ({:.3} ms/moment average)",
        elapsed.as_secs_f64() * 1000.0 / total as f64
    );
}
