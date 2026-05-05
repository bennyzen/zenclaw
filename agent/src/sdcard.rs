//! SD card (FATFS over SDMMC) — mounts a microSD at `/sdcard`.
//!
//! Gated behind the `sdcard` Cargo feature. Currently only enabled on the
//! Guition P4 board (DevKitC has no slot). The actual SDMMC + FATFS glue
//! lives in C at `components/zenclaw_sd/` because IDF's
//! `SDMMC_HOST_DEFAULT()` macro initializes 17 function pointers plus an
//! anonymous union on `sdmmc_host_t` — fragile to mirror in Rust.
//!
//! Boot sequence: `init()` runs once after the LittleFS mount. Failure is
//! non-fatal — the agent continues to run with `/sdcard` absent and
//! reports `mounted: false` in `/api/status.sdcard`.
//!
//! Path policy: `/sdcard` joins `/data` as a writable mount the file API
//! is allowed to touch. The jail in `main.rs::jail_filesystem_path` rejects
//! `/sdcard/*` if `is_mounted()` returns false.

use esp_idf_svc::sys;

/// Try to mount the SD card. Logs success or failure; never panics.
pub fn init() {
    let ret = unsafe { sys::zenclaw_sd_mount() };
    if ret == 0 {
        let bus = bus_width();
        let kind = type_str();
        log::info!(
            "SD card: mounted at /sdcard ({}-bit, type={})",
            bus, kind
        );
    } else {
        log::warn!(
            "SD card: mount failed (0x{:x}); /sdcard unavailable",
            ret as u32
        );
    }
}

pub fn is_mounted() -> bool {
    unsafe { sys::zenclaw_sd_is_mounted() }
}

/// Return `(total_bytes, free_bytes)` from FATFS f_getfree, or None if not
/// mounted or FATFS reports an error.
pub fn info() -> Option<(u64, u64)> {
    let mut total: u64 = 0;
    let mut free: u64 = 0;
    let ret = unsafe { sys::zenclaw_sd_info(&mut total, &mut free) };
    if ret == 0 { Some((total, free)) } else { None }
}

/// "SDHC" / "SDSC" / "MMC" / "SDIO" / "none". Always returns a string.
pub fn type_str() -> String {
    unsafe {
        let p = sys::zenclaw_sd_type();
        if p.is_null() {
            return "none".into();
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

pub fn bus_width() -> u8 {
    unsafe { sys::zenclaw_sd_bus_width() as u8 }
}
