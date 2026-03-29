//! SPDX-License-Identifier: MIT
//!
//! Copyright (c) 2026 Kevin Thomas
//!
//! # Button / GPIO Input Driver for RP2350 (Pico 2)
//!
//! Provides reading of multiple GPIO input pins via a critical-section mutex.
//! Pins are stored by their hardware GPIO number (e.g., 15 for a button)
//! so WASM code can address them directly. Accepts any pin that implements
//! `InputPin`. Designed as a shared plug-and-play module identical across repos.

#![allow(dead_code)]

// Enable the global allocator for heap-backed collections.
extern crate alloc;

/// Heap-allocated trait objects for type-erased pins.
use alloc::boxed::Box;
/// Sorted map keyed by GPIO pin number.
use alloc::collections::BTreeMap;
/// Interior mutability for the pin map.
use core::cell::RefCell;
/// Error type for infallible GPIO operations.
use core::convert::Infallible;
/// Interrupt-safe mutex for bare-metal concurrency.
use critical_section::Mutex;
/// Hardware abstraction trait for GPIO input.
use embedded_hal::digital::InputPin;

/// Type alias for a boxed GPIO input pin trait object.
type PinBox = Box<dyn InputPin<Error = Infallible> + Send>;

/// Global pin storage behind a critical-section mutex for safe shared access.
///
/// Pins are keyed by their hardware GPIO number.
static PINS: Mutex<RefCell<BTreeMap<u8, PinBox>>> = Mutex::new(RefCell::new(BTreeMap::new()));

/// Registers a GPIO input pin for shared access, keyed by its hardware pin number.
///
/// May be called multiple times to register different pins.
///
/// # Arguments
///
/// * `gpio_num` - Hardware GPIO pin number (e.g., 15 for a button).
/// * `pin` - Any GPIO pin configured as input with pull-up.
pub fn store_pin(gpio_num: u8, pin: impl InputPin<Error = Infallible> + Send + 'static) {
    critical_section::with(|cs| {
        PINS.borrow(cs).borrow_mut().insert(gpio_num, Box::new(pin));
    });
}

/// Returns whether the specified GPIO input pin is pressed (active-low).
///
/// Uses `is_low()` because the button is wired with a pull-up resistor:
/// pressing the button grounds the pin.
///
/// # Arguments
///
/// * `gpio_num` - Hardware GPIO pin number.
///
/// # Returns
///
/// `true` if the pin reads low (button pressed), `false` otherwise.
///
/// # Panics
///
/// Panics if the pin has not been registered via `store_pin`.
pub fn is_pressed(gpio_num: u8) -> bool {
    critical_section::with(|cs| {
        let map = PINS.borrow(cs);
        let mut map = map.borrow_mut();
        let pin = map.get_mut(&gpio_num).expect("pin not registered");
        pin.is_low().unwrap_or(false)
    })
}
