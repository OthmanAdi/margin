//! Finding the session to watch.
//!
//! Both harnesses store transcripts under the home directory in their own layout:
//!
//! ```text
//!   ~/.claude/projects/<slug>/<session-uuid>.jsonl      slug is the cwd, separators -> '-'
//!   ~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl
//! ```
//!
//! Claude Code's slug is derived from the working directory, so the session for "here" can
//! be found without asking. Codex's layout is time-based with no directory link, so newest
//! wins and the user can override.

use crate::moment::Harness;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Session {
    pub harness: Harness,
    pub path: PathBuf,
    pub modified: SystemTime,
}

impl Session {
    /// Filename without extension. For Claude Code this is the session uuid; for Codex it is
    /// the rollout name, and the real id is read out of `session_meta` while parsing.
    pub fn id(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".into())
    }
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("neither USERPROFILE nor HOME is set"))
}

/// Claude Code's project directory name for a working directory.
///
/// `C:\Users\ahmad\CLEANROOM` becomes `C--Users-ahmad-CLEANROOM`: every character that is
/// not alphanumeric, a dot or a dash becomes a dash, which turns the drive colon into the
/// doubled dash that shows up in real paths.
pub fn claude_project_slug(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Every session for both harnesses, newest first.
pub fn all_sessions(home: &Path) -> Vec<Session> {
    let mut out = Vec::new();
    out.extend(claude_sessions(home, None));
    out.extend(codex_sessions(home));
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

/// Claude Code sessions, optionally narrowed to one working directory.
pub fn claude_sessions(home: &Path, cwd: Option<&Path>) -> Vec<Session> {
    let root = home.join(".claude").join("projects");
    let dirs: Vec<PathBuf> = match cwd {
        Some(cwd) => vec![root.join(claude_project_slug(cwd))],
        None => read_dir(&root).into_iter().filter(|p| p.is_dir()).collect(),
    };

    let mut out = Vec::new();
    for dir in dirs {
        for path in read_dir(&dir) {
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(modified) = mtime(&path) {
                out.push(Session {
                    harness: Harness::ClaudeCode,
                    path,
                    modified,
                });
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

/// Codex rollouts. The tree is `sessions/YYYY/MM/DD/`, so this walks three levels rather
/// than assuming a flat directory.
pub fn codex_sessions(home: &Path) -> Vec<Session> {
    let root = home.join(".codex").join("sessions");
    let mut out = Vec::new();
    for year in read_dir(&root) {
        for month in read_dir(&year) {
            for day in read_dir(&month) {
                for path in read_dir(&day) {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    let is_rollout = name.as_deref().is_some_and(|n| n.starts_with("rollout-"));
                    if !is_rollout || path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Some(modified) = mtime(&path) {
                        out.push(Session {
                            harness: Harness::Codex,
                            path,
                            modified,
                        });
                    }
                }
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

/// The session to attach to with no arguments: the most recently written transcript for
/// this working directory, falling back to the most recent anywhere.
pub fn newest_for_cwd(home: &Path, cwd: &Path) -> Option<Session> {
    claude_sessions(home, Some(cwd))
        .into_iter()
        .next()
        .or_else(|| all_sessions(home).into_iter().next())
}

fn read_dir(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    entries.flatten().map(|e| e.path()).collect()
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_the_layout_claude_code_actually_uses() {
        // Observed on this machine: C:\Users\oasrvadmin\CLEANROOM lives under
        // ~/.claude/projects/C--Users-oasrvadmin-CLEANROOM/
        assert_eq!(
            claude_project_slug(Path::new(r"C:\Users\oasrvadmin\CLEANROOM")),
            "C--Users-oasrvadmin-CLEANROOM"
        );
        assert_eq!(
            claude_project_slug(Path::new("/home/ahmad/work")),
            "-home-ahmad-work"
        );
    }

    #[test]
    fn slug_keeps_dots_and_dashes_which_appear_in_real_project_names() {
        assert_eq!(
            claude_project_slug(Path::new(r"C:\src\my-app.v2")),
            "C--src-my-app.v2"
        );
    }

    #[test]
    fn missing_directories_yield_nothing_rather_than_erroring() {
        let nowhere = std::env::temp_dir().join("margin-no-such-home-xyz");
        assert!(claude_sessions(&nowhere, None).is_empty());
        assert!(codex_sessions(&nowhere).is_empty());
        assert!(all_sessions(&nowhere).is_empty());
        assert!(newest_for_cwd(&nowhere, Path::new("/tmp")).is_none());
    }

    #[test]
    fn sessions_come_back_newest_first() {
        let home = std::env::temp_dir().join(format!("margin-disc-{}", std::process::id()));
        let dir = home.join(".claude").join("projects").join("proj");
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("older.jsonl"), "{}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("newer.jsonl"), "{}\n").unwrap();

        let found = claude_sessions(&home, None);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id(), "newer");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn non_jsonl_files_are_ignored() {
        let home = std::env::temp_dir().join(format!("margin-disc2-{}", std::process::id()));
        let dir = home.join(".claude").join("projects").join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), "hello").unwrap();
        std::fs::write(dir.join("real.jsonl"), "{}\n").unwrap();

        let found = claude_sessions(&home, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id(), "real");
        std::fs::remove_dir_all(&home).ok();
    }
}
