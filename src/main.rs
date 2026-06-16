mod app;
mod event;
mod modules;
mod tui;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok(); // load .env if present; silently ignored if absent
    // Restore terminal on panic so the shell isn't left in raw mode.
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stderr(), crossterm::terminal::LeaveAlternateScreen);
        default_panic(info);
    }));

    let mut tui = tui::Tui::new()?;
    let mut app = app::AppState::new();
    app.run(&mut tui).await?;
    Ok(())
}
