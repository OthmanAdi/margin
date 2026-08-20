use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use margin::discover;
use margin::harness;
use margin::inject::{self, Trigger};
use margin::moment::Harness;
use margin::ratings::Store;
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
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Watch { session, replay } => watch(session, replay),
        Command::Sessions { all, limit } => sessions(all, limit),
        Command::Hook { event } => hook(&event),
        Command::Snapshot { out, cols, rows } => snapshot(out, cols, rows),
        Command::Install { write, settings } => install(write, settings),
    }
}

fn watch(session: Option<PathBuf>, replay: bool) -> Result<()> {
    let home = discover::home()?;
    let cwd = std::env::current_dir()?;

    let (path, harness_kind) = match session {
        Some(p) => {
            let text = std::fs::read_to_string(&p)
                .with_context(|| format!("reading {}", p.display()))?;
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

    let pending = store.pending().unwrap_or_default();
    let Some(context) = inject::render(&pending, trigger) else {
        return Ok(()); // nothing rated since last time: emit nothing at all
    };

    // Mark delivered before printing. If the process dies between the two, a rating is lost
    // rather than repeated, and losing one is far less damaging to a context than looping
    // the same complaint at every tool call.
    let ids: Vec<_> = pending.iter().map(|r| r.moment.clone()).collect();
    store.mark_delivered(&ids, &now_rfc3339()).ok();

    println!("{}", inject::hook_output(&context, trigger));
    Ok(())
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

fn install(write: bool, settings: Option<PathBuf>) -> Result<()> {
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
        println!("{pretty}");
        println!();
        println!("Merge that into your Claude Code settings, or rerun with --write.");
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

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&existing)?)
        .with_context(|| format!("writing {}", path.display()))?;
    println!("Installed margin hooks into {}", path.display());
    Ok(())
}

fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default()
}
