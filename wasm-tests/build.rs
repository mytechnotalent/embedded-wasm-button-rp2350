//! SPDX-License-Identifier: MIT
//!
//! Copyright (c) 2026 Kevin Thomas
//!
//! # Build Script for wasm-tests Crate
//!
//! Compiles the WASM button application, encodes it as a WASM component,
//! and places it in `OUT_DIR` for integration tests via `include_bytes!`.

use std::process::Command;
use wit_component::ComponentEncoder;

/// Compiles the WASM button application for the `wasm32-unknown-unknown` target.
///
/// # Panics
///
/// Panics if the cargo build command fails to execute or returns a non-zero exit code.
fn compile_wasm_app() {
    let status = Command::new("cargo")
        .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
        .current_dir("../wasm-app")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .expect("failed to build WASM app");
    assert!(status.success(), "WASM app compilation failed");
}

/// Encodes the core WASM module as a component and writes to `OUT_DIR`.
///
/// Reads the core WASM binary (which contains `wit-bindgen` component type
/// metadata), wraps it as a WASM component via `ComponentEncoder`, and
/// writes the component binary for `include_bytes!`.
///
/// # Panics
///
/// Panics if encoding fails.
fn encode_component() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let core_wasm =
        std::fs::read("../wasm-app/target/wasm32-unknown-unknown/release/wasm_app.wasm")
            .expect("read core WASM binary");
    let component = ComponentEncoder::default()
        .module(&core_wasm)
        .expect("set core module")
        .validate(true)
        .encode()
        .expect("encode component");
    std::fs::write(format!("{out_dir}/button.wasm"), &component).expect("write component");
}

/// Registers file change triggers for incremental test rebuilds.
fn print_rerun_triggers() {
    println!("cargo:rerun-if-changed=../wasm-app/src/lib.rs");
    println!("cargo:rerun-if-changed=../wasm-app/Cargo.toml");
    println!("cargo:rerun-if-changed=../wit/world.wit");
    println!("cargo:rerun-if-changed=build.rs");
}

/// Build script entry point that compiles the WASM app for testing.
fn main() {
    compile_wasm_app();
    encode_component();
    print_rerun_triggers();
}
