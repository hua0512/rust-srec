fn main() {
    prost_build::compile_protos(&["proto/douyin.proto", "proto/tiktok.proto"], &["proto/"])
        .expect("Failed to compile protos");
}
