//! IP101 EMAC Ethernet driver for ESP32-P4 via raw `esp_idf_sys` FFI.
//!
//! # Why raw FFI instead of `esp_idf_svc::eth`?
//!
//! `esp-idf-svc 0.52` gates its entire `eth` module — `EthDriver`, `EspEth`,
//! `BlockingEth`, `RmiiEth`, `RmiiEthChipset`, `RmiiClockConfig` — on chip-
//! specific cfg predicates:
//!
//! ```text
//! #[cfg(any(
//!     all(esp32, esp_idf_eth_use_esp32_emac),   // only original ESP32
//!     any(esp_idf_eth_spi_ethernet_*),
//!     esp_idf_eth_use_openeth
//! ))]
//! pub mod eth;
//! ```
//!
//! The P4 target (`riscv32imafc-esp-espidf`) has `esp32 = false`, so the
//! module is entirely absent.  We call the underlying C functions directly via
//! `esp_idf_svc::sys`, which is always present, and use the unconditional
//! `EspNetif` wrapper for IP polling.
//!
//! The required `esp_eth_*` symbols are included in the bindings automatically
//! when `CONFIG_ETH_USE_ESP32_EMAC=y` is set in the board sdkconfig — no extra
//! bindings header is needed.
//!
//! # Pin map — Guition JC-ESP32P4-M3-DEV, IP101 PHY addr=1
//!
//! | Signal    | GPIO |
//! |-----------|------|
//! | TX_EN     | 49   |  ← IDF default for P4; plan said 33 which is WRONG
//! | TXD0      | 34   |
//! | TXD1      | 35   |
//! | CRS_DV    | 28   |
//! | RXD0      | 29   |
//! | RXD1      | 30   |
//! | MDC       | 31   |
//! | MDIO      | 52   |
//! | REF_CLK   | 50   |  50 MHz external input from PHY oscillator
//! | PHY_PWR   | 51   |  hw-reset; IDF asserts low then releases
//!
//! GPIO49 (TX_EN) yields a working 100 Mbps link; GPIO33 leaves link down.
//! Empirically verified on hardware (DHCP → 192.168.50.213).

use crate::net::{IpInfo, Nic, NicKind};
use anyhow::Context;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::netif::{EspNetif, NetifStack};
use esp_idf_svc::sys::*;
use std::time::{Duration, Instant};

// ── EthNic ────────────────────────────────────────────────────────────────────

pub struct EthNic {
    /// Holds the lwIP netif alive for the program lifetime.
    netif: EspNetif,
    /// Opaque handle to the ESP-IDF Ethernet driver.  All operations through
    /// this handle are internally serialised by the driver's rx_task; the
    /// pointer itself is stable for the driver lifetime.
    eth_handle: esp_eth_handle_t,
    /// MAC address cached at bring-up to avoid an ioctl on every `mac()` call.
    mac: [u8; 6],
}

// SAFETY: `esp_eth_handle_t` is `*mut c_void` which is `!Send + !Sync` by
// default.  The ESP-IDF Ethernet driver is internally thread-safe (all frame
// processing happens in its dedicated rx_task, and ioctls are serialised by
// an internal mutex), so it is safe to move `EthNic` across threads and share
// read-only references.
unsafe impl Send for EthNic {}
unsafe impl Sync for EthNic {}

impl Nic for EthNic {
    fn kind(&self) -> NicKind {
        NicKind::Ethernet
    }

    fn link_up(&self) -> bool {
        self.netif.is_up().unwrap_or(false)
    }

    fn ip_info(&self) -> Option<IpInfo> {
        self.netif.get_ip_info().ok().map(|i| IpInfo {
            ip: i.ip,
            gateway: i.subnet.gateway,
            netmask: prefix_to_mask(i.subnet.mask.0),
            dns: i.dns,
        })
    }

    fn link_speed_mbps(&self) -> Option<u32> {
        let mut speed: eth_speed_t = eth_speed_t_ETH_SPEED_10M;
        let ret = unsafe {
            esp_eth_ioctl(
                self.eth_handle,
                esp_eth_io_cmd_t_ETH_CMD_G_SPEED,
                &mut speed as *mut _ as *mut _,
            )
        };
        if ret != ESP_OK {
            return None;
        }
        if speed == eth_speed_t_ETH_SPEED_100M {
            Some(100)
        } else if speed == eth_speed_t_ETH_SPEED_10M {
            Some(10)
        } else {
            None
        }
    }

    fn ssid(&self) -> Option<String> {
        None
    }

    fn rssi(&self) -> Option<i32> {
        None
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }
}

// ── bring_up ──────────────────────────────────────────────────────────────────

/// Bring up the internal EMAC + IP101 PHY, attach an lwIP netif, start the
/// driver, and wait for DHCP to assign an IP (up to 30 s).
///
/// On success returns a `Box<dyn Nic>` wrapping [`EthNic`].
/// On failure sets the LED to the "failed" state and returns an error.
///
/// `peripherals` and `sysloop` mirror the signature of the WiFi bring-up so
/// the caller can treat both paths uniformly.  The `EspSystemEventLoop` must
/// already be initialised (it drives DHCP IP events); pass the same instance
/// that `main` takes from `EspSystemEventLoop::take()`.
#[allow(unused_variables)]
pub fn bring_up(
    _peripherals: esp_idf_svc::hal::peripherals::Peripherals,
    _sysloop: esp_idf_svc::eventloop::EspSystemEventLoop,
) -> anyhow::Result<Box<dyn Nic>> {
    crate::led_status::set(crate::led_status::State::LinkConnecting);

    match bring_up_inner() {
        Ok(nic) => Ok(nic),
        Err(e) => {
            crate::led_status::set(crate::led_status::State::LinkFailed);
            Err(e)
        }
    }
}

fn bring_up_inner() -> anyhow::Result<Box<dyn Nic>> {
    // ── 1. Build EMAC config for ESP32-P4 ────────────────────────────────────
    // ESP32-P4 has SOC_EMAC_USE_MULTI_IO_MUX=y; RMII data pins are freely
    // assignable via the emac_dataif_gpio field.
    let esp32_mac_config = eth_esp32_emac_config_t {
        __bindgen_anon_1: eth_esp32_emac_config_t__bindgen_ty_1 {
            smi_gpio: emac_esp_smi_gpio_config_t {
                mdc_num: 31,  // MDC
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
        // clock_config_out_in only matters when the RMII clock is generated
        // internally and looped back externally.  Disabled here (external input).
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
                tx_en_num: 49,  // TX_EN — GPIO49, NOT 33 (plan was wrong)
                txd0_num: 34,   // TXD0
                txd1_num: 35,   // TXD1
                crs_dv_num: 28, // CRS_DV
                rxd0_num: 29,   // RXD0
                rxd1_num: 30,   // RXD1
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
        anyhow::bail!("esp_eth_mac_new_esp32 returned NULL");
    }

    // ── 2. Build IP101 PHY (addr=1, hw-reset via GPIO51) ─────────────────────
    let phy_config = eth_phy_config_t {
        phy_addr: 1,
        reset_timeout_ms: 100,
        autonego_timeout_ms: 4000,
        reset_gpio_num: 51, // PHY_PWR/RST
        hw_reset_assert_time_us: 0,
        post_hw_reset_delay_ms: 0,
    };

    let phy = unsafe { esp_eth_phy_new_ip101(&phy_config) };
    if phy.is_null() {
        anyhow::bail!("esp_eth_phy_new_ip101 returned NULL");
    }

    // ── 3. Install Ethernet driver ────────────────────────────────────────────
    let eth_cfg = esp_eth_config_t {
        mac,
        phy,
        check_link_period_ms: 2000,
        ..Default::default()
    };

    let mut eth_handle: esp_eth_handle_t = std::ptr::null_mut();
    let err = unsafe { esp_eth_driver_install(&eth_cfg, &mut eth_handle) };
    if err != ESP_OK {
        anyhow::bail!("esp_eth_driver_install failed: {:#x}", err);
    }

    // ── 4. Create default ETH netif (DHCP client) and attach glue ────────────
    // EspNetif::new creates the default Ethernet netif with DHCP client enabled,
    // equivalent to ESP_NETIF_DEFAULT_ETH().
    let netif = EspNetif::new(NetifStack::Eth).context("EspNetif::new(Eth)")?;

    let glue = unsafe { esp_eth_new_netif_glue(eth_handle) };
    if glue.is_null() {
        anyhow::bail!("esp_eth_new_netif_glue returned NULL");
    }

    // RawHandle::handle() returns *mut esp_netif_t for EspNetif.
    let err = unsafe { esp_netif_attach(netif.handle(), glue as *mut _) };
    if err != ESP_OK {
        anyhow::bail!("esp_netif_attach failed: {:#x}", err);
    }

    // ── 5. Start the driver ───────────────────────────────────────────────────
    let err = unsafe { esp_eth_start(eth_handle) };
    if err != ESP_OK {
        anyhow::bail!("esp_eth_start failed: {:#x}", err);
    }

    log::info!("eth: driver started, waiting for DHCP (up to 30s)…");

    // ── 6. Wait for DHCP IP (implies link-up; up to 30 s) ────────────────────
    let deadline = Instant::now() + Duration::from_secs(30);
    let got_ip = loop {
        if let Ok(true) = netif.is_up() {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(250));
    };

    if !got_ip {
        anyhow::bail!("Ethernet DHCP did not obtain an IP within 30s (link never came up?)");
    }

    // ── 7. Cache the MAC address via ioctl ───────────────────────────────────
    let mut mac_addr = [0u8; 6];
    unsafe {
        esp_eth_ioctl(
            eth_handle,
            esp_eth_io_cmd_t_ETH_CMD_G_MAC_ADDR,
            mac_addr.as_mut_ptr() as *mut _,
        );
    }

    // Log the outcome.
    let mut speed: eth_speed_t = eth_speed_t_ETH_SPEED_10M;
    unsafe {
        esp_eth_ioctl(
            eth_handle,
            esp_eth_io_cmd_t_ETH_CMD_G_SPEED,
            &mut speed as *mut _ as *mut _,
        );
    }
    let speed_mbps = if speed == eth_speed_t_ETH_SPEED_100M {
        100u32
    } else {
        10u32
    };

    let ip_info = netif.get_ip_info().context("get_ip_info after DHCP")?;
    log::info!(
        "eth: link UP @ {} Mbps  ip={}  gw={}  mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        speed_mbps,
        ip_info.ip,
        ip_info.subnet.gateway,
        mac_addr[0], mac_addr[1], mac_addr[2],
        mac_addr[3], mac_addr[4], mac_addr[5],
    );

    Ok(Box::new(EthNic {
        netif,
        eth_handle,
        mac: mac_addr,
    }))
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Convert an IPv4 prefix length (0–32) to an `Ipv4Addr` netmask.
///
/// Duplicated from `net::wifi` — factoring into `net::mod` would require
/// a public free function visible to all child modules; for now the duplication
/// is acceptable given the function is trivial (two lines of bit arithmetic).
fn prefix_to_mask(prefix_len: u8) -> std::net::Ipv4Addr {
    if prefix_len == 0 {
        std::net::Ipv4Addr::UNSPECIFIED
    } else {
        let m = !0u32 << (32 - prefix_len as u32);
        std::net::Ipv4Addr::from(m)
    }
}
