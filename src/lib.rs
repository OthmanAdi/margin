//! Margin: rate a running coding agent, one keystroke, without interrupting it.
//!
//! Read `docs/FEASIBILITY.md` before changing how transcripts are read. It records what
//! each harness actually writes, measured rather than assumed.

pub mod harness;
pub mod moment;

pub use moment::{Harness, Moment, MomentId, MomentKind};
