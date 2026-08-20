//! Rendering the real UI to an SVG.
//!
//! The README needs a picture, and every terminal tool worth copying has one. A screenshot
//! would be a lie within a release: it shows whatever the UI looked like on the day someone
//! remembered to take it, on their font, at their window size.
//!
//! This renders the actual widget tree through ratatui's `TestBackend` and writes the
//! resulting cell buffer as SVG. It regenerates from `cargo run -- snapshot`, so the image
//! in the README cannot drift from the code that produced it. It is also text, so a diff
//! shows what changed on screen.

use crate::moment::Harness;
use anyhow::Result;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use std::fmt::Write as _;
use std::path::Path;

const CELL_W: f32 = 8.4;
const CELL_H: f32 = 18.0;
const PAD: f32 = 20.0;
const CHROME: f32 = 34.0;

/// A terminal palette chosen to look like a modern dark theme rather than raw ANSI.
fn hex(color: Color) -> &'static str {
    match color {
        Color::Reset => "#c9d1d9",
        Color::Black => "#0d1117",
        Color::Red => "#ff7b72",
        Color::Green => "#7ee787",
        Color::Yellow => "#e3b341",
        Color::Blue => "#79c0ff",
        Color::Magenta => "#d2a8ff",
        Color::Cyan => "#56d4dd",
        Color::Gray => "#8b949e",
        Color::DarkGray => "#6e7681",
        Color::LightRed => "#ffa198",
        Color::LightGreen => "#56d364",
        Color::LightYellow => "#e3b341",
        Color::LightBlue => "#a5d6ff",
        Color::LightMagenta => "#e2c5ff",
        Color::LightCyan => "#b3f0ff",
        Color::White => "#f0f6fc",
        Color::Indexed(236) => "#161b22",
        Color::Indexed(_) => "#c9d1d9",
        Color::Rgb(_, _, _) => "#c9d1d9",
    }
}

const BG: &str = "#0d1117";
const FG: &str = "#c9d1d9";

/// Render one frame of the real UI and write it as SVG.
pub fn write_svg(
    path: &Path,
    width: u16,
    height: u16,
    title: &str,
    render: impl FnOnce(&mut ratatui::Frame),
) -> Result<()> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| render(f))?;
    let buffer = terminal.backend().buffer().clone();

    let svg = buffer_to_svg(&buffer, width, height, title);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, svg)?;
    Ok(())
}

fn buffer_to_svg(buf: &Buffer, cols: u16, rows: u16, title: &str) -> String {
    let w = cols as f32 * CELL_W + PAD * 2.0;
    let h = rows as f32 * CELL_H + PAD * 2.0 + CHROME;

    let mut s = String::new();
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}" font-family="ui-monospace,SFMono-Regular,'JetBrains Mono','Cascadia Code',Menlo,Consolas,monospace" font-size="13">
<rect width="{w:.0}" height="{h:.0}" rx="10" fill="{BG}"/>
<rect width="{w:.0}" height="{CHROME}" rx="10" fill="#161b22"/>
<rect y="{half:.0}" width="{w:.0}" height="{half:.0}" fill="#161b22"/>
<circle cx="20" cy="17" r="6" fill="#ff5f57"/><circle cx="40" cy="17" r="6" fill="#febc2e"/><circle cx="60" cy="17" r="6" fill="#28c840"/>
<text x="{tx:.0}" y="22" fill="#8b949e" font-size="12">{title}</text>
"##,
        half = CHROME / 2.0,
        tx = w / 2.0 - (title.len() as f32 * 3.2),
    );

    // Backgrounds first, merged horizontally so a highlighted row is one rect not eighty.
    for y in 0..rows {
        let mut x = 0u16;
        while x < cols {
            let bg = buf[(x, y)].bg;
            if bg == Color::Reset {
                x += 1;
                continue;
            }
            let start = x;
            while x < cols && buf[(x, y)].bg == bg {
                x += 1;
            }
            let _ = write!(
                s,
                r##"<rect x="{px:.1}" y="{py:.1}" width="{pw:.1}" height="{CELL_H:.1}" fill="{fill}"/>
"##,
                px = PAD + start as f32 * CELL_W,
                py = PAD + CHROME + y as f32 * CELL_H,
                pw = (x - start) as f32 * CELL_W,
                fill = hex(bg),
            );
        }
    }

    // Then text, merged into runs sharing a style.
    for y in 0..rows {
        let mut x = 0u16;
        while x < cols {
            let cell = &buf[(x, y)];
            let fg = cell.fg;
            let modifier = cell.modifier;
            let start = x;
            let mut run = String::new();
            while x < cols && buf[(x, y)].fg == fg && buf[(x, y)].modifier == modifier {
                run.push_str(buf[(x, y)].symbol());
                x += 1;
            }
            if run.trim().is_empty() {
                continue;
            }
            let weight = if modifier.contains(Modifier::BOLD) { " font-weight=\"600\"" } else { "" };
            let italic = if modifier.contains(Modifier::ITALIC) { " font-style=\"italic\"" } else { "" };
            let fill = if fg == Color::Reset { FG } else { hex(fg) };
            let _ = write!(
                s,
                r##"<text x="{px:.1}" y="{py:.1}" fill="{fill}"{weight}{italic} xml:space="preserve">{}</text>
"##,
                escape(&run),
                px = PAD + start as f32 * CELL_W,
                py = PAD + CHROME + y as f32 * CELL_H + 13.0,
            );
        }
    }

    s.push_str("</svg>\n");
    s
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The scene the README shows: a real session, mid-run, with one approval and one rejection
/// already recorded and a thought that Claude Code never persisted.
#[allow(dead_code)]
pub fn demo_harness() -> Harness {
    Harness::ClaudeCode
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::Paragraph;

    #[test]
    fn produces_valid_looking_svg_with_the_rendered_text_in_it() {
        let dir = std::env::temp_dir().join(format!("margin-svg-{}", std::process::id()));
        let path = dir.join("out.svg");
        write_svg(&path, 40, 3, "margin", |f| {
            f.render_widget(Paragraph::new("hello margin"), f.area());
        })
        .unwrap();

        let svg = std::fs::read_to_string(&path).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>\n"));
        assert!(svg.contains("hello margin"), "rendered text should appear in the SVG");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn escapes_markup_so_a_tool_call_cannot_break_the_document() {
        assert_eq!(escape("<a> & </a>"), "&lt;a&gt; &amp; &lt;/a&gt;");
    }
}

/// The injected block, rendered for the README.
///
/// Uses the same `inject::render` the hook uses, so the picture is the payload, not a
/// prettified retelling of it.
pub fn signal_text() -> String {
    use crate::moment::MomentId;
    use crate::ratings::{Rating, Verdict};

    let ratings = vec![
        Rating {
            moment: MomentId::new(Harness::ClaudeCode, "demo", "a", 0),
            verdict: Verdict::Up,
            note: None,
            at: "2026-08-20T12:04:11Z".into(),
            preview: Some("I'll check the transcript format first before writing the parser.".into()),
            subject: Some("said".into()),
        },
        Rating {
            moment: MomentId::new(Harness::ClaudeCode, "demo", "b", 0),
            verdict: Verdict::Down,
            note: Some("wrong file, use the debug log".into()),
            at: "2026-08-20T12:04:19Z".into(),
            preview: Some("Bash(grep -c thinking ~/.claude/debug/*.txt)".into()),
            subject: Some("did:Bash".into()),
        },
    ];
    crate::inject::render(&ratings, crate::inject::Trigger::PostToolUse).unwrap_or_default()
}

/// Height needed for the signal image, so the picture never clips its own content.
pub fn draw_signal_rows() -> u16 {
    let wrapped = wrap_to(&signal_text(), 92);
    (wrapped.len() as u16) + 2
}

pub fn draw_signal(f: &mut ratatui::Frame) {
    use ratatui::style::Style;
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;

    let lines: Vec<Line> = wrap_to(&signal_text(), 92)
        .into_iter()
        .map(|l| {
            let style = if l.starts_with("<margin-signal") || l.starts_with("</margin-signal") {
                Style::new().fg(Color::DarkGray)
            } else if l.trim_start().starts_with("1.") || l.trim_start().starts_with("2.") {
                if l.contains("APPROVED") {
                    Style::new().fg(Color::Green).bold()
                } else {
                    Style::new().fg(Color::Red).bold()
                }
            } else if l.trim_start().starts_with("takeaway:") {
                Style::new().fg(Color::Cyan)
            } else if l.trim_start().starts_with("at ") {
                Style::new().fg(Color::White)
            } else {
                Style::new().fg(Color::Gray)
            };
            Line::from(Span::styled(format!(" {l}"), style))
        })
        .collect();

    f.render_widget(Paragraph::new(lines), f.area());
}

/// Hard-wrap on word boundaries. The SVG writer works in cells, so wrapping has to happen
/// before rendering rather than being left to a widget.
fn wrap_to(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        if para.chars().count() <= width {
            out.push(para.to_string());
            continue;
        }
        let indent: String = para.chars().take_while(|c| *c == ' ').collect();
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if candidate.chars().count() + indent.len() > width && !line.is_empty() {
                out.push(format!("{indent}{line}"));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() {
            out.push(format!("{indent}{line}"));
        }
    }
    out
}
