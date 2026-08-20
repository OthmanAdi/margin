//! Render all three view levels side by side. `cargo run --example preview_views`

use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn main() -> anyhow::Result<()> {
    let (cols, rows) = (94u16, 14u16);
    for view in ["mirror", "medium", "verbose"] {
        let mut terminal = Terminal::new(TestBackend::new(cols, rows))?;
        terminal.draw(|f| margin::ui::draw_demo_view(f, view))?;
        let buf = terminal.backend().buffer().clone();
        println!("=== {view} ===");
        for y in 0..rows {
            let mut line = String::new();
            for x in 0..cols {
                line.push_str(buf[(x, y)].symbol());
            }
            println!("{}", line.trim_end());
        }
        println!();
    }
    Ok(())
}
