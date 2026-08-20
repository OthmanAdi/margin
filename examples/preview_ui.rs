//! Print the rendered UI as plain text, so layout can be checked without a terminal.
//! `cargo run --example preview_ui`

use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn main() -> anyhow::Result<()> {
    let (cols, rows) = (96u16, 18u16);
    let mut terminal = Terminal::new(TestBackend::new(cols, rows))?;
    terminal.draw(margin::ui::draw_demo)?;
    let buf = terminal.backend().buffer().clone();

    for y in 0..rows {
        let mut line = String::new();
        for x in 0..cols {
            line.push_str(buf[(x, y)].symbol());
        }
        println!("{}", line.trim_end());
    }
    Ok(())
}
