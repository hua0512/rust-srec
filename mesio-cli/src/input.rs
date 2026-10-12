use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal,
};
use pipeline_common::CancellationToken;
use std::io::{self, BufRead, IsTerminal};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use tracing::info;

use crate::cut::{CutSource, CutTarget};

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
    let enabled = {
        let mut mode = RAW_MODE.lock().unwrap_or_else(PoisonError::into_inner);
        if *mode != RawMode::Off {
            return false;
        }
        let enabled = terminal::enable_raw_mode();
        if enabled.is_ok() {
            *mode = RawMode::On;
        }
        enabled
    };
    if let Err(error) = enabled {
        info!(%error, "Failed to enable raw mode. Input handling will be disabled.");
        return false;
    }
    true
}

/// Takes the terminal out of raw mode if this process put it there, and
/// keeps it from being enabled again. Callable from any thread, including
/// while the input handler is blocked reading the terminal. On failure the
/// terminal stays marked raw, so a later call retries.
///
/// It does not log, so a forced exit can call it with stderr stalled.
pub fn restore_terminal() -> io::Result<()> {
    let mut mode = RAW_MODE.lock().unwrap_or_else(PoisonError::into_inner);
    if *mode == RawMode::On {
        terminal::disable_raw_mode()?;
    }
    *mode = RawMode::Restored;
    Ok(())
}

/// What a key press or a line of stdin asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputAction {
    Stop,
    Cut,
}

/// Terminals may report releases and repeats too; only a press acts.
fn key_action(key: &KeyEvent) -> Option<InputAction> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), KeyModifiers::NONE) => Some(InputAction::Stop),
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(InputAction::Stop),
        (KeyCode::Char('s'), KeyModifiers::NONE) => Some(InputAction::Cut),
        _ => None,
    }
}

fn line_action(line: &str) -> Option<InputAction> {
    match line.trim() {
        "q" => Some(InputAction::Stop),
        "s" => Some(InputAction::Cut),
        _ => None,
    }
}

/// Listens for user input that requests a stop or a cut: a stop cancels
/// `token`, a cut goes to `cut`.
///
/// This function checks if stdin is a terminal or piped input:
/// - If stdin is a terminal, it enables raw mode and listens for key presses:
///   'q' or Ctrl+C stops, 's' cuts.
/// - If stdin is not a terminal (piped input), it reads lines from stdin: a
///   line consisting of 'q' stops, one consisting of 's' cuts.
///
/// It blocks its thread, possibly beyond its poll timeout (a terminal read
/// waits for an escape sequence to complete), so run it on a thread of its
/// own that nothing waits for, and call [`restore_terminal`] before exiting.
pub fn input_handler(token: CancellationToken, cut: CutTarget) {
    let stdin = io::stdin();

    // Check if stdin is a terminal
    if stdin.is_terminal() {
        // Terminal mode: use keyboard events
        handle_terminal_input(token, cut);
    } else {
        // Stdin mode: read from piped input
        handle_stdin_input(token, cut);
    }
}

/// Handle input from terminal keyboard events
fn handle_terminal_input(token: CancellationToken, cut: CutTarget) {
    if !enable_raw_mode() {
        return;
    }

    let mut requested = false;
    loop {
        // Check for cancellation signal first.
        if token.is_cancelled() {
            break;
        }

        // Poll for keyboard events with a timeout.
        if let Ok(true) = event::poll(Duration::from_millis(100))
            && let Ok(Event::Key(key_event)) = event::read()
        {
            match key_action(&key_event) {
                Some(InputAction::Stop) => {
                    token.cancel();
                    requested = true;
                    break;
                }
                // Neither blocks nor logs, so `q` keeps working while the
                // acknowledgement is written.
                Some(InputAction::Cut) => {
                    cut.trigger(CutSource::Key);
                }
                None => {}
            }
        }
    }

    // Logging comes last: if it blocks on a stalled pipe, the stop is
    // already under way, and with the terminal no longer raw a second Ctrl+C
    // is a signal that forces the exit.
    let restored = restore_terminal();
    if requested {
        info!("Cancellation requested. Shutting down gracefully...");
    }
    if let Err(error) = restored {
        info!(%error, "Failed to disable raw mode.");
    }
}

/// Handle input from stdin (piped input). Each line acts as it is read, so a
/// cut is never held back for a later recording.
fn handle_stdin_input(token: CancellationToken, cut: CutTarget) {
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        match line_action(&line) {
            Some(InputAction::Stop) => {
                token.cancel();
                info!("Cancellation requested. Shutting down gracefully...");
                break;
            }
            Some(InputAction::Cut) => {
                cut.trigger(CutSource::Stdin);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEventState;

    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn keys_stop_or_cut_only_when_pressed() {
        let press = |code, modifiers| key_action(&key(code, modifiers, KeyEventKind::Press));
        assert_eq!(
            press(KeyCode::Char('q'), KeyModifiers::NONE),
            Some(InputAction::Stop)
        );
        assert_eq!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(InputAction::Stop)
        );
        assert_eq!(
            press(KeyCode::Char('s'), KeyModifiers::NONE),
            Some(InputAction::Cut)
        );
        assert_eq!(press(KeyCode::Char('s'), KeyModifiers::CONTROL), None);
        assert_eq!(press(KeyCode::Char('S'), KeyModifiers::SHIFT), None);
        assert_eq!(press(KeyCode::Char('c'), KeyModifiers::NONE), None);
        for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
            assert_eq!(
                key_action(&key(KeyCode::Char('s'), KeyModifiers::NONE, kind)),
                None
            );
            assert_eq!(
                key_action(&key(KeyCode::Char('q'), KeyModifiers::NONE, kind)),
                None
            );
        }
    }

    #[test]
    fn lines_stop_or_cut_when_they_consist_of_the_command() {
        assert_eq!(line_action("q"), Some(InputAction::Stop));
        assert_eq!(line_action(" s \r"), Some(InputAction::Cut));
        assert_eq!(line_action("split"), None);
        assert_eq!(line_action(""), None);
    }
}
