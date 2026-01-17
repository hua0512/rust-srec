use chrono::Local;
use std::{
    backtrace::Backtrace,
    fs::OpenOptions,
    io::Write,
    panic::{PanicHookInfo, take_hook},
    path::{Path, PathBuf},
    thread,
};

/// Installs a global panic hook that logs panics via `tracing` and also appends
/// a panic record to the current daily log file in `log_dir`.
///
/// This is intentionally redundant:
/// - `tracing` integrates with normal logging + websocket log streaming.
/// - Direct file append helps preserve panic details in `panic = "abort"` builds
///   where buffered/background log writers may not flush before abort.
pub fn install(log_dir: impl AsRef<Path>) {
    let log_dir = log_dir.as_ref().to_path_buf();
    let previous_hook = take_hook();

    std::panic::set_hook(Box::new(move |panic_info: &PanicHookInfo<'_>| {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let panic_record = format_panic_record(panic_info);

            tracing::error!(target: "rust_srec::panic", "{panic_record}");

            // Best-effort: in `panic = "abort"` builds, append to the current daily log file
            // (matches `tracing_appender::rolling::daily` naming) because background log writers
            // may not flush before the process aborts.
            if cfg!(panic = "abort") {
                let _ = append_panic_record(&log_dir, &panic_record);
            }
        }));

        // Preserve the default hook output/backtrace behavior.
        previous_hook(panic_info);
    }));
}

fn append_panic_record(log_dir: &Path, record: &str) -> std::io::Result<()> {
    let filename = format!("rust-srec.log.{}", Local::now().format("%Y-%m-%d"));
    let path = PathBuf::from(log_dir).join(filename);

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{record}")?;
    file.flush()
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
