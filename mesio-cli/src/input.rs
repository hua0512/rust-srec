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

/// Asynchronously listens for user input to trigger cancellation.
///
/// This function checks if stdin is a terminal or piped input:
/// - If stdin is a terminal, it enables raw mode and listens for key presses.
///   Pressing 'q' or Ctrl+C will trigger the cancellation token.
/// - If stdin is not a terminal (piped input), it reads lines from stdin.
///   If a line contains 'q', it triggers the cancellation token.
///
/// The function will clean up by disabling raw mode when the token is cancelled.
pub async fn input_handler(token: CancellationToken) {
    let stdin = io::stdin();

    // Check if stdin is a terminal
    if stdin.is_terminal() {
        // Terminal mode: use keyboard events
        handle_terminal_input(token).await;
    } else {
        // Stdin mode: read from piped input
        handle_stdin_input(token).await;
    }
}

/// Handle input from terminal keyboard events
async fn handle_terminal_input(token: CancellationToken) {
    if terminal::enable_raw_mode().is_err() {
        info!("Failed to enable raw mode. Input handling will be disabled.");
        return;
    }

    loop {
        // Check for cancellation signal first.
        if token.is_cancelled() {
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
                println!("Cancellation requested. Shutting down gracefully...");
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
async fn handle_stdin_input(token: CancellationToken) {
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
        if token.is_cancelled() {
            break;
        }

        // Try to receive a line from stdin with timeout
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if line.trim() == "q" {
                    println!("Cancellation requested. Shutting down gracefully...");
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
