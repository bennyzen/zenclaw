//! WS2812 LED status indicator via ESP-IDF led_strip component (RMT).
//!
//! Non-blocking: state is a global AtomicU8. A background thread
//! renders the LED at 20fps. If led_strip init fails, everything noops.

use esp_idf_svc::sys;
use std::sync::atomic::{AtomicU8, Ordering};

const MAX_BRIGHT: u8 = 40;
const TICK_MS: u64 = 50;
const STACK_SIZE: usize = 4096;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
pub enum State {
    Off = 0,
    Boot = 1,
    WifiConnecting = 2,
    WifiFailed = 3,
    Idle = 4,
    Thinking = 5,
    Error = 6,
    Updating = 7,
}

static LED_STATE: AtomicU8 = AtomicU8::new(State::Off as u8);

pub fn set(state: State) {
    LED_STATE.store(state as u8, Ordering::Relaxed);
}

pub fn init(gpio_pin: i32) {
    let strip = unsafe { create_strip(gpio_pin) };
    if strip.is_null() {
        log::warn!("LED: failed to init led_strip on GPIO {}", gpio_pin);
        return;
    }
    log::info!("LED: WS2812 on GPIO {}", gpio_pin);

    // Self-test: red → green → blue
    for &(r, g, b) in &[(60u8, 0u8, 0u8), (0, 60, 0), (0, 0, 60)] {
        unsafe {
            sys::led_strip_set_pixel(strip, 0, r as u32, g as u32, b as u32);
            sys::led_strip_refresh(strip);
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    unsafe { sys::led_strip_clear(strip); }

    // Cast to usize to cross the Send boundary — we're the sole owner.
    let ptr = strip as usize;
    match std::thread::Builder::new()
        .name("led".into())
        .stack_size(STACK_SIZE)
        .spawn(move || render_loop(ptr as sys::led_strip_handle_t))
    {
        Ok(_) => log::info!("LED: render thread started"),
        Err(e) => log::error!("LED: thread spawn failed: {}", e),
    }
}

unsafe fn create_strip(gpio_pin: i32) -> sys::led_strip_handle_t {
    let mut strip_config: sys::led_strip_config_t = std::mem::zeroed();
    strip_config.strip_gpio_num = gpio_pin;
    strip_config.max_leds = 1;
    // led_model = 0 (WS2812), color_component_format = 0 (GRB) — from zeroed

    let mut rmt_config: sys::led_strip_rmt_config_t = std::mem::zeroed();
    rmt_config.resolution_hz = 10_000_000; // 10 MHz
    // mem_block_symbols = 0 → driver default, with_dma = 0 — from zeroed

    let mut handle: sys::led_strip_handle_t = std::ptr::null_mut();
    let ret = sys::led_strip_new_rmt_device(&strip_config, &rmt_config, &mut handle);
    if ret != 0 {
        log::warn!("LED: led_strip_new_rmt_device failed (0x{:x})", ret);
        return std::ptr::null_mut();
    }
    handle
}

/// (R, G, B) base color for each state.
fn color(state: u8) -> (u8, u8, u8) {
    match state {
        1 => (MAX_BRIGHT, MAX_BRIGHT, MAX_BRIGHT), // Boot: white
        2 => (0, 0, MAX_BRIGHT),                   // WiFi connecting: blue
        3 => (MAX_BRIGHT, 0, 0),                   // WiFi failed: red
        4 => (0, MAX_BRIGHT / 3, 0),               // Idle: dim green
        5 => (0, MAX_BRIGHT / 2, MAX_BRIGHT / 2),  // Thinking: cyan
        6 => (MAX_BRIGHT, 0, 0),                   // Error: red
        7 => (MAX_BRIGHT, MAX_BRIGHT / 2, 0),      // Updating: amber
        _ => (0, 0, 0),
    }
}

/// Scale factor (0..255) for brightness modulation based on state + tick.
fn scale(state: u8, tick: u32) -> u8 {
    match state {
        0 => 0,
        1 => 255,                                                        // Boot: solid
        2 => if (tick % 10) < 5 { 255 } else { 0 },                     // WiFi: blink 2Hz
        3 => if (tick % 20) < 10 { 255 } else { 0 },                    // WiFi fail: blink 1Hz
        4 => 255,                                                        // Idle: solid
        5 => {                                                           // Thinking: breathe
            let period = 24u32;
            let half = period / 2;
            let pos = tick % period;
            let level = if pos < half { pos } else { period - pos };
            (40 + level * 215 / half) as u8
        }
        6 => 255,                                                        // Error: solid
        7 => if (tick % 10) < 5 { 255 } else { 0 },                     // Updating: blink
        _ => 0,
    }
}

fn render_loop(strip: sys::led_strip_handle_t) {
    log::info!("LED: render loop running");
    let mut tick: u32 = 0;
    let mut prev_state: u8 = 255;
    let mut prev_scale: u8 = 255;

    loop {
        let state = LED_STATE.load(Ordering::Relaxed);
        if state != prev_state {
            tick = 0;
            prev_state = state;
        }

        let s = scale(state, tick);
        if s != prev_scale || tick == 0 {
            let (r, g, b) = color(state);
            let r = (r as u16 * s as u16 / 255) as u32;
            let g = (g as u16 * s as u16 / 255) as u32;
            let b = (b as u16 * s as u16 / 255) as u32;
            unsafe {
                sys::led_strip_set_pixel(strip, 0, r, g, b);
                sys::led_strip_refresh(strip);
            }
            prev_scale = s;
        }

        tick = tick.wrapping_add(1);
        std::thread::sleep(std::time::Duration::from_millis(TICK_MS));
    }
}
