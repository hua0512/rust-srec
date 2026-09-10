//! Finite native containment fixture. Every descendant inherits the owned tree.
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let mode = arguments.next().unwrap();
    let ready = PathBuf::from(arguments.next().unwrap());
    if mode == "leaf" {
        std::io::stdout().write_all(b"leaf owns stdout\n").unwrap();
        std::io::stdout().flush().unwrap();
        std::fs::write(ready, std::process::id().to_string()).unwrap();
        // Even a broken fixture owner cannot turn this into a permanent daemon.
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("leaf")
        .arg(&ready)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let _child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(Instant::now() < deadline, "descendant startup timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    if mode == "exit" {
        return;
    }
    if mode == "flood" {
        std::io::stdout().write_all(&[b'x'; 8192]).unwrap();
        std::io::stdout().flush().unwrap();
    }
    std::thread::sleep(Duration::from_secs(30));
}
