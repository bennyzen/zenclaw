fn main() {
    // Bake the board name from `just build <board>` into the binary so /api/status
    // can report it. Falls back to "unknown" when built outside the just pipeline.
    println!("cargo:rerun-if-env-changed=ZENCLAW_BOARD");
    let board = std::env::var("ZENCLAW_BOARD").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=ZENCLAW_BOARD={board}");

    // The esp-idf-sys link-arg propagation is only meaningful when targeting
    // the device. Skip it on host builds (used for `cargo test --features
    // desktop`) so the host toolchain doesn't try to resolve esp-idf.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        embuild::espidf::sysenv::output();
    }
}
