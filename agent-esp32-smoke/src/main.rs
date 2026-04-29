use esp_idf_svc::sys::link_patches;
use esp_idf_svc::log::EspLogger;

fn main() -> anyhow::Result<()> {
    link_patches();
    EspLogger::initialize_default();
    log::info!("zenclaw P4 smoke test booting (Phase A.1 stub)");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
