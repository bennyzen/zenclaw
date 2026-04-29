fn main() {
    // The esp-idf-sys link-arg propagation is only meaningful when targeting
    // the device. Skip it on host builds (used for `cargo test --features
    // desktop`) so the host toolchain doesn't try to resolve esp-idf.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        embuild::espidf::sysenv::output();
    }
}
