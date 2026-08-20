//! Append-only rating store.
//!
//! Two processes touch this concurrently: the TUI, where you press a key, and the hook,
//! which fires inside the agent's own process. Rather than lock a shared file, each writes
//! its own append-only log and "pending" is derived:
//!
//! ```text
//!   ratings.jsonl    written by the TUI     one line per keypress
//!   delivered.jsonl  written by the hook    one line per rating handed to the agent
//!   pending = ratings - delivered
//! ```
//!
//! Nothing is ever mutated in place, so a crash mid-write costs at most the torn last line,
//! which parses as garbage and is skipped. That also makes the whole history auditable
//! later, which is what turns ratings into eval data.

use crate::moment::MomentId;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Up,
    Down,
}

impl Verdict {
    pub fn glyph(self) -> &'static str {
        match self {
            Verdict::Up => "▲",
            Verdict::Down => "▼",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rating {
    /// Which moment this is about.
    pub moment: MomentId,
    pub verdict: Verdict,
    /// Optional one line of why. This is the durable artifact when the harness did not
    /// persist what was rated, which on Claude Code is every thought.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// RFC3339, when the key was pressed.
    pub at: String,
    /// What was on screen when it was rated. Lets a stale rating be detected if a
    /// transcript is ever rewritten, and gives the injected text something to quote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Coarse label for what was rated: `said`, `thought`, or `did:Bash`.
    ///
    /// Exists so two ratings can be told to be about the same kind of behaviour. Without
    /// it, "these signals conflict" would fire on any batch containing one approval and one
    /// rejection, which is nearly every batch, and telling an agent that two unrelated
    /// judgments contradict each other is worse than saying nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Delivery {
    moment: MomentId,
    /// When the delivery happened.
    at: String,
    /// The `at` of the exact rating that was delivered.
    ///
    /// Without this, delivery is keyed by moment, so changing your mind about a moment that
    /// was already sent is suppressed forever: the correction is filtered out as "already
    /// delivered" and the agent keeps the stale verdict. Absent on records written before
    /// this field existed, which are read as covering every rating up to the delivery time
    /// and nothing after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rating_at: Option<String>,
}

/// Where the logs live for one session.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// `~/.margin/<harness>/<session-id>/`
    pub fn for_session(root: &Path, harness: &str, session_id: &str) -> Self {
        Self {
            dir: root.join(harness).join(sanitise(session_id)),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn ratings_path(&self) -> PathBuf {
        self.dir.join("ratings.jsonl")
    }

    fn delivered_path(&self) -> PathBuf {
        self.dir.join("delivered.jsonl")
    }

    /// Append one rating. Called on a keypress, so it must be fast and must not block the
    /// UI thread on anything more than a single small append.
    pub fn record(&self, rating: &Rating) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        append_line(&self.ratings_path(), &serde_json::to_string(rating)?)
    }

    fn heartbeat_path(&self) -> PathBuf {
        self.dir.join("hook-seen")
    }

    /// Record that the hook ran, whether or not it had anything to say.
    ///
    /// Exists because of a trap a first-time user hits immediately: Claude Code reads
    /// `settings.json` at session start, so a hook installed mid-session is silently inert
    /// until the next restart. Without this, the symptom is a rating that simply never
    /// arrives and no way to tell whether the tool is broken or merely not loaded yet.
    ///
    /// Best effort. The hook runs inside the agent's process and must never fail it, so a
    /// heartbeat that cannot be written is ignored rather than propagated.
    pub fn touch_heartbeat(&self) {
        let _ = fs::create_dir_all(&self.dir);
        let _ = fs::write(self.heartbeat_path(), b"");
    }

    /// Whether the hook has fired for this session at all.
    pub fn hook_seen(&self) -> bool {
        self.heartbeat_path().exists()
    }

    /// Mark ratings as handed to the agent, so the next hook invocation does not repeat
    /// them. Repeating is the failure mode that poisons a context.
    /// Mark exactly the ratings that were handed over.
    ///
    /// Takes ratings rather than moments because the injected block is capped: passing every
    /// pending moment marked ratings delivered that were never rendered, and they then never
    /// appeared again.
    pub fn mark_delivered(&self, ratings: &[Rating], at: &str) -> Result<()> {
        if ratings.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        let mut buf = String::new();
        for r in ratings {
            let d = Delivery {
                moment: r.moment.clone(),
                at: at.to_string(),
                rating_at: Some(r.at.clone()),
            };
            buf.push_str(&serde_json::to_string(&d)?);
            buf.push('\n');
        }
        append_raw(&self.delivered_path(), &buf)
    }

    pub fn all(&self) -> Result<Vec<Rating>> {
        Ok(read_jsonl(&self.ratings_path()))
    }

    /// Ratings not yet handed to the agent, oldest first.
    ///
    /// A moment rated twice keeps only the last verdict: changing your mind should not
    /// deliver both opinions.
    pub fn pending(&self) -> Result<Vec<Rating>> {
        // Exact revisions that went out, plus, for records written before revisions existed,
        // the latest delivery time, which covers everything rated up to it.
        let mut delivered: HashSet<(MomentId, String)> = HashSet::new();
        let mut legacy: HashMap<MomentId, String> = HashMap::new();
        for d in read_jsonl::<Delivery>(&self.delivered_path()) {
            match d.rating_at {
                Some(ra) => {
                    delivered.insert((d.moment, ra));
                }
                None => {
                    let slot = legacy.entry(d.moment).or_default();
                    if d.at > *slot {
                        *slot = d.at;
                    }
                }
            }
        }

        // Index alongside the vec rather than scanning it.
        //
        // The obvious `latest.iter_mut().find(...)` is O(n) per rating and therefore O(n^2)
        // over the file. That is invisible at normal sizes and a cliff at abnormal ones:
        // measured at 29ms for 535 undelivered ratings and 960ms for 10,000, paid
        // synchronously inside the agent's process on every single tool call. The vec is
        // kept so pending stays in rating order, oldest first, which the injected text
        // depends on.
        let mut latest: Vec<Rating> = Vec::new();
        let mut seen: HashMap<MomentId, usize> = HashMap::new();

        for r in read_jsonl::<Rating>(&self.ratings_path()) {
            if delivered.contains(&(r.moment.clone(), r.at.clone())) {
                continue;
            }
            if legacy.get(&r.moment).is_some_and(|cutoff| r.at <= *cutoff) {
                continue;
            }
            match seen.get(&r.moment) {
                // Re-rating keeps the newer verdict in the older position: the user changed
                // their mind about that moment, they did not have a new thought later.
                Some(&idx) => latest[idx] = r,
                None => {
                    seen.insert(r.moment.clone(), latest.len());
                    latest.push(r);
                }
            }
        }
        Ok(latest)
    }
}

fn append_line(path: &Path, line: &str) -> Result<()> {
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    append_raw(path, &buf)
}

fn append_raw(path: &Path, data: &str) -> Result<()> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    // One write call per append. Short appends to a file opened for append are not
    // interleaved by the OS, which is what keeps two writers from tearing each other.
    f.write_all(data.as_bytes())?;
    f.flush()?;
    Ok(())
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<T>(l).ok())
        .collect()
}

/// Session ids come from the harness and land in a path, so refuse anything that could
/// escape the store directory.
fn sanitise(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moment::Harness;

    fn tmp() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "margin-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn rating(entry: &str, verdict: Verdict) -> Rating {
        Rating {
            moment: MomentId::new(Harness::ClaudeCode, "sess", entry, 0),
            verdict,
            note: None,
            at: "2026-08-20T12:00:00Z".into(),
            preview: Some(format!("preview of {entry}")),
            subject: Some("said".into()),
        }
    }

    #[test]
    fn records_and_reads_back() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "sess");
        s.record(&rating("a", Verdict::Up)).unwrap();
        s.record(&rating("b", Verdict::Down)).unwrap();

        let all = s.all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].verdict, Verdict::Up);
        assert_eq!(all[1].verdict, Verdict::Down);
        fs::remove_dir_all(&root).ok();
    }

    /// The rule that stops the injected context from being poisoned by repetition.
    #[test]
    fn delivered_ratings_never_come_back() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "sess");
        s.record(&rating("a", Verdict::Up)).unwrap();
        s.record(&rating("b", Verdict::Down)).unwrap();
        assert_eq!(s.pending().unwrap().len(), 2);

        let sent = s.pending().unwrap();
        s.mark_delivered(&sent, "2026-08-20T12:00:01Z").unwrap();
        assert!(
            s.pending().unwrap().is_empty(),
            "delivered ratings were re-queued"
        );

        // a later rating still gets through
        s.record(&rating("c", Verdict::Up)).unwrap();
        assert_eq!(s.pending().unwrap().len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    /// The dedup used to be a linear scan per rating, which is a cliff rather than a slope:
    /// 960ms for one call at 10k undelivered, paid inside the agent's process on every tool
    /// call. This would take minutes if that regressed.
    #[test]
    fn a_large_backlog_stays_fast() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "sess");
        for i in 0..8000 {
            s.record(&rating(&format!("m{i}"), Verdict::Up)).unwrap();
        }
        let start = std::time::Instant::now();
        let pending = s.pending().unwrap();
        let elapsed = start.elapsed();

        assert_eq!(pending.len(), 8000);
        assert!(
            elapsed.as_millis() < 500,
            "pending() took {elapsed:?} for 8000 undelivered ratings; the quadratic dedup is back"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// The injected text weights the last item most, so order is load-bearing, not cosmetic.
    #[test]
    fn pending_stays_in_rating_order_even_when_a_moment_is_re_rated() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "sess");
        s.record(&rating("first", Verdict::Up)).unwrap();
        s.record(&rating("second", Verdict::Up)).unwrap();
        s.record(&rating("third", Verdict::Up)).unwrap();
        // change our mind about the first one; it must not jump to the end
        s.record(&rating("first", Verdict::Down)).unwrap();

        let pending = s.pending().unwrap();
        let order: Vec<&str> = pending.iter().map(|r| r.moment.entry.as_str()).collect();
        assert_eq!(order, vec!["first", "second", "third"]);
        assert_eq!(pending[0].verdict, Verdict::Down);
        fs::remove_dir_all(&root).ok();
    }

    /// Delivery used to be keyed by moment, so a correction to something already sent was
    /// filtered out forever and the agent kept the stale verdict.
    #[test]
    fn re_rating_an_already_delivered_moment_is_delivered_again() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "sess");

        let mut first = rating("a", Verdict::Up);
        first.at = "2026-08-20T12:00:00Z".into();
        s.record(&first).unwrap();

        let sent = s.pending().unwrap();
        assert_eq!(sent.len(), 1);
        s.mark_delivered(&sent, "2026-08-20T12:00:01Z").unwrap();
        assert!(s.pending().unwrap().is_empty());

        // the user changes their mind about the same moment
        let mut correction = rating("a", Verdict::Down);
        correction.at = "2026-08-20T12:05:00Z".into();
        s.record(&correction).unwrap();

        let pending = s.pending().unwrap();
        assert_eq!(
            pending.len(),
            1,
            "the correction was suppressed as already delivered"
        );
        assert_eq!(pending[0].verdict, Verdict::Down);
        fs::remove_dir_all(&root).ok();
    }

    /// Records written before deliveries carried a revision must still suppress what they
    /// covered, without swallowing anything rated afterwards.
    #[test]
    fn a_legacy_delivery_record_covers_only_what_preceded_it() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "sess");

        let mut old = rating("a", Verdict::Up);
        old.at = "2026-08-20T12:00:00Z".into();
        s.record(&old).unwrap();

        // hand-write the pre-revision shape
        fs::create_dir_all(s.dir()).unwrap();
        append_raw(
            &s.dir().join("delivered.jsonl"),
            "{\"moment\":{\"harness\":\"claude-code\",\"session_id\":\"sess\",\"entry\":\"a\",\"block\":0},\"at\":\"2026-08-20T12:00:01Z\"}\n",
        )
        .unwrap();
        assert!(
            s.pending().unwrap().is_empty(),
            "legacy record should still suppress"
        );

        let mut later = rating("a", Verdict::Down);
        later.at = "2026-08-20T12:09:00Z".into();
        s.record(&later).unwrap();
        assert_eq!(
            s.pending().unwrap().len(),
            1,
            "a later rating must survive a legacy record"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn changing_your_mind_delivers_only_the_last_verdict() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "sess");
        s.record(&rating("a", Verdict::Up)).unwrap();
        s.record(&rating("a", Verdict::Down)).unwrap();

        let pending = s.pending().unwrap();
        assert_eq!(pending.len(), 1, "both opinions were queued");
        assert_eq!(pending[0].verdict, Verdict::Down);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_torn_last_line_does_not_lose_earlier_ratings() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "sess");
        s.record(&rating("a", Verdict::Up)).unwrap();
        // simulate a crash mid-append
        append_raw(&s.ratings_path(), "{\"moment\":{\"harn").unwrap();
        assert_eq!(s.all().unwrap().len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_files_read_as_empty_rather_than_erroring() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "never-written");
        assert!(s.all().unwrap().is_empty());
        assert!(s.pending().unwrap().is_empty());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn session_ids_cannot_escape_the_store_directory() {
        let root = tmp();
        let s = Store::for_session(&root, "claude-code", "../../etc/passwd");
        assert!(
            s.dir().starts_with(&root),
            "path traversal via session id: {:?}",
            s.dir()
        );
        fs::remove_dir_all(&root).ok();
    }
}
