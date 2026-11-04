fn main() {
    // Explicit rerun hints keep unrelated source, documentation, or local
    // planning files from invalidating the package.
    //
    // `rust_i18n::i18n!` reads `locales/` at compile time without registering
    // the files with rustc, and `sqlx::migrate!` only registers migrations that
    // already exist, so new or edited inputs would otherwise leave stale
    // translations or a migrator missing a migration.
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-changed=migrations");
}
