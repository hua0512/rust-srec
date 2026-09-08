fn main() {
    // Include new/imported schemas while avoiding package-wide invalidation by
    // unrelated source, documentation, or local planning files.
    println!("cargo:rerun-if-changed=proto");
    println!("cargo:rerun-if-env-changed=PROTOC");
    println!("cargo:rerun-if-env-changed=PROTOC_INCLUDE");
    println!("cargo:rerun-if-env-changed=PATH");
    for variable in ["PROTOC", "PROTOC_INCLUDE"] {
        if let Some(path) = std::env::var_os(variable) {
            let path = std::path::Path::new(&path);
            // Bare executable names are resolved through PATH, not the package root.
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    prost_build::compile_protos(&["proto/download_progress.proto"], &["proto/"])
        .expect("Failed to compile protos");
    prost_build::compile_protos(&["proto/log_event.proto"], &["proto/"])
        .expect("Failed to compile protos");
}
