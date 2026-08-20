//! The pane you glance at.
//!
//! Design constraints that outrank everything else, from `CLAUDE.md`:
//!
//! - rating costs exactly one keystroke
//! - the agent's own terminal keeps focus, because this is a separate pane, not a wrapper
//! - a parse that returns nothing says so on screen rather than looking idle
//!
//! Two implementation notes worth keeping:
//!
//! Crossterm emits both a Press and a Release for every keystroke on Windows. Without the
//! `KeyEventKind::Press` filter, every rating fires twice, and only on Windows, which is
//! exactly the sort of bug that survives review on a Mac.
//!
//! The file watcher watches the transcript's parent directory, never the file. notify opens
//! a directory handle that way, so it cannot contend with the harness's own write handle.

use crate::harness;
use crate::moment::{Harness, Moment, MomentKind};
use crate::ratings::{Rating, Store, Verdict};
use crate::tail::Tailer;
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use notify::{RecursiveMode, Watcher};
use ratatui::prelude::*;
use ratatui::DefaultTerminal;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

/// How often the render loop wakes when nothing has happened.
const IDLE_TICK: Duration = Duration::from_millis(250);

#[derive(Debug)]
enum Signal {
    /// Something changed on disk. Coalesced: several notify events collapse into one redraw
    /// rather than one redraw each.
    FileChanged,
    Key(KeyEvent),
    Quit,
}

#[derive(Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Browsing,
    /// Typing the one line of why behind a rejection.
    Noting {
        target: usize,
        buffer: String,
    },
}

struct App {
    harness: Harness,
    path: PathBuf,
    session_id: String,
    moments: Vec<Moment>,
    verdicts: HashMap<String, Verdict>,
    notes: HashMap<String, String>,
    store: Store,
    /// Kept so the store can be rebuilt once the real session id is known, without
    /// reconstructing it by walking back up the store's own path.
    store_root: PathBuf,
    list: ListState,
    /// Whether to stick to the newest moment as new ones arrive.
    following: bool,
    mode: Mode,
    status: Option<String>,
    parsed_lines: usize,
}

impl App {
    fn rateable_count(&self) -> usize {
        self.moments.iter().filter(|m| m.kind.rateable()).count()
    }

    /// Absorb newly appended transcript lines.
    ///
    /// Reparsing the whole file would be simpler, but a rating anchors to a moment's
    /// identity, so a moment must never change index under the cursor while the user is
    /// aiming at it. New moments are appended and existing ones are updated in place, which
    /// is also what lets a tool result fill in the card its call already created.
    fn absorb(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        self.parsed_lines += lines.len();
        let fresh = harness::parse(self.harness, &lines.join("\n"));

        for m in fresh {
            match self.moments.iter_mut().find(|x| x.id == m.id) {
                Some(existing) => *existing = m,
                None => {
                    // Codex only reveals its real session id in session_meta, so the store
                    // starts on a filename-derived placeholder and is corrected here, once.
                    if self.session_id == "unknown" || self.session_id != m.id.session_id {
                        self.session_id = m.id.session_id.clone();
                        self.store = Store::for_session(
                            &self.store_root,
                            self.harness.as_str(),
                            &self.session_id,
                        );
                    }
                    self.moments.push(m);
                }
            }
        }

        if self.following && !self.moments.is_empty() {
            self.list.select(Some(self.moments.len() - 1));
        }
    }

    fn move_by(&mut self, delta: isize) {
        if self.moments.is_empty() {
            return;
        }
        let last = self.moments.len() - 1;
        let cur = self.list.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, last as isize) as usize;
        self.list.select(Some(next));
        // Moving away from the end means the user is inspecting history; stop yanking the
        // cursor to the bottom every time the agent does something.
        self.following = next == last;
    }

    fn rate(&mut self, verdict: Verdict, note: Option<String>) {
        let Some(index) = self.list.selected() else { return };
        let Some(moment) = self.moments.get(index) else { return };

        if !moment.kind.rateable() {
            self.status = Some("that one is yours, not the agent's".into());
            return;
        }

        let rating = Rating {
            moment: moment.id.clone(),
            verdict,
            note: note.clone(),
            at: now_rfc3339(),
            preview: Some(moment.preview(160)),
            subject: Some(subject_of(&moment.kind)),
        };

        let key = moment.id.to_string();
        match self.store.record(&rating) {
            Ok(()) => {
                self.verdicts.insert(key.clone(), verdict);
                if let Some(n) = note {
                    self.notes.insert(key, n);
                }
                self.status = Some("noted, the agent hears it at its next tool call".into());
            }
            Err(e) => self.status = Some(format!("could not save: {e}")),
        }
    }
}

fn subject_of(kind: &MomentKind) -> String {
    match kind {
        MomentKind::Said { .. } => "said".into(),
        MomentKind::Asked { .. } => "asked".into(),
        MomentKind::Thought { .. } => "thought".into(),
        MomentKind::Did { tool, .. } => format!("did:{tool}"),
    }
}

pub fn run(path: PathBuf, harness_kind: Harness, replay: bool) -> Result<()> {
    let home = crate::discover::home()?;
    let root = std::env::var_os("MARGIN_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".margin"));

    let mut tailer = if replay { Tailer::new(&path) } else { Tailer::from_end(&path)? };

    // Session id is not known until a line is parsed, so start with a placeholder and let
    // `absorb` correct it. Codex only reveals it in `session_meta`.
    let session_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());

    let mut app = App {
        harness: harness_kind,
        path: path.clone(),
        session_id: session_id.clone(),
        moments: Vec::new(),
        verdicts: HashMap::new(),
        notes: HashMap::new(),
        store: Store::for_session(&root, harness_kind.as_str(), &session_id),
        store_root: root.clone(),
        list: ListState::default(),
        following: true,
        mode: Mode::default(),
        status: None,
        parsed_lines: 0,
    };

    let initial = tailer.poll()?;
    app.absorb(&initial);
    if !app.moments.is_empty() {
        app.list.select(Some(app.moments.len() - 1));
    }

    let (tx, rx) = mpsc::channel::<Signal>();

    // Keyboard thread. Blocks on read() so an idle session costs nothing.
    let key_tx = tx.clone();
    std::thread::spawn(move || loop {
        match event::read() {
            Ok(Event::Key(k)) => {
                if key_tx.send(Signal::Key(k)).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => {
                let _ = key_tx.send(Signal::Quit);
                break;
            }
        }
    });

    // Watch the parent directory, not the file: notify then holds a directory handle and
    // never contends with the harness's write handle. Events are coalesced into a single
    // FileChanged, since a full drain happens on the next tick anyway.
    let watch_dir = path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let file_tx = tx.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = file_tx.send(Signal::FileChanged);
        }
    })
    .context("starting the file watcher")?;
    watcher
        .watch(&watch_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", watch_dir.display()))?;

    let mut terminal = ratatui::init();
    install_panic_hook();
    let result = event_loop(&mut terminal, &mut app, &mut tailer, &rx);
    ratatui::restore();
    result
}

/// A TUI that leaves the shell in raw mode after a panic is worse than one that never ran.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        original(info);
    }));
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    tailer: &mut Tailer,
    rx: &mpsc::Receiver<Signal>,
) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;

        match rx.recv_timeout(IDLE_TICK) {
            Ok(Signal::Quit) => return Ok(()),
            Ok(Signal::FileChanged) => {
                // Drain any other events that piled up so a burst of writes costs one redraw.
                while rx.try_recv().is_ok() {}
                let lines = tailer.poll()?;
                app.absorb(&lines);
            }
            Ok(Signal::Key(key)) => {
                if handle_key(app, key) {
                    return Ok(());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Some editors and network shares do not produce watch events reliably, so
                // the idle tick also polls. Cheap: a metadata call that usually returns
                // "unchanged" and reads nothing.
                let lines = tailer.poll()?;
                app.absorb(&lines);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Returns true to quit.
fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    // Windows sends Press and Release for every keystroke. Without this, one tap rates
    // twice, and only on Windows.
    if key.kind != KeyEventKind::Press {
        return false;
    }

    if let Mode::Noting { target, buffer } = &mut app.mode {
        match key.code {
            KeyCode::Esc => app.mode = Mode::Browsing,
            KeyCode::Enter => {
                let note = buffer.trim().to_string();
                let target = *target;
                app.mode = Mode::Browsing;
                app.list.select(Some(target));
                app.rate(Verdict::Down, (!note.is_empty()).then_some(note));
            }
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Char(c) => buffer.push(c),
            _ => {}
        }
        return false;
    }

    app.status = None;
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,

        KeyCode::Char('j') | KeyCode::Down => app.move_by(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_by(-1),
        KeyCode::PageDown => app.move_by(10),
        KeyCode::PageUp => app.move_by(-10),

        KeyCode::Char('g') => {
            if !app.moments.is_empty() {
                app.list.select(Some(app.moments.len() - 1));
            }
            app.following = true;
            app.status = Some("following the newest moment".into());
        }

        KeyCode::Char('f') => app.rate(Verdict::Up, None),
        KeyCode::Char('d') => app.rate(Verdict::Down, None),
        KeyCode::Char('D') => {
            if let Some(i) = app.list.selected() {
                app.mode = Mode::Noting { target: i, buffer: String::new() };
            }
        }
        _ => {}
    }
    false
}

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(3),    // moments
        Constraint::Length(1), // status
        Constraint::Length(1), // keys
    ])
    .split(area);

    draw_header(f, chunks[0], app);
    draw_moments(f, chunks[1], app);
    draw_status(f, chunks[2], app);
    draw_keys(f, chunks[3]);

    if let Mode::Noting { buffer, .. } = &app.mode {
        draw_note_prompt(f, area, buffer);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let rated = app.verdicts.len();
    let line = Line::from(vec![
        Span::styled("  margin ", Style::new().fg(ACCENT).bold()),
        Span::styled(
            format!("{} ", app.harness.as_str()),
            Style::new().fg(DIM),
        ),
        Span::styled(short_id(&app.session_id), Style::new().fg(DIM)),
        Span::raw("  "),
        Span::styled(
            format!("{} moments", app.rateable_count()),
            Style::new().fg(DIM),
        ),
        Span::raw("  "),
        Span::styled(format!("{rated} rated"), Style::new().fg(if rated > 0 { ACCENT } else { DIM })),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_moments(f: &mut Frame, area: Rect, app: &mut App) {
    if app.moments.is_empty() {
        // Never look idle when something is wrong. A parse that yields nothing is the most
        // likely symptom of a harness changing its format.
        let msg = if app.parsed_lines == 0 {
            vec![
                Line::from(""),
                Line::from(Span::styled("  Waiting for the agent to do something.", Style::new().fg(DIM))),
                Line::from(""),
                Line::from(Span::styled(format!("  watching {}", app.path.display()), Style::new().fg(DIM))),
            ]
        } else {
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  Read {} lines and recognised none of them.", app.parsed_lines),
                    Style::new().fg(WARN),
                )),
                Line::from(Span::styled(
                    "  The transcript format has probably changed. This is a margin bug, not yours.",
                    Style::new().fg(DIM),
                )),
            ]
        };
        f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: false }), area);
        return;
    }

    let width = area.width.saturating_sub(24) as usize;
    let items: Vec<ListItem> = app
        .moments
        .iter()
        .map(|m| {
            let key = m.id.to_string();
            let verdict = app.verdicts.get(&key).copied();
            let mut spans = vec![
                Span::styled(format!(" {} ", verdict_glyph(verdict)), verdict_style(verdict)),
                Span::styled(format!("{:<9}", clock(m.at.as_deref())), Style::new().fg(DIM)),
                Span::styled(format!("{:<8}", m.kind.label()), kind_style(&m.kind)),
                Span::styled(m.preview(width.max(20)), body_style(&m.kind)),
            ];
            if let Some(note) = app.notes.get(&key) {
                spans.push(Span::styled(format!("  ({note})"), Style::new().fg(WARN).italic()));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(SELECT).bold())
        .highlight_symbol("");

    f.render_stateful_widget(list, area, &mut app.list);

    let mut sb = ScrollbarState::new(app.moments.len()).position(app.list.selected().unwrap_or(0));
    f.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight).style(Style::new().fg(DIM)),
        area,
        &mut sb,
    );
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let text = match (&app.status, app.following) {
        (Some(s), _) => Span::styled(format!("  {s}"), Style::new().fg(ACCENT)),
        (None, true) => Span::styled("  following", Style::new().fg(DIM)),
        (None, false) => Span::styled("  paused, g to follow again", Style::new().fg(DIM)),
    };
    f.render_widget(Paragraph::new(Line::from(text)), area);
}

fn draw_keys(f: &mut Frame, area: Rect) {
    let key = Style::new().fg(ACCENT).bold();
    let lbl = Style::new().fg(DIM);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  j k", key),
            Span::styled(" move   ", lbl),
            Span::styled("f", key),
            Span::styled(" good   ", lbl),
            Span::styled("d", key),
            Span::styled(" bad   ", lbl),
            Span::styled("D", key),
            Span::styled(" bad + why   ", lbl),
            Span::styled("g", key),
            Span::styled(" follow   ", lbl),
            Span::styled("q", key),
            Span::styled(" quit", lbl),
        ])),
        area,
    );
}

fn draw_note_prompt(f: &mut Frame, area: Rect, buffer: &str) {
    let w = area.width.saturating_sub(8).min(80);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + area.height / 2 - 2,
        width: w,
        height: 3,
    };
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" why? ", Style::new().fg(WARN).bold()),
            Span::raw(buffer),
            Span::styled("_", Style::new().fg(ACCENT)),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(WARN)),
        ),
        popup,
    );
}

// Palette. Kept to indexed colours so it survives terminals without true-colour support
// rather than rendering as garbage, which is the opposite of premium.
const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const WARN: Color = Color::Yellow;
const GOOD: Color = Color::Green;
const BAD: Color = Color::Red;
const SELECT: Color = Color::Indexed(236);

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

/// A thought with no text is rendered dim, so the eye can tell at a glance that nothing was
/// persisted rather than that the agent said nothing.
fn body_style(kind: &MomentKind) -> Style {
    match kind {
        MomentKind::Thought { text: None, .. } => Style::new().fg(DIM).italic(),
        MomentKind::Asked { .. } => Style::new().fg(DIM),
        _ => Style::new(),
    }
}

fn clock(at: Option<&str>) -> String {
    at.and_then(|s| s.split_once('T'))
        .map(|(_, t)| t.split(['.', 'Z', '+']).next().unwrap_or(t).to_string())
        .unwrap_or_else(|| "--:--:--".into())
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_labels_separate_tools_from_prose() {
        assert_eq!(subject_of(&MomentKind::Said { text: "x".into() }), "said");
        assert_eq!(
            subject_of(&MomentKind::Did {
                tool: "Bash".into(),
                input: "ls".into(),
                output: None,
                tool_use_id: None
            }),
            "did:Bash"
        );
        assert_eq!(subject_of(&MomentKind::Thought { text: None, bytes: 0 }), "thought");
    }

    #[test]
    fn clock_shortens_a_timestamp_and_survives_a_missing_one() {
        assert_eq!(clock(Some("2026-08-20T12:04:19.412Z")), "12:04:19");
        assert_eq!(clock(None), "--:--:--");
        assert_eq!(clock(Some("garbage")), "--:--:--");
    }
}
