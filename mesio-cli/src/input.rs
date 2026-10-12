use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};
use pipeline_common::CancellationToken;
use std::io::{self, BufRead, IsTerminal};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tracing::info;

/// Listens for user input that requests a stop, and cancels `token` on it.
///
/// This function checks if stdin is a terminal or piped input:
/// - If stdin is a terminal, it enables raw mode and listens for key presses.
///   Pressing 'q' or Ctrl+C will trigger the cancellation token.
/// - If stdin is not a terminal (piped input), it reads lines from stdin.
///   A line consisting of 'q' triggers the cancellation token.
///
/// It polls, so it blocks its thread: run it with `spawn_blocking`. It returns
/// after restoring the terminal mode once `done` is cancelled, which, with
/// `done` a child of `token`, includes any requested stop.
pub fn input_handler(token: CancellationToken, done: CancellationToken) {
    let stdin = io::stdin();

    // Check if stdin is a terminal
    if stdin.is_terminal() {
        // Terminal mode: use keyboard events
        handle_terminal_input(token, done);
    } else {
        // Stdin mode: read from piped input
        handle_stdin_input(token, done);
    }
}

/// Handle input from terminal keyboard events
fn handle_terminal_input(token: CancellationToken, done: CancellationToken) {
    if terminal::enable_raw_mode().is_err() {
        info!("Failed to enable raw mode. Input handling will be disabled.");
        return;
    }

    loop {
        // Check for cancellation signal first.
        if done.is_cancelled() {
            break;
        }

        // Poll for keyboard events with a timeout.
        if let Ok(true) = event::poll(Duration::from_millis(100))
            && let Ok(Event::Key(key_event)) = event::read()
        {
            let should_cancel = match key_event {
                // Handle 'q' key press
                KeyEvent {
                    code: KeyCode::Char('q'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => true,
                // Handle Ctrl+C
                KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => true,
                _ => false,
            };

            if should_cancel {
                info!("Cancellation requested. Shutting down gracefully...");
                token.cancel();
                break;
            }
        }
    }

    if terminal::disable_raw_mode().is_err() {
        info!("Failed to disable raw mode.");
    }
}

/// Handle input from stdin (piped input)
fn handle_stdin_input(token: CancellationToken, done: CancellationToken) {
    let (tx, rx) = mpsc::channel();

    // Spawn a blocking thread to read from stdin
    thread::spawn(move || {
        let stdin = io::stdin();
        let reader = stdin.lock();

        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    loop {
        // Check for cancellation signal first.
        if done.is_cancelled() {
            break;
        }

        // Try to receive a line from stdin with timeout
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if line.trim() == "q" {
                    info!("Cancellation requested. Shutting down gracefully...");
                    token.cancel();
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Continue waiting
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Stdin closed
                break;
            }
        }
    }
}
