pub mod config;
pub mod core;
#[cfg(feature = "desktop")]
pub mod desktop;
#[cfg(feature = "esp32")]
pub mod esp32;
#[cfg(feature = "esp32")]
pub mod led_status;
#[cfg(feature = "esp32")]
pub mod net;
#[cfg(feature = "usb_storage")]
pub mod usb_storage;
#[cfg(feature = "sdcard")]
pub mod sdcard;
pub mod platform;

/// Global TLS connection mutex — ESP32 without PSRAM can only sustain one TLS context at a time.
/// Acquire before opening any outbound HTTPS connection, release after the connection is closed.
#[cfg(feature = "esp32")]
pub static TLS_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
