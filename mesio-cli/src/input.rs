use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};
use pipeline_common::CancellationToken;
use std::io::{self, BufRead, IsTerminal};
use std::sync::{Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;
use tracing::info;

/// Whether this process has put the terminal in raw mode. Enabling and
/// restoring hold the lock, so raw mode is never enabled after
/// [`restore_terminal`] has run, whichever thread gets there first.
static RAW_MODE: Mutex<RawMode> = Mutex::new(RawMode::Off);

#[derive(Clone, Copy, PartialEq, Eq)]
enum RawMode {
    Off,
    On,
    Restored,
}

/// Puts the terminal in raw mode unless it has already been restored.
fn enable_raw_mode() -> bool {
    let mut mode = RAW_MODE.lock().unwrap_or_else(PoisonError::into_inner);
    if *mode != RawMode::Off {
        return false;
    }
    if terminal::enable_raw_mode().is_err() {
        info!("Failed to enable raw mode. Input handling will be disabled.");
        return false;
    }
    *mode = RawMode::On;
    true
}

/// Takes the terminal out of raw mode if this process put it there, and
/// keeps it from being enabled again. Callable from any thread, including
/// while the input handler is blocked reading the terminal.
pub fn restore_terminal() {
    let mut mode = RAW_MODE.lock().unwrap_or_else(PoisonError::into_inner);
    if *mode == RawMode::On && terminal::disable_raw_mode().is_err() {
        info!("Failed to disable raw mode.");
    }
    *mode = RawMode::Restored;
}

/// Listens for user input that requests a stop, and cancels `token` on it.
///
/// This function checks if stdin is a terminal or piped input:
/// - If stdin is a terminal, it enables raw mode and listens for key presses.
///   Pressing 'q' or Ctrl+C will trigger the cancellation token.
/// - If stdin is not a terminal (piped input), it reads lines from stdin.
///   A line consisting of 'q' triggers the cancellation token.
///
/// It blocks its thread, possibly beyond its poll timeout (a terminal read
/// waits for an escape sequence to complete), so run it on a thread of its
/// own that nothing waits for, and call [`restore_terminal`] before exiting.
pub fn input_handler(token: CancellationToken) {
    let stdin = io::stdin();

    // Check if stdin is a terminal
    if stdin.is_terminal() {
        // Terminal mode: use keyboard events
        handle_terminal_input(token);
    } else {
        // Stdin mode: read from piped input
        handle_stdin_input(token);
    }
}

/// Handle input from terminal keyboard events
fn handle_terminal_input(token: CancellationToken) {
    if !enable_raw_mode() {
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
                info!("Cancellation requested. Shutting down gracefully...");
                token.cancel();
                break;
            }
        }
    }

    restore_terminal();
}

/// Handle input from stdin (piped input)
fn handle_stdin_input(token: CancellationToken) {
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
