// Minimal ESP32 build test — does the toolchain + esp-idf-svc compile?
use esp_idf_svc::hal::prelude::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::sys::link_patches;

fn main() {
    link_patches();
    EspLogger::initialize_default();
    log::info!("ZenClaw ESP32 boot");

    let _peripherals = Peripherals::take().unwrap();
    log::info!("Peripherals initialized, halting");
}
