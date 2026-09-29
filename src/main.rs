mod app;
mod config;
mod library;
mod metadata;
mod model;
mod player;
mod soundcloud;
mod spotify;
mod tags;
mod tidal;
mod sync;
mod ui;

use std::{error::Error, io};

use app::App;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

fn main() -> Result<(), Box<dyn Error>> {
    // Audio backends (ALSA especially) print straight to stderr when a device is missing or
    // busy, which scribbles all over the TUI — send it to a log file while the UI is up.
    let saved_stderr = stderr_to_log();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        restore_stderr(saved_stderr);
        default_hook(info);
    }));
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run(&mut terminal);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    restore_stderr(saved_stderr);
    result
}

/// Points fd 2 at `crate-rat.log` in the config dir; returns a dup of the original stderr.
#[cfg(unix)]
fn stderr_to_log() -> Option<i32> {
    use std::os::fd::AsRawFd;
    let dir = config::config_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let log = std::fs::File::create(dir.join("crate-rat.log")).ok()?;
    // SAFETY: plain fd juggling on descriptors we own; `log` stays open until dup2 copied it.
    unsafe {
        let saved = libc::dup(2);
        if saved < 0 {
            return None;
        }
        if libc::dup2(log.as_raw_fd(), 2) < 0 {
            libc::close(saved);
            return None;
        }
        Some(saved)
    }
}

#[cfg(not(unix))]
fn stderr_to_log() -> Option<i32> {
    None
}

fn restore_stderr(saved: Option<i32>) {
    #[cfg(unix)]
    if let Some(saved) = saved {
        // SAFETY: `saved` came from dup() in stderr_to_log. Restoring twice (panic hook + normal
        // exit) is harmless: it just points fd 2 at the same place again.
        unsafe {
            libc::dup2(saved, 2);
        }
    }
    #[cfg(not(unix))]
    let _ = saved;
}

fn run<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<(), Box<dyn Error>> {
    let mut app = App::load();
    while !app.should_quit {
        app.poll_spotify_login();
        app.poll_spotify_import();
        app.poll_tidal_search();
        app.poll_soundcloud_download();
        app.poll_spotify_tidal_download();
        app.poll_tidal_playlist_download();
        app.poll_tidal_crate_refresh();
        app.poll_tags();
        app.poll_playback();
        terminal.draw(|frame| ui::draw(frame, &app))?;
        if event::poll(std::time::Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key.code);
                }
            }
        }
    }
    app.shutdown();
    Ok(())
}