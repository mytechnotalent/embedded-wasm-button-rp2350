//! SPDX-License-Identifier: MIT
//!
//! Copyright (c) 2026 Kevin Thomas
//!
//! # WASM Button Component
//!
//! A minimal WebAssembly component that reads a button on GPIO15 and
//! controls the onboard LED on GPIO25 of an RP2350 Pico 2 by calling
//! host-provided GPIO, button, and delay functions through typed WIT
//! interfaces. GPIO pins are addressed by their hardware pin number.

#![no_std]

// Enable the global allocator for heap-backed collections.
extern crate alloc;

/// Panic handler signature type.
use core::panic::PanicInfo;

/// Global heap allocator required by the canonical ABI's `cabi_realloc`.
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

/// Host-provided button input imports.
use embedded::platform::button;
/// Host-provided GPIO output imports.
use embedded::platform::gpio;
/// Host-provided timing imports.
use embedded::platform::timing;

// Generate guest-side bindings for the `button-led` WIT world.
wit_bindgen::generate!({
    world: "button-led",
    path: "../wit",
});

/// WASM guest component implementing the `button` world.
struct ButtonApp;

// Register `ButtonApp` as the component's exported implementation.
export!(ButtonApp);

impl Guest for ButtonApp {
    /// Polls the button and mirrors its state to the LED at 10ms intervals.
    fn run() {
        /// Hardware GPIO pin number for the button input.
        const BUTTON_PIN: u32 = 15;
        /// Hardware GPIO pin number for the onboard LED.
        const LED_PIN: u32 = 25;
        loop {
            if button::is_pressed(BUTTON_PIN) {
                gpio::set_high(LED_PIN);
            } else {
                gpio::set_low(LED_PIN);
            }
            timing::delay_ms(10);
        }
    }
}

/// Panic handler for the WASM environment that halts in an infinite loop.
///
/// # Arguments
///
/// * `_info` - Panic information (unused in the WASM environment).
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
