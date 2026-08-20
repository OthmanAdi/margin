//! Margin: rate a running coding agent, one keystroke, without interrupting it.
//!
//! Read `docs/FEASIBILITY.md` before changing how transcripts are read or how feedback is
//! delivered. It records what each harness actually writes and what actually reaches a live
//! turn, measured rather than assumed.

pub mod discover;
pub mod harness;
pub mod humanize;
pub mod inject;
pub mod moment;
pub mod ratings;
pub mod snapshot;
pub mod tail;
pub mod ui;

pub use moment::{Harness, Moment, MomentId, MomentKind};
pub use ratings::{Rating, Store, Verdict};
