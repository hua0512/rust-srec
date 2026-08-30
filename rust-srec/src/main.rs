//! rust-srec standalone server entrypoint.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_srec::backend::run_server().await
}
