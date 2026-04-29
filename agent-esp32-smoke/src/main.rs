use esp_idf_svc::log::EspLogger;
use esp_idf_svc::sys::{self, esp_chip_info, esp_chip_info_t, link_patches};

fn pass(step: u8, name: &str, msg: &str) {
    log::info!("[{}/6] {:.<20}: {}", step, name, msg);
}

fn fail(step: u8, name: &str, msg: &str) -> ! {
    log::error!("[{}/6] {:.<20}: FAIL — {}", step, name, msg);
    log::error!("SMOKE FAIL");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

fn checkpoint_1_chip_info() {
    let mut info = esp_chip_info_t::default();
    unsafe { esp_chip_info(&mut info as *mut _) };

    let model = match info.model {
        m if m == sys::esp_chip_model_t_CHIP_ESP32 => "ESP32",
        m if m == sys::esp_chip_model_t_CHIP_ESP32S2 => "ESP32-S2",
        m if m == sys::esp_chip_model_t_CHIP_ESP32S3 => "ESP32-S3",
        m if m == sys::esp_chip_model_t_CHIP_ESP32C3 => "ESP32-C3",
        m if m == sys::esp_chip_model_t_CHIP_ESP32C6 => "ESP32-C6",
        m if m == sys::esp_chip_model_t_CHIP_ESP32H2 => "ESP32-H2",
        m if m == sys::esp_chip_model_t_CHIP_ESP32P4 => "ESP32-P4",
        _ => "UNKNOWN",
    };
    // esp_chip_info_t.revision is in MXX format (M = wafer major, XX = wafer minor)
    let rev_major = info.revision / 100;
    let rev_minor = info.revision % 100;

    if model == "UNKNOWN" {
        fail(1, "chip_info", &format!("unknown chip model {}", info.model));
    }
    pass(
        1,
        "chip_info",
        &format!(
            "{} rev v{}.{}, {} cores",
            model, rev_major, rev_minor, info.cores
        ),
    );
}

fn main() -> anyhow::Result<()> {
    link_patches();
    EspLogger::initialize_default();
    log::info!("zenclaw P4 smoke test booting");

    checkpoint_1_chip_info();

    log::info!("(checkpoints 2-6 not yet implemented)");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
