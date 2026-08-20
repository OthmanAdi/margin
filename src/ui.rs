//! The pane you glance at.
//!
//! Design constraints that outrank everything else, from `CLAUDE.md`:
//!
//! - rating costs exactly one keystroke once this pane has focus
//! - this pane never intercepts a key the agent's terminal wanted
//! - a parse that returns nothing says so on screen rather than looking idle
//!
//! The second is not the same as "the agent keeps focus", and an earlier version of this
//! comment conflated them. A terminal pane only receives keys when it is focused, so rating
//! costs a pane switch, then one key. The honest claim is that margin never steals a key
//! from the agent, not that focus never moves.
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
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use notify::{RecursiveMode, Watcher};
use ratatui::layout::Margin;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::DefaultTerminal;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How often the render loop wakes when nothing has happened.
///
/// 60ms rather than 250ms. The rating flash below lasts 180ms, and a 250ms tick could not
/// reliably clear it, so the highlight lingered or never appeared depending on when the key
/// landed relative to the tick. An idle wake is a metadata call and a redraw of a screen that
/// has not changed, which is cheap enough to pay for feedback that feels immediate.
const IDLE_TICK: Duration = Duration::from_millis(60);

/// How long a row stays lit after being rated.
///
/// Long enough to register as a deliberate confirmation, short enough that it is gone before
/// the eye moves on. Under about 100ms reads as a flicker; over about 300ms reads as lag.
const FLASH: Duration = Duration::from_millis(180);

#[derive(Debug)]
enum Signal {
    /// Something changed on disk. Coalesced: several notify events collapse into one redraw
    /// rather than one redraw each.
    FileChanged,
    Key(KeyEvent),
    Mouse(MouseEvent),
    Quit,
}

#[derive(Debug, Default, PartialEq)]
enum Mode {
    #[default]
    Browsing,
    /// Typing the one line of why behind a rejection.
    Noting { target: usize, buffer: String },
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
    /// How many moments sit below the cursor. Shown so the user knows work is happening
    /// further down without the cursor being yanked there.
    unseen: usize,
    mode: Mode,
    status: Option<String>,
    parsed_lines: usize,
    /// Whether the hook has ever fired for this session. Answers the first question a new
    /// user has, which is whether any of this is actually wired up.
    hook_live: bool,
    /// Row index and expiry of the confirmation flash, set on a rating.
    flash: Option<(usize, Instant)>,
    /// How many ratings were still undelivered when the session ended, if it has ended.
    ///
    /// The one real deadline this tool has. Transcripts and ratings are permanent, but a hook
    /// only fires inside a live session, so a rating still queued when the session closes
    /// never reaches anyone.
    stranded: Option<usize>,
}

impl App {
    /// Re-read the store so marks survive a restart of the pane.
    ///
    /// Called once at startup and again whenever the store is repointed at the real session
    /// id, which for Codex only becomes known after the first parsed line.
    fn reload_ratings(&mut self) {
        let Ok(all) = self.store.all() else { return };
        self.verdicts.clear();
        self.notes.clear();
        for r in all {
            let key = r.moment.to_string();
            // Last write wins, matching how the store resolves a moment rated twice.
            self.verdicts.insert(key.clone(), r.verdict);
            match r.note {
                Some(n) if !n.trim().is_empty() => {
                    self.notes.insert(key, n);
                }
                _ => {
                    self.notes.remove(&key);
                }
            }
        }
    }

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
        let mut reload_after = false;

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
                        reload_after = true;
                    }
                    self.moments.push(m);
                }
            }
        }

        if reload_after {
            self.reload_ratings();
        }

        // The cursor never moves on its own.
        //
        // It used to jump to the newest moment whenever one arrived while following. That is
        // a correctness bug, not a preference: a moment landing between the user looking at
        // a row and pressing a key means the keypress rates something they never saw. Wrong
        // target is the worst failure this tool has, so new moments now only ever appear
        // below the cursor and `g` is the one way to jump.
        if self.list.selected().is_none() && !self.moments.is_empty() {
            self.list.select(Some(self.moments.len() - 1));
        }
        self.unseen = self
            .moments
            .len()
            .saturating_sub(self.list.selected().map_or(0, |i| i + 1));
    }

    fn move_by(&mut self, delta: isize) {
        if self.moments.is_empty() {
            return;
        }
        let last = self.moments.len() - 1;
        let cur = self.list.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, last as isize) as usize;
        self.list.select(Some(next));
        self.unseen = last - next;
    }

    fn rate(&mut self, verdict: Verdict, note: Option<String>) {
        let Some(index) = self.list.selected() else {
            return;
        };
        let Some(moment) = self.moments.get(index) else {
            return;
        };

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
                self.flash = Some((index, Instant::now() + FLASH));
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

    let mut tailer = if replay {
        Tailer::new(&path)
    } else {
        Tailer::from_end(&path)?
    };

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
        unseen: 0,
        mode: Mode::default(),
        status: None,
        parsed_lines: 0,
        hook_live: false,
        flash: None,
        stranded: None,
    };

    // Ratings already on disk must reappear as marks. Keeping verdicts only in memory means
    // restarting the pane silently loses every judgment the user already made, which reads
    // as "it forgot" and is the fastest way to stop being trusted.
    app.reload_ratings();

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
            Ok(Event::Mouse(m)) => {
                if key_tx.send(Signal::Mouse(m)).is_err() {
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
    let watch_dir = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
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
    // Scrolling with the wheel is the other natural way to move a cursor in a list. The cost
    // is that the terminal's own click-drag text selection stops working while margin runs,
    // which is the standard trade every mouse-aware TUI makes.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let result = event_loop(&mut terminal, &mut app, &mut tailer, &rx);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// A TUI that leaves the shell in raw mode after a panic is worse than one that never ran.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
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
                // Coalesce further file events so a burst of writes costs one redraw, but
                // keep any keypresses that arrived in the same window. Draining the channel
                // indiscriminately silently ate ratings: the user pressed a key, the file
                // happened to change first, and the keypress was discarded with the
                // duplicate file events.
                let mut keys = Vec::new();
                let mut mice = Vec::new();
                while let Ok(sig) = rx.try_recv() {
                    match sig {
                        Signal::Key(k) => keys.push(k),
                        Signal::Mouse(m) => mice.push(m),
                        Signal::Quit => return Ok(()),
                        Signal::FileChanged => {}
                    }
                }
                let lines = tailer.poll()?;
                app.absorb(&lines);
                for m in mice {
                    handle_mouse(app, m);
                }
                for k in keys {
                    if handle_key(app, k) {
                        return Ok(());
                    }
                }
            }
            Ok(Signal::Key(key)) => {
                if handle_key(app, key) {
                    return Ok(());
                }
            }
            Ok(Signal::Mouse(m)) => handle_mouse(app, m),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                app.hook_live = app.store.hook_seen();
                app.stranded = app.store.stranded();
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

fn handle_mouse(app: &mut App, m: MouseEvent) {
    // Three rows per notch, which is what most terminals and editors use. One row per notch
    // feels stuck on a list this dense; a full page feels like it jumped.
    match m.kind {
        MouseEventKind::ScrollUp => app.move_by(-3),
        MouseEventKind::ScrollDown => app.move_by(3),
        _ => {}
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
            app.unseen = 0;
            app.status = Some("jumped to the newest moment".into());
        }

        KeyCode::Char('f') => app.rate(Verdict::Up, None),
        KeyCode::Char('d') => app.rate(Verdict::Down, None),
        KeyCode::Char('D') => {
            if let Some(i) = app.list.selected() {
                app.mode = Mode::Noting {
                    target: i,
                    buffer: String::new(),
                };
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
        Constraint::Length(4), // what is selected, in full
        Constraint::Length(1), // status
        Constraint::Length(1), // keys
    ])
    .split(area);

    draw_header(f, chunks[0], app);
    draw_moments(f, chunks[1], app);
    draw_detail(f, chunks[2], app);
    draw_status(f, chunks[3], app);
    draw_keys(f, chunks[4]);

    if let Mode::Noting { buffer, .. } = &app.mode {
        draw_note_prompt(f, area, buffer);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let rated = app.verdicts.len();
    let line = Line::from(vec![
        Span::styled("  margin ", Style::new().fg(ACCENT).bold()),
        Span::styled(format!("{} ", app.harness.as_str()), Style::new().fg(DIM)),
        Span::styled(short_id(&app.session_id), Style::new().fg(DIM)),
        Span::raw("  "),
        Span::styled(
            format!("{} moments", app.rateable_count()),
            Style::new().fg(DIM),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{rated} rated"),
            Style::new().fg(if rated > 0 { ACCENT } else { DIM }),
        ),
        Span::raw("  "),
        if let Some(n) = app.stranded.filter(|n| *n > 0) {
            Span::styled(
                format!("session ended, {n} never sent"),
                Style::new().fg(BAD).bold(),
            )
        } else if app.hook_live {
            Span::styled("hook: live", Style::new().fg(GOOD))
        } else {
            // Not an error: it is also what a brand new session looks like before the
            // agent's first tool call. Saying "not seen yet" avoids crying wolf while still
            // pointing at the real cause when it is the real cause.
            Span::styled("hook: not seen yet", Style::new().fg(WARN))
        },
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
                Line::from(Span::styled(
                    "  Waiting for the agent to do something.",
                    Style::new().fg(DIM),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  watching {}", app.path.display()),
                    Style::new().fg(DIM),
                )),
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
    let now = Instant::now();
    let flashing = app.flash.filter(|(_, until)| *until > now).map(|(i, _)| i);

    let items: Vec<ListItem> = app
        .moments
        .iter()
        .enumerate()
        .map(|(idx, m)| {
            let key = m.id.to_string();
            let verdict = app.verdicts.get(&key).copied();
            let mut spans = vec![
                // The rating mark gets its own column, separate from the cursor. Overloading
                // one glyph with "where you are" and "what you decided" made a rated row and
                // the selected row compete for the same cue.
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
            if let Some(note) = app.notes.get(&key) {
                spans.push(Span::styled(
                    format!("  ({note})"),
                    Style::new().fg(WARN).italic(),
                ));
            }
            let item = ListItem::new(Line::from(spans));
            // The confirmation flash. A whole-row wash for a moment, so a keypress is
            // unmistakably registered without the eye having to find a small glyph.
            match flashing {
                Some(f) if f == idx => item.style(
                    Style::new()
                        .bg(match verdict {
                            Some(Verdict::Down) => FLASH_BAD,
                            _ => FLASH_GOOD,
                        })
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
                _ => item,
            }
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM));

    let list = List::new(items)
        .block(block)
        // Background only, no foreground. Setting a foreground here would flatten the
        // per-column colour coding on exactly the row the user is reading most carefully.
        .highlight_style(Style::new().bg(SELECT))
        // A solid left bar, always reserved, so rows never shift sideways as the cursor moves.
        .highlight_symbol("▌")
        .highlight_spacing(ratatui::widgets::HighlightSpacing::Always);

    f.render_stateful_widget(list, area, &mut app.list);

    // Only when there is something to scroll. A full-height thumb on a list that fits is
    // noise, and drawn over the border it reads as a rendering bug.
    let viewport = area.height.saturating_sub(2) as usize;
    if app.moments.len() > viewport {
        let mut sb = ScrollbarState::new(app.moments.len().saturating_sub(viewport))
            .position(app.list.selected().unwrap_or(0));
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_style(Style::new().fg(DIM)),
            // inset by one row so the thumb sits inside the rounded border, not on it
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut sb,
        );
    }
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let text = match (&app.status, app.unseen) {
        (Some(s), _) => Span::styled(format!("  {s}"), Style::new().fg(ACCENT)),
        (None, 0) => Span::styled("  at the newest moment", Style::new().fg(DIM)),
        (None, n) => Span::styled(
            format!("  {n} newer below, g to jump"),
            Style::new().fg(WARN),
        ),
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
            Span::styled(" newest   ", lbl),
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
/// Selection background. Dark enough that the per-column colours still read on top of it,
/// light enough to locate instantly in a dense list.
const SELECT: Color = Color::Indexed(237);
/// Confirmation wash after an approval and after a rejection.
const FLASH_GOOD: Color = Color::Indexed(114);
const FLASH_BAD: Color = Color::Indexed(174);

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
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// Build a representative screen and draw it, for the README image.
///
/// Uses the committed Claude Code fixture, so the picture shows real parsed moments,
/// including the thought Claude Code never persisted. Two ratings are pre-set to show what
/// an approval and a rejection with a note look like.
pub fn draw_demo(f: &mut Frame) {
    let fixture = include_str!("../fixtures/claude-code/session-basic.jsonl");
    let mut moments = harness::parse(Harness::ClaudeCode, fixture);

    // The fixture is one short session; extend it with a few more moments so the picture
    // shows a realistic run rather than four lines in a large empty box.
    moments.extend(demo_extra_moments(moments.len()));

    let mut verdicts = HashMap::new();
    let mut notes = HashMap::new();
    if let Some(m) = moments
        .iter()
        .find(|m| matches!(m.kind, MomentKind::Said { .. }))
    {
        verdicts.insert(m.id.to_string(), Verdict::Up);
    }
    if let Some(m) = moments
        .iter()
        .find(|m| matches!(m.kind, MomentKind::Did { .. }))
    {
        verdicts.insert(m.id.to_string(), Verdict::Down);
        notes.insert(
            m.id.to_string(),
            "wrong file, use the debug log".to_string(),
        );
    }

    let mut list = ListState::default();
    list.select(Some(moments.len().saturating_sub(2)));

    let mut app = App {
        harness: Harness::ClaudeCode,
        path: PathBuf::from("~/.claude/projects/margin/session.jsonl"),
        session_id: "9c42ba52-3bf1-449f-a040-8ee33284a1c8".into(),
        moments,
        verdicts,
        notes,
        store: Store::for_session(std::path::Path::new("/tmp"), "claude-code", "demo"),
        store_root: PathBuf::from("/tmp"),
        list,
        unseen: 3,
        mode: Mode::Browsing,
        status: Some("noted, the agent hears it at its next tool call".into()),
        parsed_lines: 42,
        hook_live: true,
        flash: None,
        stranded: None,
    };
    draw(f, &mut app);
}

fn demo_extra_moments(from: usize) -> Vec<Moment> {
    use crate::moment::MomentId;
    let at = |s: &str| Some(format!("2026-08-20T{s}Z"));
    let mk = |i: usize, t: &str, kind: MomentKind| Moment {
        id: MomentId::new(Harness::ClaudeCode, "demo", format!("d{i}"), 0),
        seq: from + i,
        at: at(t),
        kind,
    };
    vec![
        mk(
            0,
            "12:04:31",
            MomentKind::Thought {
                text: None,
                bytes: 4524,
            },
        ),
        mk(
            1,
            "12:04:38",
            MomentKind::Said {
                text: "0 of 71 thinking blocks have readable text. It is signature only.".into(),
            },
        ),
        mk(
            2,
            "12:04:52",
            MomentKind::Did {
                tool: "Read".into(),
                input: "src/harness/claude_code.rs".into(),
                output: Some("ok".into()),
                tool_use_id: Some("toolu_02".into()),
                intent: Some("check what the parser reads".into()),
            },
        ),
        mk(
            3,
            "12:05:03",
            MomentKind::Thought {
                text: None,
                bytes: 3180,
            },
        ),
        mk(
            4,
            "12:05:11",
            MomentKind::Said {
                text: "Switching to the streaming interface, where the text is available.".into(),
            },
        ),
    ]
}

/// What is under the cursor, in full.
///
/// The list clips every row to one line, which is what makes it skimmable, but it means the
/// thing you are about to rate is usually truncated. Pressing a key on a half-read row is a
/// guess. This pane removes the guess: it shows the selected moment wrapped, with what it
/// was and what came back.
fn draw_detail(f: &mut Frame, area: Rect, app: &App) {
    let Some(m) = app.list.selected().and_then(|i| app.moments.get(i)) else {
        return;
    };
    let key = m.id.to_string();
    let verdict = app.verdicts.get(&key).copied();

    let head = Line::from(vec![
        Span::styled(" ▎", Style::new().fg(ACCENT)),
        Span::styled(
            format!("{} ", m.kind.label()),
            kind_style(&m.kind).add_modifier(Modifier::BOLD),
        ),
        Span::styled(clock(m.at.as_deref()), Style::new().fg(DIM)),
        match verdict {
            Some(Verdict::Up) => Span::styled("  approved", Style::new().fg(GOOD).bold()),
            Some(Verdict::Down) => Span::styled("  rejected", Style::new().fg(BAD).bold()),
            None => Span::styled("  not rated", Style::new().fg(DIM)),
        },
        match &m.kind {
            MomentKind::Did {
                output: Some(o), ..
            } => Span::styled(
                format!("   returned {}", size_of(o.len())),
                Style::new().fg(DIM),
            ),
            MomentKind::Did { output: None, .. } => {
                Span::styled("   still running", Style::new().fg(WARN))
            }
            _ => Span::raw(""),
        },
    ]);

    // The full text, not the row's clipped version.
    let body = match &m.kind {
        MomentKind::Asked { text } | MomentKind::Said { text } => crate::humanize::collapse(text),
        MomentKind::Thought {
            text: Some(t),
            ..
        } => crate::humanize::collapse(t),
        MomentKind::Thought { text: None, bytes } => format!(
            "Claude Code does not persist thinking text, so there is nothing to show. {} of reasoning happened here.",
            size_of(*bytes)
        ),
        MomentKind::Did { tool, input, .. } => format!("{tool}  {}", crate::humanize::collapse(input)),
    };

    let mut lines = vec![head];
    lines.push(Line::from(Span::styled(
        format!(
            "  {}",
            crate::humanize::clip(&body, area.width.saturating_sub(4) as usize * 2)
        ),
        body_style(&m.kind),
    )));
    if let Some(note) = app.notes.get(&key) {
        lines.push(Line::from(Span::styled(
            format!("  why: {note}"),
            Style::new().fg(WARN).italic(),
        )));
    }

    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(DIM)),
        ),
        area,
    );
}

/// Byte counts a person reads without counting digits.
fn size_of(n: usize) -> String {
    if n >= 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1} kB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn app_for_test(moments: Vec<Moment>) -> App {
        let mut list = ListState::default();
        if !moments.is_empty() {
            list.select(Some(moments.len() - 1));
        }
        App {
            harness: Harness::ClaudeCode,
            path: PathBuf::from("t.jsonl"),
            session_id: "t".into(),
            moments,
            verdicts: HashMap::new(),
            notes: HashMap::new(),
            store: Store::for_session(
                &std::env::temp_dir().join("margin-ui-test"),
                "claude-code",
                "t",
            ),
            store_root: std::env::temp_dir().join("margin-ui-test"),
            list,
            unseen: 0,
            mode: Mode::Browsing,
            status: None,
            parsed_lines: 0,
            hook_live: false,
            flash: None,
            stranded: None,
        }
    }

    fn line(uuid: &str, text: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "uuid": uuid,
            "sessionId": "t",
            "timestamp": "2026-08-20T12:00:00Z",
            "message": { "role": "assistant", "content": [ { "type": "text", "text": text } ] }
        })
        .to_string()
    }

    /// The worst bug this tool can have is rating the wrong moment. New moments arriving
    /// must never move the cursor out from under a keypress.
    #[test]
    fn arriving_moments_never_move_the_cursor() {
        let mut app = app_for_test(Vec::new());
        app.absorb(&[line("a", "first"), line("b", "second")]);

        let aimed_at = app.list.selected().unwrap();
        let aimed_id = app.moments[aimed_at].id.clone();

        // the agent keeps working while the user is deciding
        app.absorb(&[line("c", "third"), line("d", "fourth")]);

        assert_eq!(
            app.list.selected(),
            Some(aimed_at),
            "cursor moved on its own"
        );
        assert_eq!(app.moments[app.list.selected().unwrap()].id, aimed_id);
        assert_eq!(app.unseen, 2, "should report how many arrived below");
    }

    #[test]
    fn the_cursor_only_moves_on_a_keypress() {
        let mut app = app_for_test(Vec::new());
        app.absorb(&[line("a", "one"), line("b", "two"), line("c", "three")]);
        let start = app.list.selected().unwrap();

        app.move_by(-1);
        assert_eq!(app.list.selected(), Some(start - 1));
        assert_eq!(app.unseen, 1);

        app.move_by(-5); // clamps rather than underflowing
        assert_eq!(app.list.selected(), Some(0));
    }

    /// A rating must land on whatever the cursor is on, not on whatever is newest.
    #[test]
    fn rating_targets_the_selected_moment_not_the_newest() {
        let dir = std::env::temp_dir().join(format!("margin-ui-rate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = app_for_test(Vec::new());
        app.store = Store::for_session(&dir, "claude-code", "t");
        app.store_root = dir.clone();

        app.absorb(&[line("a", "one"), line("b", "two"), line("c", "three")]);
        app.move_by(-2); // aim at the oldest
        let target = app.moments[app.list.selected().unwrap()].id.clone();

        app.absorb(&[line("d", "four")]); // agent keeps working
        app.rate(Verdict::Down, Some("this one".into()));

        let saved = app.store.all().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            saved[0].moment, target,
            "rated a moment the user was not pointing at"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn subject_labels_separate_tools_from_prose() {
        assert_eq!(subject_of(&MomentKind::Said { text: "x".into() }), "said");
        assert_eq!(
            subject_of(&MomentKind::Did {
                tool: "Bash".into(),
                input: "ls".into(),
                output: None,
                tool_use_id: None,
                intent: None,
            }),
            "did:Bash"
        );
        assert_eq!(
            subject_of(&MomentKind::Thought {
                text: None,
                bytes: 0
            }),
            "thought"
        );
    }

    #[test]
    fn clock_shortens_a_timestamp_and_survives_a_missing_one() {
        assert_eq!(clock(Some("2026-08-20T12:04:19.412Z")), "12:04:19");
        assert_eq!(clock(None), "--:--:--");
        assert_eq!(clock(Some("garbage")), "--:--:--");
    }
}
