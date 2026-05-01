//! WiFi UI / config layer — always compiled regardless of which WiFi driver
//! (or none) is active. Reads/writes credentials in NVS (`wifi` namespace,
//! keys `ssid` and `password`). Used by /api/wifi GET/PUT handlers.

use anyhow::Context;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

const NVS_NAMESPACE: &str = "wifi";
const SSID_KEY: &str = "ssid";
const PASSWORD_KEY: &str = "password";

pub fn read_credentials(nvs: &EspDefaultNvsPartition) -> Option<(String, Option<String>)> {
    let handle = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true).ok()?;

    // Try string first; fall back to blob (matches existing provisioning paths).
    let mut ssid_buf = [0u8; 64];
    let ssid = handle
        .get_str(SSID_KEY, &mut ssid_buf)
        .ok()
        .flatten()
        .map(|s| s.to_string())
        .or_else(|| {
            let mut buf = [0u8; 64];
            handle
                .get_blob(SSID_KEY, &mut buf)
                .ok()
                .flatten()
                .and_then(|b| std::str::from_utf8(b).ok().map(|s| s.to_string()))
        })?;

    let mut pw_buf = [0u8; 128];
    let password = handle
        .get_str(PASSWORD_KEY, &mut pw_buf)
        .ok()
        .flatten()
        .map(|s| s.to_string())
        .or_else(|| {
            let mut buf = [0u8; 128];
            handle
                .get_blob(PASSWORD_KEY, &mut buf)
                .ok()
                .flatten()
                .and_then(|b| std::str::from_utf8(b).ok().map(|s| s.to_string()))
        });

    Some((ssid, password))
}

pub fn write_credentials(
    nvs: &EspDefaultNvsPartition,
    ssid: &str,
    password: Option<&str>,
) -> anyhow::Result<()> {
    let mut handle =
        EspNvs::new(nvs.clone(), NVS_NAMESPACE, true).context("EspNvs::new wifi")?;
    handle.set_str(SSID_KEY, ssid).context("set ssid")?;
    if let Some(pw) = password {
        handle.set_str(PASSWORD_KEY, pw).context("set password")?;
    }
    Ok(())
}

/// What `/api/status.wifi.driver` reports.
pub fn driver_label() -> &'static str {
    #[cfg(feature = "nic-wifi-internal")]
    {
        return "internal";
    }
    #[cfg(all(feature = "nic-wifi-hosted", not(feature = "nic-wifi-internal")))]
    {
        return "hosted";
    }
    #[cfg(not(any(feature = "nic-wifi-internal", feature = "nic-wifi-hosted")))]
    {
        return "none";
    }
}
