//! NIC abstraction. Each board picks a driver via cargo features
//! (`nic-wifi-internal`, `nic-wifi-hosted`, `nic-eth`); main.rs only sees the
//! `Nic` trait.
//!
//! This module declares the trait and its companion types only. The actual
//! `wifi` and `eth` submodules are added in Chunks C3 and C4 respectively, and
//! the dispatch helper that picks one at runtime is wired up in C6 (main.rs
//! refactor). Defining the contract first lets later chunks add implementations
//! without churning the trait shape.

use std::net::Ipv4Addr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicKind {
    Wifi,
    Ethernet,
}

#[derive(Debug, Clone, Copy)]
pub struct IpInfo {
    pub ip: Ipv4Addr,
    pub gateway: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub dns: Option<Ipv4Addr>,
}

pub trait Nic: Send + Sync {
    fn kind(&self) -> NicKind;
    fn link_up(&self) -> bool;
    fn ip_info(&self) -> Option<IpInfo>;
    fn link_speed_mbps(&self) -> Option<u32>;
    fn ssid(&self) -> Option<String>;
    fn rssi(&self) -> Option<i32>;
    fn mac(&self) -> [u8; 6];
}

#[cfg(any(feature = "nic-wifi-internal", feature = "nic-wifi-hosted"))]
pub mod wifi;

// Always compiled — owns NVS-based credential read/write and the driver label,
// independent of which radio (or none) is active. /api/wifi handlers use it.
pub mod wifi_ui;

#[cfg(feature = "nic-eth")]
pub mod eth;

/// Bring up the primary NIC chosen by cargo features. Each board's manifest
/// enables exactly one of `nic-eth`, `nic-wifi-internal`, or `nic-wifi-hosted`,
/// so only the matching arm compiles.
#[cfg(feature = "esp32")]
pub fn bring_up_primary(
    peripherals: esp_idf_svc::hal::peripherals::Peripherals,
    sysloop: esp_idf_svc::eventloop::EspSystemEventLoop,
    nvs: esp_idf_svc::nvs::EspDefaultNvsPartition,
) -> anyhow::Result<Box<dyn Nic>> {
    #[cfg(feature = "nic-eth")]
    {
        let _ = nvs; // unused on Eth — credentials come from DHCP
        return eth::bring_up(peripherals, sysloop);
    }
    #[cfg(all(feature = "nic-wifi-internal", not(feature = "nic-eth")))]
    {
        return wifi::bring_up_internal(peripherals, sysloop, nvs);
    }
    #[cfg(all(
        feature = "nic-wifi-hosted",
        not(feature = "nic-wifi-internal"),
        not(feature = "nic-eth")
    ))]
    {
        return wifi::bring_up_hosted(peripherals, sysloop, nvs);
    }
    #[cfg(not(any(
        feature = "nic-wifi-internal",
        feature = "nic-wifi-hosted",
        feature = "nic-eth",
    )))]
    {
        compile_error!("at least one NIC feature must be enabled (nic-wifi-internal | nic-wifi-hosted | nic-eth)");
    }
}
