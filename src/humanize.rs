//! Turning a tool call into something a human can judge in one second.
//!
//! A row that reads
//!
//! ```text
//! did  Bash(cd /c/Users/oasrvadmin/CLEANROOM/work/margin && python -c " import io p='src/mai…
//! ```
//!
//! tells the reader nothing. Four consecutive rows look identical, and the whole product is
//! picking one row and pressing one key, so rows that cannot be told apart defeat it.
//!
//! Everything here is deterministic string work. Nothing is inferred by a model: a row that
//! quietly guesses wrong about what the agent did is worse than one that is merely ugly.

/// Longest a rendered summary may be before the caller clips it.
const MAX: usize = 160;

/// One line describing what a tool call actually did.
pub fn tool(name: &str, input: &str) -> String {
    // MCP tools arrive as mcp__server__tool and the server name is the useful half.
    if let Some(rest) = name.strip_prefix("mcp__") {
        let mut parts = rest.split("__");
        let server = parts.next().unwrap_or(rest);
        let tool = parts.next().unwrap_or("");
        let label = if tool.is_empty() {
            server.to_string()
        } else {
            format!("{server}: {}", tool.replace('_', " "))
        };
        return clip(&label, MAX);
    }

    let arg = input.trim();
    let out = match name {
        "Read" => format!("read {}", short_path(arg)),
        "Write" => format!("wrote {}", short_path(arg)),
        "Edit" | "NotebookEdit" => format!("edited {}", short_path(arg)),
        "Glob" => format!("find {arg}"),
        "Grep" => format!("search {arg}"),
        "WebFetch" => format!("fetch {}", host_of(arg)),
        "WebSearch" => format!("web search {arg}"),
        "TodoWrite" => "updated the todo list".to_string(),
        "Task" | "Agent" => format!("delegated: {arg}"),
        "Bash" | "PowerShell" | "Shell" => command(arg),
        other if arg.is_empty() => other.to_lowercase(),
        other => format!("{}: {arg}", other.to_lowercase()),
    };
    clip(&collapse(&out), MAX)
}

/// Reduce a shell command to the part that identifies it.
///
/// Real commands from an agent are chains: a `cd` into the repo, then the actual work, then
/// a couple of `echo`s for progress. The `cd` and the `echo`s are noise every single time,
/// and they sit at the front where the eye lands.
pub fn command(cmd: &str) -> String {
    let segments = split_chain(cmd);

    let interesting: Vec<&str> = segments
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !is_noise(s))
        .collect();

    // A chain of nothing but cd and echo is still worth naming rather than rendering blank.
    if interesting.is_empty() {
        return collapse(segments.first().unwrap_or(&cmd).trim());
    }

    let head = summarise_segment(interesting[0]);
    if interesting.len() > 1 {
        format!("{head}  (+{} more)", interesting.len() - 1)
    } else {
        head
    }
}

/// Split on `&&`, `||` and `;` without breaking inside quotes.
fn split_chain(cmd: &str) -> Vec<&str> {
    let bytes = cmd.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => {
                if b == b'\'' || b == b'"' {
                    quote = Some(b);
                } else if b == b';' {
                    out.push(&cmd[start..i]);
                    start = i + 1;
                } else if (b == b'&' || b == b'|') && i + 1 < bytes.len() && bytes[i + 1] == b {
                    out.push(&cmd[start..i]);
                    i += 1;
                    start = i + 1;
                }
            }
        }
        i += 1;
    }
    out.push(&cmd[start..]);
    out
}

/// Segments that are pure ceremony and never what the reader wants to see.
fn is_noise(segment: &str) -> bool {
    let s = segment.trim_start();
    s.starts_with("cd ")
        || s == "cd"
        || s.starts_with("echo ")
        || s.starts_with("export ")
        || s.starts_with("set -")
        || s.starts_with("pwd")
}

/// Name one command by its program and the most identifying argument.
fn summarise_segment(segment: &str) -> String {
    let seg = segment.trim();
    let mut words = seg.split_whitespace();
    let Some(prog) = words.next() else {
        return String::new();
    };
    let prog_short = short_path(prog);

    // An inline script is the common case for python/node/perl and its body is unreadable
    // at this width. Naming it as a script beats showing forty characters of its middle.
    let rest: Vec<&str> = words.collect();
    if rest.iter().any(|w| *w == "-c" || *w == "-e") {
        return format!("{prog_short} (inline script)");
    }
    if seg.contains("<<") {
        return format!("{prog_short} (heredoc)");
    }

    // Otherwise: the program, its subcommand if it has one, and the first real argument.
    let mut parts = vec![prog_short.to_string()];
    for w in rest.iter().take(6) {
        if w.starts_with('-') {
            continue; // flags rarely identify a command to a human
        }
        parts.push(short_path(w).to_string());
        if parts.len() >= 3 {
            break;
        }
    }
    parts.join(" ")
}

/// Last one or two path components, so a row shows `src/main.rs` not a 60-character absolute
/// path whose only distinguishing part is at the end.
pub fn short_path(p: &str) -> &str {
    let trimmed = p.trim_matches(|c| c == '"' || c == '\'');
    if trimmed.len() < 24 {
        return trimmed;
    }
    let sep = |c: char| c == '/' || c == '\\';
    let cut = trimmed
        .rmatch_indices(sep)
        .nth(1)
        .map(|(i, _)| i + 1)
        .or_else(|| trimmed.rmatch_indices(sep).next().map(|(i, _)| i + 1));
    match cut {
        Some(i) if i < trimmed.len() => &trimmed[i..],
        _ => trimmed,
    }
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
}

/// Collapse every run of whitespace, including newlines, to one space.
pub fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first sentence, for prose rows.
///
/// A human deciding good or bad needs the point, and the point is almost always the first
/// sentence. Showing the first N characters instead cuts mid-word and reads as damage.
pub fn first_sentence(text: &str, max: usize) -> String {
    let flat = collapse(text);
    let mut end = None;
    let bytes = flat.as_bytes();
    for (i, c) in flat.char_indices() {
        if matches!(c, '.' | '!' | '?') {
            // not a sentence end if it is a decimal point or an ellipsis
            let next = bytes.get(i + 1).copied();
            let prev = if i == 0 {
                None
            } else {
                bytes.get(i - 1).copied()
            };
            let next_is_space_or_end = next.is_none_or(|n| n == b' ');
            let numeric = prev.is_some_and(|p| p.is_ascii_digit())
                && next.is_some_and(|n| n.is_ascii_digit());
            if next_is_space_or_end && !numeric {
                end = Some(i + 1);
                break;
            }
        }
    }
    let candidate = match end {
        Some(i) if i <= max => flat[..i].to_string(),
        _ => flat,
    };
    clip(&candidate, max)
}

/// Clip on a word boundary where possible, so a row never ends mid-word.
pub fn clip(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width <= 2 {
        return "…".to_string();
    }
    let budget = width - 1;
    let head: String = s.chars().take(budget).collect();
    // back off to the last space, but not so far that the row becomes stubby
    let cut = head.rfind(' ').filter(|i| *i > budget * 2 / 3);
    let kept = match cut {
        Some(i) => &head[..i],
        None => head.as_str(),
    };
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact row from a real screenshot that made this module necessary.
    #[test]
    fn the_row_that_told_a_human_nothing() {
        let raw = r#"cd /c/Users/oasrvadmin/CLEANROOM/work/margin && python -c " import io p='src/main.rs' s=io.open(p).read() ...""#;
        assert_eq!(tool("Bash", raw), "python (inline script)");
    }

    #[test]
    fn a_cd_prefix_is_always_dropped() {
        assert_eq!(
            tool("Bash", "cd /some/deep/path && cargo test --quiet"),
            "cargo test"
        );
        assert_eq!(tool("Bash", "cd x; ls -la"), "ls");
    }

    #[test]
    fn echo_progress_noise_is_dropped_but_counted() {
        let out = tool("Bash", "cd repo && cargo build && echo done");
        assert_eq!(out, "cargo build");
    }

    #[test]
    fn a_chain_of_real_commands_says_how_many() {
        let out = tool("Bash", "cargo fmt && cargo clippy && cargo test");
        assert!(out.starts_with("cargo fmt"), "got {out}");
        assert!(out.contains("+2 more"), "got {out}");
    }

    #[test]
    fn separators_inside_quotes_do_not_split_the_chain() {
        let out = tool("Bash", r#"grep "a && b" file.txt"#);
        assert!(out.starts_with("grep"), "got {out}");
        assert!(!out.contains("more"), "split inside a quoted string: {out}");
    }

    #[test]
    fn a_command_that_is_only_ceremony_still_renders() {
        assert_eq!(tool("Bash", "cd /tmp"), "cd /tmp");
        assert_eq!(tool("Bash", "echo hello"), "echo hello");
    }

    #[test]
    fn file_tools_read_as_verbs_on_a_short_path() {
        assert_eq!(
            tool(
                "Read",
                r"C:\Users\oasrvadmin\CLEANROOM\work\margin\docs\img\live-window.png"
            ),
            // separator style is left as the platform wrote it; rewriting it would mean
            // allocating on every row to change something a reader does not care about
            r"read img\live-window.png"
        );
        assert_eq!(tool("Edit", "src/main.rs"), "edited src/main.rs");
        assert_eq!(tool("Write", "notes.md"), "wrote notes.md");
    }

    #[test]
    fn mcp_tools_name_their_server() {
        assert_eq!(
            tool("mcp__playwright__browser_navigate", "{}"),
            "playwright: browser navigate"
        );
        assert_eq!(tool("mcp__qmd__query", ""), "qmd: query");
    }

    #[test]
    fn a_heredoc_is_named_not_quoted() {
        assert_eq!(
            tool("Bash", "cat > f.txt <<'EOF'\nhello\nEOF"),
            "cat (heredoc)"
        );
    }

    #[test]
    fn web_fetch_shows_the_host() {
        assert_eq!(
            tool("WebFetch", "https://docs.rs/ratatui/latest/ratatui/"),
            "fetch docs.rs"
        );
    }

    #[test]
    fn first_sentence_stops_at_the_point() {
        assert_eq!(
            first_sentence(
                "Two problems. Your status bar is empty because margin stays silent.",
                80
            ),
            "Two problems."
        );
    }

    #[test]
    fn first_sentence_is_not_fooled_by_a_decimal() {
        assert_eq!(
            first_sentence("It took 1.5 seconds. Then it stopped.", 80),
            "It took 1.5 seconds."
        );
    }

    #[test]
    fn a_long_first_sentence_clips_on_a_word_boundary() {
        let out = first_sentence(
            "this sentence keeps going and going and going without stopping at all ever",
            30,
        );
        assert!(out.ends_with('…'));
        assert!(!out.contains("goin…"), "clipped mid-word: {out}");
        assert!(out.chars().count() <= 30);
    }

    #[test]
    fn clip_never_splits_a_multibyte_char() {
        let out = clip("ééééééééééééééééééééé", 8);
        assert!(out.chars().count() <= 8);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn nothing_here_panics_on_hostile_input() {
        for s in ["", "   ", "&&", ";;;", "\"unclosed", "cd", "-", "\u{1F600}"] {
            let _ = tool("Bash", s);
            let _ = first_sentence(s, 10);
            let _ = clip(s, 3);
        }
    }
}
