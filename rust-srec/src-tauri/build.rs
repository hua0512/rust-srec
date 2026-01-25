fn main() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"),
    );

    // `tauri::generate_context!()` validates `build.frontendDist` exists at compile time.
    // Ensure the directory exists so `cargo test`/`cargo nextest` can compile the workspace
    // without requiring a frontend build.
    let frontend_dist = manifest_dir.join("../frontend/dist-desktop");
    std::fs::create_dir_all(&frontend_dist).unwrap_or_else(|e| {
        panic!("failed to create frontendDist directory {frontend_dist:?}: {e}")
    });

    tauri_build::build()
}
