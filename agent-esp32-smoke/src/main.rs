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

fn checkpoint_2_psram() {
    use esp_idf_svc::sys::{
        esp_psram_get_size, heap_caps_get_free_size, heap_caps_malloc, heap_caps_free,
        MALLOC_CAP_SPIRAM,
    };

    let psram_bytes = unsafe { esp_psram_get_size() } as usize;
    if psram_bytes == 0 {
        fail(2, "psram", "no PSRAM detected — check sdkconfig CONFIG_SPIRAM");
    }
    let psram_mb = psram_bytes / (1024 * 1024);
    let free_psram = unsafe { heap_caps_get_free_size(MALLOC_CAP_SPIRAM) } as usize;

    // Pattern test: allocate 4 MB in PSRAM, write/read a known pattern.
    const TEST_BYTES: usize = 4 * 1024 * 1024;
    let buf = unsafe { heap_caps_malloc(TEST_BYTES, MALLOC_CAP_SPIRAM) as *mut u8 };
    if buf.is_null() {
        fail(2, "psram", "could not allocate 4 MB in PSRAM");
    }
    unsafe {
        for i in 0..TEST_BYTES {
            *buf.add(i) = ((i * 31 + 7) & 0xFF) as u8;
        }
        for i in 0..TEST_BYTES {
            let expected = ((i * 31 + 7) & 0xFF) as u8;
            if *buf.add(i) != expected {
                fail(2, "psram", &format!("pattern mismatch at byte {}", i));
            }
        }
        heap_caps_free(buf as *mut _);
    }

    pass(
        2,
        "psram",
        &format!("{} MiB detected, {} KiB free, 4 MiB pattern test OK", psram_mb, free_psram / 1024),
    );
}

/// Ethernet + DHCP bring-up for ESP32-P4 + IP101 PHY.
///
/// # Why raw FFI?
///
/// esp-idf-svc 0.52 gates the entire `eth` module — including `EthDriver`,
/// `EspEth`, `BlockingEth`, and `EthEvent` — on chip-specific cfgs:
///
/// ```text
/// #[cfg(any(
///     all(esp32, esp_idf_eth_use_esp32_emac),   // only original ESP32
///     any(esp_idf_eth_spi_ethernet_*),           // SPI variants
///     esp_idf_eth_use_openeth
/// ))]
/// pub mod eth;
/// ```
///
/// ESP32-P4 builds target `riscv32imafc-esp-espidf`, so the `esp32` cfg is
/// false and the whole module is absent.  We construct the driver using raw
/// `esp_idf_sys` FFI instead, and use the unconditional `EspNetif` from svc for
/// netif creation and IP polling.
///
/// # Pin map (Guition JC-ESP32P4-M3-DEV, IP101 PHY addr=1)
///   RMII data: TX_EN=49, TXD0=34, TXD1=35, CRS_DV=28, RXD0=29, RXD1=30
///   Control  : MDC=31, MDIO=52
///   REF_CLK  : GPIO50 (50 MHz external input from PHY's oscillator; EMAC_CLK_EXT_IN)
///   PHY_PWR  : GPIO51 (wired as hw-reset; IDF driver asserts low then releases)
///
/// Note: TX_EN=49 is the IDF default for the Guition board.  The plan's stated
/// TX_EN=33 is incorrect for this board — the physical link never comes up
/// with GPIO33.  GPIO49 matches the IDF `ETH_ESP32_EMAC_DEFAULT_CONFIG()` for
/// ESP32-P4 and yields a working 100 Mbps link.
fn checkpoint_3_4_ethernet_dhcp() -> (esp_idf_svc::netif::EspNetif, sys::esp_eth_handle_t) {
    use esp_idf_svc::sys::*;
    use esp_idf_svc::netif::{EspNetif, NetifStack};
    use esp_idf_svc::handle::RawHandle;
    use std::time::{Duration, Instant};

    // ── 1. Build EMAC config for ESP32-P4 ─────────────────────────────────
    // ESP32-P4 has SOC_EMAC_USE_MULTI_IO_MUX=y, so data-plane RMII pins are
    // freely assignable via the emac_dataif_gpio union field.
    let esp32_mac_config = eth_esp32_emac_config_t {
        __bindgen_anon_1: eth_esp32_emac_config_t__bindgen_ty_1 {
            smi_gpio: emac_esp_smi_gpio_config_t {
                mdc_num:  31, // MDC
                mdio_num: 52, // MDIO
            },
        },
        interface: eth_data_interface_t_EMAC_DATA_INTERFACE_RMII,
        clock_config: eth_mac_clock_config_t {
            rmii: eth_mac_clock_config_t__bindgen_ty_2 {
                clock_mode: emac_rmii_clock_mode_t_EMAC_CLK_EXT_IN,
                clock_gpio: 50, // GPIO50 = REF_CLK input from PHY oscillator
            },
        },
        // clock_config_out_in is only relevant when internally generated clock
        // is looped back externally; set to disabled when using external input.
        clock_config_out_in: eth_mac_clock_config_t {
            rmii: eth_mac_clock_config_t__bindgen_ty_2 {
                clock_mode: emac_rmii_clock_mode_t_EMAC_CLK_EXT_IN,
                clock_gpio: -1,
            },
        },
        dma_burst_len: eth_mac_dma_burst_len_t_ETH_DMA_BURST_LEN_32,
        intr_priority: 0,
        emac_dataif_gpio: eth_mac_dataif_gpio_config_t {
            rmii: eth_mac_rmii_gpio_config_t {
                tx_en_num:  49, // TX_EN (IDF default for P4; plan said 33 which is wrong)
                txd0_num:   34, // TXD0
                txd1_num:   35, // TXD1
                crs_dv_num: 28, // CRS_DV
                rxd0_num:   29, // RXD0
                rxd1_num:   30, // RXD1
            },
        },
    };

    let mac_config = eth_mac_config_t {
        sw_reset_timeout_ms: 100,
        rx_task_stack_size: 4096,
        rx_task_prio: 15,
        flags: 0,
    };

    let mac = unsafe { esp_eth_mac_new_esp32(&esp32_mac_config, &mac_config) };
    if mac.is_null() {
        fail(3, "ethernet_link", "esp_eth_mac_new_esp32 returned NULL");
    }

    // ── 2. Build IP101 PHY (addr=1, hw-reset via GPIO51) ──────────────────
    let phy_config = eth_phy_config_t {
        phy_addr: 1,
        reset_timeout_ms: 100,
        autonego_timeout_ms: 4000,
        reset_gpio_num: 51,          // PHY_PWR/RST
        hw_reset_assert_time_us: 0,
        post_hw_reset_delay_ms: 0,
    };

    let phy = unsafe { esp_eth_phy_new_ip101(&phy_config) };
    if phy.is_null() {
        fail(3, "ethernet_link", "esp_eth_phy_new_ip101 returned NULL");
    }

    // ── 3. Install Ethernet driver ─────────────────────────────────────────
    let eth_cfg = esp_eth_config_t {
        mac,
        phy,
        check_link_period_ms: 2000,
        ..Default::default()
    };

    let mut eth_handle: esp_eth_handle_t = std::ptr::null_mut();
    let err = unsafe { esp_eth_driver_install(&eth_cfg, &mut eth_handle) };
    if err != ESP_OK {
        fail(3, "ethernet_link", &format!("esp_eth_driver_install failed: {:#x}", err));
    }

    // ── 4. Create default ETH netif (DHCP client) and attach glue ─────────
    // EspNetif::new calls esp_netif_init() (once) and creates the default eth
    // netif with DHCP client enabled — matches ESP_NETIF_DEFAULT_ETH().
    let netif = match EspNetif::new(NetifStack::Eth) {
        Ok(n) => n,
        Err(e) => fail(3, "ethernet_link", &format!("EspNetif::new failed: {:?}", e)),
    };

    let glue = unsafe { esp_eth_new_netif_glue(eth_handle) };
    if glue.is_null() {
        fail(3, "ethernet_link", "esp_eth_new_netif_glue returned NULL");
    }

    let err = unsafe { esp_netif_attach(netif.handle(), glue as *mut _) };
    if err != ESP_OK {
        fail(3, "ethernet_link", &format!("esp_netif_attach failed: {:#x}", err));
    }

    // ── 5. Start the driver ────────────────────────────────────────────────
    let err = unsafe { esp_eth_start(eth_handle) };
    if err != ESP_OK {
        fail(3, "ethernet_link", &format!("esp_eth_start failed: {:#x}", err));
    }

    // ── 6. Wait for DHCP IP (covers both link-up + DHCP; up to 30 s) ──────
    // We combine checkpoints 3 and 4 into one poll: DHCP success implies
    // physical link is up.  Once we have an IP we query the link speed for
    // the checkpoint 3 message.
    let deadline = Instant::now() + Duration::from_secs(30);
    let got_ip = loop {
        match netif.is_up() {
            Ok(true) => break true,
            _ => {}
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(250));
    };

    if !got_ip {
        fail(4, "dhcp", "DHCP did not obtain an IP within 30 s (or link never came up)");
    }

    // Link must be up at this point — query speed for checkpoint 3 message.
    let mut speed: eth_speed_t = eth_speed_t_ETH_SPEED_10M;
    unsafe {
        esp_eth_ioctl(
            eth_handle,
            esp_eth_io_cmd_t_ETH_CMD_G_SPEED,
            &mut speed as *mut _ as *mut _,
        );
    }
    let speed_str = if speed == eth_speed_t_ETH_SPEED_100M { "100" } else { "10" };

    // Get MAC address via ioctl
    let mut mac_addr = [0u8; 6];
    unsafe {
        esp_eth_ioctl(
            eth_handle,
            esp_eth_io_cmd_t_ETH_CMD_G_MAC_ADDR,
            mac_addr.as_mut_ptr() as *mut _,
        );
    }

    pass(
        3,
        "ethernet_link",
        &format!(
            "link UP @ {} Mbps, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            speed_str,
            mac_addr[0], mac_addr[1], mac_addr[2],
            mac_addr[3], mac_addr[4], mac_addr[5],
        ),
    );

    // ── 7. Report DHCP result ──────────────────────────────────────────────
    let ip_info = match netif.get_ip_info() {
        Ok(info) => info,
        Err(e) => fail(4, "dhcp", &format!("get_ip_info failed: {:?}", e)),
    };

    let dns = netif.get_dns();

    pass(
        4,
        "dhcp",
        &format!(
            "ip={}  gw={}  dns={}",
            ip_info.ip, ip_info.subnet.gateway, dns
        ),
    );

    // Caller holds (netif, eth_handle) for the lifetime of the program.
    // Dropping either would tear down the link.
    (netif, eth_handle)
}

fn checkpoint_5_outbound_https() {
    use embedded_svc::http::client::Client;
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
    use std::time::Duration;

    let conn = match EspHttpConnection::new(&HttpConfig {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(Duration::from_secs(15)),
        ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => fail(5, "outbound_https", &format!("connection setup: {}", e)),
    };
    let mut client = Client::wrap(conn);
    let request = match client.get("https://httpbin.org/ip") {
        Ok(r) => r,
        Err(e) => fail(5, "outbound_https", &format!("request setup: {}", e)),
    };
    let mut response = match request.submit() {
        Ok(r) => r,
        Err(e) => fail(5, "outbound_https", &format!("submit: {}", e)),
    };
    let status = response.status();
    if status != 200 {
        fail(5, "outbound_https", &format!("HTTP {}", status));
    }
    let mut buf = [0u8; 256];
    let n = match response.read(&mut buf) {
        Ok(n) => n,
        Err(e) => fail(5, "outbound_https", &format!("body read: {}", e)),
    };
    if n == 0 {
        fail(5, "outbound_https", "read 0 bytes from body");
    }
    pass(5, "outbound_https", &format!("GET https://httpbin.org/ip → 200 ({} bytes)", n));
}

fn main() -> anyhow::Result<()> {
    link_patches();
    EspLogger::initialize_default();
    log::info!("zenclaw P4 smoke test booting");

    // EspSystemEventLoop::take() creates the default event loop, which is
    // required for DHCP events (IP_EVENT_ETH_GOT_IP) to fire.
    let _sysloop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;

    checkpoint_1_chip_info();
    checkpoint_2_psram();

    let _eth = checkpoint_3_4_ethernet_dhcp();

    checkpoint_5_outbound_https();

    log::info!("(checkpoint 6 not yet implemented)");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
