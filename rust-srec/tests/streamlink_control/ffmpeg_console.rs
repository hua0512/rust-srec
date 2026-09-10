//! Native-only shim: observe the child's own console, then forward to real FFmpeg.
use std::io::Write;
use std::process::{Command, Stdio};

#[cfg(windows)]
fn console_window() -> usize {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
    }
    unsafe { GetConsoleWindow() as usize }
}

#[cfg(not(windows))]
fn console_window() -> usize {
    0
}

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let role = if arguments.iter().any(|arg| arg == "-version") {
        "validation"
    } else {
        "muxer"
    };
    let log = std::env::var_os("SREC_NATIVE_CONSOLE_LOG").unwrap();
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .unwrap();
    let record = format!("{role}:{}\n", console_window());
    log.write_all(record.as_bytes()).unwrap();
    drop(log);
    let mut command = Command::new(std::env::var_os("SREC_NATIVE_REAL_FFMPEG").unwrap());
    command
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    std::process::exit(command.status().unwrap().code().unwrap_or(1));
}
