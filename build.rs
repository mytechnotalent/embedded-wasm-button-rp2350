//! SPDX-License-Identifier: MIT
//!
//! Copyright (c) 2026 Kevin Thomas
//!
//! # Build Script for embedded-wasm-button-rp2350 Firmware
//!
//! Sets up the RP2350 linker script, compiles the WASM button application,
//! and AOT-compiles the WASM binary to Pulley bytecode for the RP2350.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use wasmtime::{Config, Engine};
use wit_component::ComponentEncoder;

/// Creates the output directory path and registers it for linker search.
///
/// # Returns
///
/// The output directory path as a `PathBuf`.
///
/// # Panics
///
/// Panics if the `OUT_DIR` environment variable is not set.
fn setup_output_dir() -> PathBuf {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    println!("cargo:rustc-link-search={}", out.display());
    out
}

/// Writes the RP2350 memory layout linker script to the output directory.
///
/// # Arguments
///
/// * `out` - Output directory path where `memory.x` will be created.
///
/// # Panics
///
/// Panics if the file cannot be created or written.
fn write_linker_script(out: &Path) {
    let memory_x = include_bytes!("rp2350.x");
    let mut f = File::create(out.join("memory.x")).unwrap();
    f.write_all(memory_x).unwrap();
}

/// Compiles the WASM button application for the `wasm32-unknown-unknown` target.
///
/// # Panics
///
/// Panics if the WASM compilation fails.
fn compile_wasm_app() {
    let status = Command::new("cargo")
        .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
        .current_dir("wasm-app")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .expect("failed to build WASM app");
    assert!(status.success(), "WASM app compilation failed");
}

/// Creates a wasmtime engine configured to cross-compile for Pulley 32-bit.
///
/// Mirrors the runtime engine configuration on the RP2350 so that the
/// precompiled bytecode matches the device's expectations: no signal-based
/// traps (bare-metal has no OS signal handlers) and no virtual-memory
/// guard pages (embedded target with limited RAM).
///
/// # Returns
///
/// A configured wasmtime `Engine` targeting the Pulley 32-bit interpreter.
///
/// # Panics
///
/// Panics if the engine configuration fails.
fn create_pulley_engine() -> Engine {
    let mut config = Config::new();
    config.target("pulley32").expect("set pulley32 target");
    config.signals_based_traps(false);
    config.memory_init_cow(false);
    config.memory_reservation(0);
    config.memory_guard_size(0);
    config.memory_reservation_for_growth(0);
    config.guard_before_linear_memory(false);
    config.max_wasm_stack(16384);
    Engine::new(&config).expect("create Pulley engine")
}

/// Encodes the core WASM module as a component and AOT-compiles to Pulley bytecode.
///
/// Reads the core WASM binary (which contains `wit-bindgen` component type
/// metadata), wraps it as a WASM component via `ComponentEncoder`, then
/// AOT-compiles the component to Pulley bytecode via Cranelift.
///
/// # Arguments
///
/// * `out` - Output directory path where `button.cwasm` will be placed.
///
/// # Panics
///
/// Panics if encoding, compilation, or serialization fails.
fn compile_wasm_to_pulley(out: &Path) {
    let wasm_path = "wasm-app/target/wasm32-unknown-unknown/release/wasm_app.wasm";
    let wasm_bytes = std::fs::read(wasm_path).expect("read WASM binary");
    let component_bytes = ComponentEncoder::default()
        .module(&wasm_bytes)
        .expect("set core module")
        .validate(true)
        .encode()
        .expect("encode component");
    let engine = create_pulley_engine();
    let serialized = engine
        .precompile_component(&component_bytes)
        .expect("precompile component");
    std::fs::write(out.join("button.cwasm"), &serialized).expect("write Pulley bytecode");
}

/// Registers file change triggers for incremental rebuilds.
fn print_rerun_triggers() {
    println!("cargo:rerun-if-changed=rp2350.x");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=wasm-app/src/lib.rs");
    println!("cargo:rerun-if-changed=wasm-app/Cargo.toml");
    println!("cargo:rerun-if-changed=wit/world.wit");
}

/// Build script entry point that sets up linker scripts and compiles the WASM app.
fn main() {
    let out = setup_output_dir();
    write_linker_script(&out);
    compile_wasm_app();
    compile_wasm_to_pulley(&out);
    print_rerun_triggers();
}
