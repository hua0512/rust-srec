use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};
use pipeline_common::CancellationToken;
use std::time::Duration;
use tracing::info;

/// Asynchronously listens for user input to trigger cancellation.
///
/// This function enables raw mode for the terminal and listens for key presses.
/// If the 'q' key is pressed, it triggers the cancellation token.
/// The function will clean up by disabling raw mode when the token is cancelled.
pub async fn input_handler(token: CancellationToken) {
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
            && let Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                modifiers: KeyModifiers::NONE,
                ..
            })) = event::read()
        {
            println!("Cancellation requested. Shutting down gracefully...");
            token.cancel();
            break;
        }
    }

    if terminal::disable_raw_mode().is_err() {
        info!("Failed to disable raw mode.");
    }
}
