//! Native test executable: version text is not evidence of a plugin loader.
use std::io::Write;

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.iter().any(|arg| arg == "--plugin-dir") {
        let log = std::env::current_exe()
            .unwrap()
            .with_extension("probe-count");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .unwrap()
            .write_all(b"x")
            .unwrap();
    }
    if arguments.iter().any(|arg| arg == "--version") {
        println!("streamlink 8.5.0");
    } else if arguments.iter().any(|arg| arg == "--plugin-dir")
        && std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("reject")
    {
        std::process::exit(2);
    } else if arguments.iter().any(|arg| arg == "--help") {
        println!("opaque CLI help");
    } else if arguments.iter().any(|arg| arg == "-i") {
        let path = arguments.last().unwrap();
        let mut output = std::fs::File::create(path).unwrap();
        eprintln!("[segment @ fixture] Opening '{path}' for writing");
        std::io::copy(&mut std::io::stdin(), &mut output).unwrap();
        output.flush().unwrap();
    } else {
        std::io::stdout()
            .write_all(b"opaque recording remains available\n")
            .unwrap();
    }
}
