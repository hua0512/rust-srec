use std::{
    backtrace::Backtrace,
    panic::{PanicHookInfo, take_hook},
    path::Path,
    thread,
};

use chrono::Local;

/// Installs a global panic hook that logs panics via `tracing` and also appends
/// a panic record through the bounded log store in `log_dir` for abort builds.
///
/// This is intentionally redundant:
/// - `tracing` integrates with normal logging + websocket log streaming.
/// - Direct file append helps preserve panic details in `panic = "abort"` builds
///   where buffered/background log writers may not flush before abort.
pub fn install(log_dir: impl AsRef<Path>) {
    let log_dir = log_dir.as_ref().to_path_buf();
    let emergency_store = match crate::logging::store::LogStore::from_env(log_dir) {
        Ok(store) => Some(store),
        Err(error) => {
            eprintln!("Emergency file logging unavailable: {error}");
            None
        }
    };
    let previous_hook = take_hook();

    std::panic::set_hook(Box::new(move |panic_info: &PanicHookInfo<'_>| {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let panic_record = format_panic_record(panic_info);

            tracing::error!(target: "rust_srec::panic", "{panic_record}");

            // Abort builds cannot rely on the background writer flushing. Never
            // wait for its ownership here: the panic may have occurred inside it.
            if cfg!(panic = "abort")
                && let Some(store) = &emergency_store
                && let Err(error) = store.try_append_emergency(&format!("{panic_record}\n"))
            {
                eprintln!("Emergency file logging failed; panic details follow on stderr: {error}");
            }
        }));

        // Preserve the default hook output/backtrace behavior.
        previous_hook(panic_info);
    }));
}

fn format_panic_record(panic_info: &PanicHookInfo<'_>) -> String {
    let payload = panic_payload_to_string(panic_info);
    let location = panic_info
        .location()
        .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
        .unwrap_or_else(|| "<unknown>".to_string());

    let thread_name = thread::current()
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| "<unnamed>".to_string());

    let backtrace = Backtrace::force_capture();
    let ts = Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z");

    format!(
        "{ts} PANIC thread={thread_name} location={location} payload={payload}\nBacktrace:\n{backtrace}"
    )
}

fn panic_payload_to_string(panic_info: &PanicHookInfo<'_>) -> String {
    if let Some(s) = panic_info.payload().downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = panic_info.payload().downcast_ref::<String>() {
        return s.clone();
    }
    // Fallback includes any location/message formatting the std panic type provides.
    panic_info.to_string()
}
