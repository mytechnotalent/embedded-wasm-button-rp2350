//! SPDX-License-Identifier: MIT
//!
//! Copyright (c) 2026 Kevin Thomas
//!
//! # Platform Glue for wasmtime no_std on RP2350
//!
//! Implements the minimum thread-local storage (TLS) symbols required by
//! wasmtime when running without an operating system. On this single-threaded
//! embedded platform, TLS is a simple global atomic pointer.

/// Null pointer constant for TLS initialization.
use core::ptr;
/// Atomic pointer for single-threaded TLS.
use core::sync::atomic::{AtomicPtr, Ordering};

/// Thread-local storage value used internally by the wasmtime runtime.
static TLS_VALUE: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());

/// Returns the current TLS value for the wasmtime runtime.
///
/// # Returns
///
/// The stored TLS pointer, or null if no value has been set.
#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_tls_get() -> *mut u8 {
    TLS_VALUE.load(Ordering::Relaxed)
}

/// Sets the current TLS value for the wasmtime runtime.
///
/// # Arguments
///
/// * `ptr` - The new TLS pointer value to store.
#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_tls_set(ptr: *mut u8) {
    TLS_VALUE.store(ptr, Ordering::Relaxed);
}
