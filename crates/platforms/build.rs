fn main() {
    // Common metadata occurs in chat messages, but its optional room embeds a
    // large response tree. Keeping it inline can make even small chat payloads
    // exhaust Windows thread stacks in unoptimized Prost decode paths.
    prost_build::Config::new()
        .boxed(".douyin.Webcast.Im.Common.room")
        .compile_protos(&["proto/douyin.proto", "proto/tiktok.proto"], &["proto/"])
        .expect("Failed to compile protos");
}
