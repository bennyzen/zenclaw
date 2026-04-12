//! USB Host Mass Storage — mounts a FAT-formatted USB stick at `/usb`.
//!
//! Gated behind the `usb_storage` Cargo feature. Requires the
//! `espressif/usb_host_msc` managed component (declared in Cargo.toml).
//!
//! The module installs the USB Host library, spawns an event-processing
//! daemon thread, and registers the MSC class driver with a callback.
//! When a USB mass-storage device is plugged in the callback mounts it
//! via VFS; on disconnect it unmounts automatically.

use core::ffi::c_void;
use esp_idf_svc::sys;
use std::sync::atomic::{AtomicBool, Ordering};

const MOUNT_PATH: &[u8] = b"/usb\0";
const MAX_FILES: i32 = 5;

static MOUNTED: AtomicBool = AtomicBool::new(false);

// Handles are only written from the MSC event callback (single FreeRTOS task)
// and read only during disconnect within the same callback context.
static mut DEVICE: sys::msc_host_device_handle_t = core::ptr::null_mut();
static mut VFS: sys::msc_host_vfs_handle_t = core::ptr::null_mut();

/// Called by the MSC driver when a device connects or disconnects.
unsafe extern "C" fn on_msc_event(event: *const sys::msc_host_event_t, _arg: *mut c_void) {
    let ev = unsafe { &*event };

    // The event field is a c_uint enum; match against generated constants.
    #[allow(non_upper_case_globals)]
    match ev.event {
        sys::msc_host_event_t_MSC_DEVICE_CONNECTED => {
            let addr = unsafe { ev.device.address };
            log::info!("USB: device connected (addr={})", addr);

            let ret = unsafe { sys::msc_host_install_device(addr, &mut DEVICE) };
            if ret != 0 {
                log::error!("USB: msc_host_install_device failed (0x{:x})", ret);
                return;
            }

            // Print device descriptors for diagnostics
            unsafe { sys::msc_host_print_descriptors(DEVICE) };

            let mount_cfg = sys::esp_vfs_fat_mount_config_t {
                format_if_mount_failed: false,
                max_files: MAX_FILES,
                allocation_unit_size: 0,
                disk_status_check_enable: false,
                use_one_fat: false,
            };
            let ret = unsafe {
                sys::msc_host_vfs_register(
                    DEVICE,
                    MOUNT_PATH.as_ptr() as *const _,
                    &mount_cfg,
                    &mut VFS,
                )
            };
            if ret != 0 {
                log::error!("USB: VFS mount failed (0x{:x})", ret);
                unsafe { sys::msc_host_uninstall_device(DEVICE) };
                return;
            }

            MOUNTED.store(true, Ordering::Release);
            log::info!("USB: FAT mounted at /usb");
        }

        sys::msc_host_event_t_MSC_DEVICE_DISCONNECTED => {
            log::info!("USB: device disconnected");
            if MOUNTED.load(Ordering::Acquire) {
                unsafe {
                    sys::msc_host_vfs_unregister(VFS);
                    sys::msc_host_uninstall_device(DEVICE);
                    DEVICE = core::ptr::null_mut();
                    VFS = core::ptr::null_mut();
                }
                MOUNTED.store(false, Ordering::Release);
            }
        }

        _ => {}
    }
}

/// Install USB Host stack + MSC driver. Non-blocking — returns immediately.
/// The actual mount happens asynchronously when a device is plugged in.
pub fn init() {
    // 1. Install USB Host library
    let host_cfg = sys::usb_host_config_t {
        skip_phy_setup: false,
        root_port_unpowered: false,
        intr_flags: 0,
        enum_filter_cb: None,
    };
    let ret = unsafe { sys::usb_host_install(&host_cfg) };
    if ret != 0 {
        log::error!("USB: usb_host_install failed (0x{:x})", ret);
        return;
    }

    // 2. Daemon thread — pumps USB Host library events (must run continuously)
    std::thread::Builder::new()
        .name("usb_host".into())
        .stack_size(4096)
        .spawn(|| loop {
            let mut flags: u32 = 0;
            unsafe { sys::usb_host_lib_handle_events(u32::MAX, &mut flags) };
        })
        .expect("USB Host daemon thread");

    // 3. Install MSC class driver (creates its own background task)
    let msc_cfg = sys::msc_host_driver_config_t {
        create_backround_task: true,
        task_priority: 5,
        stack_size: 4096,
        core_id: 0,
        callback: Some(on_msc_event),
        callback_arg: core::ptr::null_mut(),
    };
    let ret = unsafe { sys::msc_host_install(&msc_cfg) };
    if ret != 0 {
        log::error!("USB: msc_host_install failed (0x{:x})", ret);
        return;
    }

    log::info!("USB: Host MSC ready, waiting for device...");
}

/// Whether a USB mass-storage device is currently mounted.
pub fn is_mounted() -> bool {
    MOUNTED.load(Ordering::Relaxed)
}

/// VFS mount path (always "/usb").
pub fn mount_path() -> &'static str {
    "/usb"
}
