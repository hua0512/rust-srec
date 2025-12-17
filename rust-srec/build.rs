fn main() {
    prost_build::compile_protos(&["proto/download_progress.proto"], &["proto/"])
        .expect("Failed to compile protos");

    prost_build::compile_protos(&["proto/douyin.proto"], &["proto/"])
        .expect("Failed to compile protos");

    prost_build::compile_protos(&["proto/log_event.proto"], &["proto/"])
        .expect("Failed to compile protos");
}
