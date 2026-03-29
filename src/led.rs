//! SPDX-License-Identifier: MIT
//!
//! Copyright (c) 2026 Kevin Thomas
//!
//! # LED / GPIO Output Driver for RP2350 (Pico 2)
//!
//! Provides control of multiple GPIO output pins via a critical-section mutex.
//! Pins are stored by their hardware GPIO number (e.g., 25 for the onboard LED)
//! so WASM code can address them directly. Accepts any pin that implements
//! `OutputPin`. Designed as a shared plug-and-play module identical across repos.

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
/// Hardware abstraction trait for GPIO output.
use embedded_hal::digital::OutputPin;

/// Type alias for a boxed GPIO output pin trait object.
type PinBox = Box<dyn OutputPin<Error = Infallible> + Send>;

/// Global pin storage behind a critical-section mutex for safe shared access.
///
/// Pins are keyed by their hardware GPIO number.
static PINS: Mutex<RefCell<BTreeMap<u8, PinBox>>> = Mutex::new(RefCell::new(BTreeMap::new()));

/// Registers a GPIO pin for shared access, keyed by its hardware pin number.
///
/// May be called multiple times to register different pins.
///
/// # Arguments
///
/// * `gpio_num` - Hardware GPIO pin number (e.g., 25 for onboard LED).
/// * `pin` - Any GPIO pin configured as push-pull output.
pub fn store_pin(gpio_num: u8, pin: impl OutputPin<Error = Infallible> + Send + 'static) {
    critical_section::with(|cs| {
        PINS.borrow(cs).borrow_mut().insert(gpio_num, Box::new(pin));
    });
}

/// Sets the specified GPIO pin high (on).
///
/// # Arguments
///
/// * `gpio_num` - Hardware GPIO pin number.
///
/// # Panics
///
/// Panics if the pin has not been registered via `store_pin`.
pub fn set_high(gpio_num: u8) {
    critical_section::with(|cs| {
        let map = PINS.borrow(cs);
        let mut map = map.borrow_mut();
        let pin = map.get_mut(&gpio_num).expect("pin not registered");
        let _ = pin.set_high();
    });
}

/// Sets the specified GPIO pin low (off).
///
/// # Arguments
///
/// * `gpio_num` - Hardware GPIO pin number.
///
/// # Panics
///
/// Panics if the pin has not been registered via `store_pin`.
pub fn set_low(gpio_num: u8) {
    critical_section::with(|cs| {
        let map = PINS.borrow(cs);
        let mut map = map.borrow_mut();
        let pin = map.get_mut(&gpio_num).expect("pin not registered");
        let _ = pin.set_low();
    });
}
