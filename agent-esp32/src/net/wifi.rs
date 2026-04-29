//! Internal WiFi driver implementation (S3/S2 native EspWifi).
//! Hosted (esp_hosted) variant lives behind the `nic-wifi-hosted` feature.

use crate::net::{IpInfo, Nic, NicKind};
use anyhow::Context;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi};
use std::time::Duration;

#[cfg(feature = "nic-wifi-internal")]
pub struct WifiNic {
    wifi: EspWifi<'static>,
    ssid: Option<String>,
}

#[cfg(feature = "nic-wifi-internal")]
impl Nic for WifiNic {
    fn kind(&self) -> NicKind {
        NicKind::Wifi
    }
    fn link_up(&self) -> bool {
        self.wifi.is_connected().unwrap_or(false)
    }
    fn ip_info(&self) -> Option<IpInfo> {
        let netif = self.wifi.sta_netif();
        netif.get_ip_info().ok().map(|i| IpInfo {
            ip: i.ip,
            gateway: i.subnet.gateway,
            netmask: ipv4_mask(i.subnet.mask.0),
            dns: i.dns,
        })
    }
    fn link_speed_mbps(&self) -> Option<u32> {
        None
    }
    fn ssid(&self) -> Option<String> {
        self.ssid.clone()
    }
    fn rssi(&self) -> Option<i32> {
        let mut info: esp_idf_svc::sys::wifi_ap_record_t = unsafe { std::mem::zeroed() };
        let ret = unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut info) };
        if ret == 0 {
            Some(info.rssi as i32)
        } else {
            None
        }
    }
    fn mac(&self) -> [u8; 6] {
        self.wifi.sta_netif().get_mac().unwrap_or([0; 6])
    }
}

fn ipv4_mask(prefix_len: u8) -> std::net::Ipv4Addr {
    if prefix_len == 0 {
        std::net::Ipv4Addr::UNSPECIFIED
    } else {
        let m = !0u32 << (32 - prefix_len);
        std::net::Ipv4Addr::from(m)
    }
}

#[cfg(feature = "nic-wifi-internal")]
pub fn bring_up_internal(
    peripherals: Peripherals,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> anyhow::Result<Box<dyn Nic>> {
    let creds = crate::net::wifi_ui::read_credentials(&nvs);
    let (ssid, password) = creds
        .as_ref()
        .map(|(s, p)| (s.as_str(), p.as_deref().unwrap_or("")))
        .unwrap_or(("", ""));

    let mut wifi = EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))
        .context("EspWifi::new")?;

    let mut ssid_buf = heapless::String::<32>::new();
    let _ = ssid_buf.push_str(ssid);
    let mut pw_buf = heapless::String::<64>::new();
    let _ = pw_buf.push_str(password);

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid_buf,
        password: pw_buf,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;
    wifi.start()?;
    crate::led_status::set(crate::led_status::State::LinkConnecting);
    if wifi.connect().is_err() {
        crate::led_status::set(crate::led_status::State::LinkFailed);
        anyhow::bail!("wifi.connect() failed");
    }

    // Wait up to 15s for link
    for _ in 0..30 {
        if wifi.is_connected().unwrap_or(false) {
            if wifi.sta_netif().get_ip_info().is_ok() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if !wifi.is_connected().unwrap_or(false) {
        crate::led_status::set(crate::led_status::State::LinkFailed);
        anyhow::bail!("wifi did not connect within 15s");
    }

    Ok(Box::new(WifiNic {
        wifi,
        ssid: if ssid.is_empty() {
            None
        } else {
            Some(ssid.to_string())
        },
    }))
}

#[cfg(feature = "nic-wifi-hosted")]
pub fn bring_up_hosted(
    _peripherals: Peripherals,
    _sysloop: EspSystemEventLoop,
    _nvs: EspDefaultNvsPartition,
) -> anyhow::Result<Box<dyn Nic>> {
    anyhow::bail!(
        "nic-wifi-hosted (esp_hosted via C6 SDIO) is not implemented in v1; deferred to v2"
    )
}
