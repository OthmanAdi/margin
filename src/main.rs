use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use margin::discover;
use margin::harness;
use margin::inject::{self, Trigger};
use margin::moment::Harness;
use margin::ratings::{Store, Verdict};
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "margin",
    version,
    about = "Rate your coding agent while it runs. One keystroke, no interruption."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Watch a running session and rate its moments.
    Watch {
        /// Transcript to follow. Defaults to the newest session for this directory.
        #[arg(long)]
        session: Option<PathBuf>,
        /// Show the whole session rather than only what happens from now on.
        #[arg(long)]
        replay: bool,
    },

    /// List sessions margin can see, newest first.
    Sessions {
        /// Include sessions from other working directories.
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },

    /// Hand pending ratings to the running agent. Called by a hook, not by a human.
    ///
    /// Reads the harness's hook payload on stdin and prints the injection JSON on stdout.
    /// Prints nothing when there is nothing pending, which is the common case.
    Hook {
        /// Which hook event is firing.
        #[arg(value_parser = ["PostToolUse", "Stop", "UserPromptSubmit"])]
        event: String,
    },

    /// Print a one-line margin segment for Claude Code's own status line.
    ///
    /// Reads the status-line payload on stdin and prints a short segment, or nothing at all
    /// when there is nothing to say. Runs on every repaint, so it must stay cheap and must
    /// never write anything that could break the host's line.
    Statusline {
        /// Run this command first and print its output before margin's segment, so an
        /// existing status line keeps working instead of being replaced.
        #[arg(long)]
        wrap: Option<String>,
        /// Separator placed between the wrapped output and margin's segment.
        #[arg(long, default_value = "  ")]
        sep: String,
    },

    /// Render the UI to an SVG for the README. Regenerates docs/img/.
    Snapshot {
        #[arg(long, default_value = "docs/img/margin.svg")]
        out: PathBuf,
        #[arg(long, default_value_t = 96)]
        cols: u16,
        #[arg(long, default_value_t = 20)]
        rows: u16,
    },

    /// Print the hook configuration to add to Claude Code's settings.
    Install {
        /// Write it into the settings file instead of printing it.
        #[arg(long)]
        write: bool,
        /// Which settings file to write. Defaults to the user's.
        #[arg(long)]
        settings: Option<PathBuf>,
        /// Also take over the status line, wrapping any existing one so it keeps working.
        #[arg(long)]
        statusline: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Watch { session, replay } => watch(session, replay),
        Command::Sessions { all, limit } => sessions(all, limit),
        Command::Hook { event } => hook(&event),
        Command::Statusline { wrap, sep } => statusline(wrap, sep),
        Command::Snapshot { out, cols, rows } => snapshot(out, cols, rows),
        Command::Install {
            write,
            settings,
            statusline: sl,
        } => install(write, settings, sl),
    }
}

fn watch(session: Option<PathBuf>, replay: bool) -> Result<()> {
    let home = discover::home()?;
    let cwd = std::env::current_dir()?;

    let (path, harness_kind) = match session {
        Some(p) => {
            let text =
                std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
            let h = harness::detect(&text).context(
                "could not tell which harness wrote this file; it does not look like a \
                 Claude Code or Codex transcript",
            )?;
            (p, h)
        }
        None => {
            let s = discover::newest_for_cwd(&home, &cwd).context(
                "no session found. Start Claude Code or Codex first, or pass --session <path>",
            )?;
            (s.path, s.harness)
        }
    };

    margin::ui::run(path, harness_kind, replay)
}

fn sessions(all: bool, limit: usize) -> Result<()> {
    let home = discover::home()?;
    let cwd = std::env::current_dir()?;

    let found = if all {
        discover::all_sessions(&home)
    } else {
        let mut v = discover::claude_sessions(&home, Some(&cwd));
        v.extend(discover::codex_sessions(&home));
        v.sort_by_key(|s| std::cmp::Reverse(s.modified));
        v
    };

    if found.is_empty() {
        println!("No sessions found under {}.", home.display());
        println!("Start Claude Code or Codex, then run this again.");
        return Ok(());
    }

    for s in found.iter().take(limit) {
        let text = std::fs::read_to_string(&s.path).unwrap_or_default();
        let moments = harness::parse(s.harness, &text);
        let rateable = moments.iter().filter(|m| m.kind.rateable()).count();
        println!(
            "{:<12} {:<40} {:>4} moments  {}",
            s.harness.as_str(),
            s.id(),
            rateable,
            s.path.display()
        );
    }
    Ok(())
}

/// The whole point, in one function.
///
/// Runs inside the agent's own process, must be fast, and must be silent when it has
/// nothing to say. Any failure here exits 0 with no output: a broken feedback tool must
/// never break the agent it is attached to.
fn hook(event: &str) -> Result<()> {
    let trigger = match event {
        "Stop" => Trigger::Stop,
        "UserPromptSubmit" => Trigger::UserPromptSubmit,
        _ => Trigger::PostToolUse,
    };

    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok();
    let payload: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);

    let Some(session_id) = payload.get("session_id").and_then(|v| v.as_str()) else {
        return Ok(()); // not a hook payload we understand; stay quiet
    };

    let home = discover::home()?;
    let store = Store::for_session(&store_root(&home), Harness::ClaudeCode.as_str(), session_id);

    // Leave proof of life before deciding whether to speak. A hook that only writes when it
    // has something to say is indistinguishable from a hook that was never loaded, and
    // "installed but inert until restart" is the first trap a new user hits.
    store.touch_heartbeat();

    let pending = store.pending().unwrap_or_default();
    let Some(context) = inject::render(&pending, trigger) else {
        return Ok(()); // nothing rated since last time: emit nothing at all
    };

    // Mark delivered before printing. If the process dies between the two, a rating is lost
    // rather than repeated, and losing one is far less damaging to a context than looping
    // the same complaint at every tool call.
    let ids: Vec<_> = pending.iter().map(|r| r.moment.clone()).collect();
    store.mark_delivered(&ids, &now_rfc3339()).ok();

    // additionalContext reaches the model but is invisible to the human. systemMessage
    // prints into the agent's own window, which is the only confirmation the user gets that
    // a keypress in another pane actually did something.
    println!(
        "{}",
        inject::hook_output_with_notice(&context, trigger, &inject::notice(&pending))
    );
    Ok(())
}

/// The in-window surface.
///
/// Claude Code has no plugin API for its TUI, so a second pane is the only place to render a
/// full list. The status line is the one piece of its own window a tool can legitimately
/// write to, and it is the difference between "is this even on?" and knowing.
///
/// Every failure path here prints nothing and exits 0. A status line that errors would
/// scribble on the user's own line on every repaint, which is worse than being absent.
fn statusline(wrap: Option<String>, sep: String) -> Result<()> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok();

    // Prefer the command stored in a file over one passed as an argument.
    //
    // Both forms work: `cmd /c "<exe>" statusline --wrap "<inner>"` was verified to run
    // correctly. The file exists because the registered command then contains no nested
    // quotes at all, which keeps it readable in settings.json and portable across whichever
    // shell a host picks. `powershell -Command`, for instance, rejects a quoted leading path
    // without a call operator, while cmd accepts it.
    let wrap_cmd = wrap.or_else(read_wrap_file);
    let wrapped = wrap_cmd.as_deref().and_then(|cmd| run_wrapped(cmd, &raw));

    let segment = margin_segment(&raw);

    match (wrapped, segment) {
        (Some(w), Some(m)) => println!("{}{}{}", w.trim_end(), sep, m),
        (Some(w), None) => println!("{}", w.trim_end()),
        (None, Some(m)) => println!("{m}"),
        (None, None) => {}
    }
    Ok(())
}

fn wrap_file() -> Option<PathBuf> {
    Some(store_root(&discover::home().ok()?).join("statusline-wrap"))
}

fn read_wrap_file() -> Option<String> {
    let text = std::fs::read_to_string(wrap_file()?).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn run_wrapped(cmd: &str, stdin_payload: &str) -> Option<String> {
    use std::io::Write as _;
    use std::process::{Command as Proc, Stdio};

    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let mut child = Proc::new(shell)
        .arg(flag)
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(stdin_payload.as_bytes());
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    (!text.is_empty()).then_some(text)
}

/// None means stay silent, which is the correct output for a session nobody has rated.
fn margin_segment(payload: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let session_id = v.get("session_id")?.as_str()?;

    let home = discover::home().ok()?;
    let store = Store::for_session(&store_root(&home), Harness::ClaudeCode.as_str(), session_id);

    let all = store.all().ok()?;
    if all.is_empty() {
        return None;
    }
    let pending = store.pending().unwrap_or_default();
    let up = pending.iter().filter(|r| r.verdict == Verdict::Up).count();
    let down = pending
        .iter()
        .filter(|r| r.verdict == Verdict::Down)
        .count();
    let sent = all.len().saturating_sub(pending.len());

    // 256-colour SGR rather than truecolour: a status line is repainted constantly and has
    // to survive whatever terminal the user is in, including ones that render 24-bit escapes
    // as literal garbage. NO_COLOR is honoured because status lines get scraped and logged.
    let plain = std::env::var_os("NO_COLOR").is_some();
    let c = |code: &str, text: String| {
        if plain {
            text
        } else {
            format!("[38;5;{code}m{text}[0m")
        }
    };

    let mut parts: Vec<String> = Vec::new();
    if up > 0 {
        parts.push(c("108", format!("+{up}")));
    }
    if down > 0 {
        parts.push(c("174", format!("-{down}")));
    }
    if !parts.is_empty() {
        parts.push(c("245", "queued".into()));
    }
    if sent > 0 {
        parts.push(c("245", format!("{sent} sent")));
    }
    // Rated but nothing queued and nothing sent should still say something, otherwise the
    // segment vanishes exactly when the user is checking whether it works.
    if parts.is_empty() {
        parts.push(c("245", format!("{} rated", all.len())));
    }
    if !store.hook_seen() {
        parts.push(c("179", "hook not loaded".into()));
    }
    Some(format!("{} {}", c("109", "margin".into()), parts.join(" ")))
}

fn snapshot(out: PathBuf, cols: u16, rows: u16) -> Result<()> {
    margin::snapshot::write_svg(&out, cols, rows, "margin", margin::ui::draw_demo)?;
    println!("wrote {} ({cols}x{rows})", out.display());

    // Second image: what the agent actually receives. The README claims mid-run steering
    // works, so it should show the payload rather than describe it.
    let signal = out.with_file_name("signal.svg");
    let rows2 = margin::snapshot::draw_signal_rows();
    margin::snapshot::write_svg(&signal, cols, rows2, "what the agent receives", |f| {
        margin::snapshot::draw_signal(f)
    })?;
    println!("wrote {} ({cols}x{rows2})", signal.display());
    Ok(())
}

fn store_root(home: &std::path::Path) -> PathBuf {
    std::env::var_os("MARGIN_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".margin"))
}

fn install(write: bool, settings: Option<PathBuf>, statusline: bool) -> Result<()> {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "margin".into());

    let config = serde_json::json!({
        "hooks": {
            "PostToolUse": [{
                "matcher": "*",
                "hooks": [{ "type": "command", "command": format!("{exe} hook PostToolUse") }]
            }],
            "Stop": [{
                "hooks": [{ "type": "command", "command": format!("{exe} hook Stop") }]
            }]
        }
    });
    let pretty = serde_json::to_string_pretty(&config)?;

    if !write {
        // JSON on stdout, prose on stderr. Printing both to stdout made the output
        // unparseable, so anyone piping this into jq got a syntax error at the last line.
        println!("{pretty}");
        eprintln!();
        eprintln!("Merge that into your Claude Code settings, or rerun with --write.");
        return Ok(());
    }

    let path = match settings {
        Some(p) => p,
        None => discover::home()?.join(".claude").join("settings.json"),
    };

    let mut existing: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    // Merge into the existing hook arrays rather than replacing them. Clobbering someone's
    // other hooks to install a feedback tool would be an unusually rude way to introduce
    // yourself.
    let hooks = existing
        .as_object_mut()
        .context("settings file is not a JSON object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    for event in ["PostToolUse", "Stop"] {
        let ours = config["hooks"][event][0].clone();
        let arr = hooks
            .as_object_mut()
            .context("hooks is not a JSON object")?
            .entry(event)
            .or_insert_with(|| serde_json::json!([]));
        let list = arr.as_array_mut().context("hook event is not an array")?;
        let already = list
            .iter()
            .any(|e| e.to_string().contains("margin") && e.to_string().contains(event));
        if !already {
            list.push(ours);
        }
    }

    if statusline {
        // Wrap whatever is already there rather than replacing it. Someone's status line is
        // usually a thing they built and care about; silently deleting it to advertise a
        // feedback tool would be a poor introduction.
        let existing_cmd = existing
            .get("statusLine")
            .and_then(|s| s.get("command"))
            .and_then(|c| c.as_str())
            .map(str::to_string);

        // Anything already there is stored in a file, so the registered command stays free
        // of nested quotes. Re-running install must not wrap our own wrapper.
        if let Some(prev) = existing_cmd.filter(|c| !c.contains("margin")) {
            if let Some(f) = wrap_file() {
                if let Some(parent) = f.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(&f, prev).ok();
            }
        }
        let new_cmd = format!("\"{exe}\" statusline");

        existing
            .as_object_mut()
            .context("settings file is not a JSON object")?
            .insert(
                "statusLine".into(),
                serde_json::json!({ "type": "command", "command": new_cmd }),
            );
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&existing)?)
        .with_context(|| format!("writing {}", path.display()))?;
    println!("Installed margin hooks into {}", path.display());
    println!();
    println!("This does NOT affect sessions that are already running. Claude Code reads");
    println!("settings.json at session start, so restart it before rating anything.");
    println!("margin watch shows `hook: live` once it has actually fired.");
    if statusline {
        println!();
        println!("Status line wired. Any status line you already had is wrapped, not replaced.");
    }
    Ok(())
}

fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}
