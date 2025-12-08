fn main() {
    prost_build::compile_protos(&["proto/download_progress.proto"], &["proto/"])
        .expect("Failed to compile protos");
}
