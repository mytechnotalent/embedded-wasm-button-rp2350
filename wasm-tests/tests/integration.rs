//! SPDX-License-Identifier: MIT
//!
//! Copyright (c) 2026 Kevin Thomas
//!
//! # Integration Tests for Wasm Button Component
//!
//! Validates that the compiled Wasm component loads correctly through the
//! Component Model, implements the expected WIT interfaces
//! (`embedded:platform/gpio`, `embedded:platform/button`, and
//! `embedded:platform/timing`), exports the `run` function, and polls
//! the button to control the LED with the correct pin targeting and delay
//! values.

use wasmtime::component::{Component, HasSelf};
use wasmtime::{Config, Engine, Store};

wasmtime::component::bindgen!({
    world: "button-led",
    path: "../wit",
});

/// Compiled Wasm button component embedded at build time.
const WASM_BINARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/button.wasm"));

/// Represents a single host function call recorded during Wasm execution.
#[derive(Debug, PartialEq)]
enum HostCall {
    /// The `gpio.set-high` WIT function was called with the given pin.
    GpioSetHigh(u32),
    /// The `gpio.set-low` WIT function was called with the given pin.
    GpioSetLow(u32),
    /// The `button.is-pressed` WIT function was called with the given pin.
    ButtonIsPressed(u32),
    /// The `timing.delay-ms` WIT function was called with the given value.
    DelayMs(u32),
}

/// Host state that records all function calls and provides simulated button input.
struct TestHostState {
    /// Ordered log of every host function call.
    calls: Vec<HostCall>,
    /// Simulated button pressed state returned by `button.is-pressed`.
    button_pressed: bool,
}

impl embedded::platform::gpio::Host for TestHostState {
    /// Records a `set-high` call with the given pin number.
    ///
    /// # Arguments
    ///
    /// * `pin` - GPIO pin number passed by the Wasm guest.
    fn set_high(&mut self, pin: u32) {
        self.calls.push(HostCall::GpioSetHigh(pin));
    }

    /// Records a `set-low` call with the given pin number.
    ///
    /// # Arguments
    ///
    /// * `pin` - GPIO pin number passed by the Wasm guest.
    fn set_low(&mut self, pin: u32) {
        self.calls.push(HostCall::GpioSetLow(pin));
    }
}

impl embedded::platform::button::Host for TestHostState {
    /// Records an `is-pressed` call and returns the simulated button state.
    ///
    /// # Arguments
    ///
    /// * `pin` - GPIO pin number passed by the Wasm guest.
    ///
    /// # Returns
    ///
    /// The simulated `button_pressed` value from the test host state.
    fn is_pressed(&mut self, pin: u32) -> bool {
        self.calls.push(HostCall::ButtonIsPressed(pin));
        self.button_pressed
    }
}

impl embedded::platform::timing::Host for TestHostState {
    /// Records a `delay-ms` call with the given duration.
    ///
    /// # Arguments
    ///
    /// * `ms` - Delay duration in milliseconds passed by the Wasm guest.
    fn delay_ms(&mut self, ms: u32) {
        self.calls.push(HostCall::DelayMs(ms));
    }
}

/// Creates a wasmtime engine with fuel metering enabled.
///
/// # Returns
///
/// A wasmtime `Engine` with fuel consumption enabled.
///
/// # Panics
///
/// Panics if engine creation fails.
fn create_fuel_engine() -> Engine {
    let mut config = Config::default();
    config.consume_fuel(true);
    Engine::new(&config).expect("create fuel engine")
}

/// Creates a default wasmtime engine without fuel metering.
///
/// # Returns
///
/// A wasmtime `Engine` with default configuration.
fn create_default_engine() -> Engine {
    Engine::default()
}

/// Compiles the embedded Wasm binary into a wasmtime component.
///
/// # Arguments
///
/// * `engine` - The wasmtime engine to compile with.
///
/// # Returns
///
/// The compiled Wasm `Component`.
///
/// # Panics
///
/// Panics if the Wasm binary is invalid.
fn compile_component(engine: &Engine) -> Component {
    Component::new(engine, WASM_BINARY).expect("valid Wasm component")
}

/// Builds a fully configured test linker with all WIT interfaces registered.
///
/// # Arguments
///
/// * `engine` - The wasmtime engine to associate the linker with.
///
/// # Returns
///
/// A component `Linker` with `gpio::Host`, `button::Host`, and `timing::Host` registered.
///
/// # Panics
///
/// Panics if WIT interface registration fails.
fn build_test_linker(engine: &Engine) -> wasmtime::component::Linker<TestHostState> {
    let mut linker = wasmtime::component::Linker::new(engine);
    ButtonLed::add_to_linker::<TestHostState, HasSelf<TestHostState>>(
        &mut linker,
        |state: &mut TestHostState| state,
    )
    .expect("register WIT interfaces");
    linker
}

/// Creates a store with a test host state and the given fuel budget.
///
/// # Arguments
///
/// * `engine` - The wasmtime engine to create the store for.
/// * `fuel` - The amount of fuel to allocate for execution.
/// * `button_pressed` - Simulated button state for the test.
///
/// # Returns
///
/// A `Store` containing a `TestHostState` with the fuel budget set.
///
/// # Panics
///
/// Panics if fuel allocation fails.
fn create_fueled_store(engine: &Engine, fuel: u64, button_pressed: bool) -> Store<TestHostState> {
    let state = TestHostState {
        calls: Vec::new(),
        button_pressed,
    };
    let mut store = Store::new(engine, state);
    store.set_fuel(fuel).expect("set fuel");
    store
}

/// Runs the Wasm `run` function until fuel is exhausted.
///
/// # Arguments
///
/// * `store` - The wasmtime store with fuel and host state.
/// * `linker` - The component linker with WIT interfaces registered.
/// * `component` - The compiled Wasm component.
///
/// # Panics
///
/// Panics if component instantiation fails.
fn run_until_out_of_fuel(
    store: &mut Store<TestHostState>,
    linker: &wasmtime::component::Linker<TestHostState>,
    component: &Component,
) {
    let instance =
        ButtonLed::instantiate(&mut *store, component, linker).expect("instantiate component");
    let _ = instance.call_run(&mut *store);
}

/// Verifies that the Wasm component binary loads without error.
///
/// # Panics
///
/// Panics if the Wasm component binary fails to compile.
#[test]
fn test_wasm_component_loads() {
    let engine = create_default_engine();
    let _component = compile_component(&engine);
}

/// Verifies that the component instantiates and exports the `run` function.
///
/// # Panics
///
/// Panics if the component fails to instantiate.
#[test]
fn test_wasm_exports_run_function() {
    let engine = create_default_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let state = TestHostState {
        calls: Vec::new(),
        button_pressed: false,
    };
    let mut store = Store::new(&engine, state);
    let instance = ButtonLed::instantiate(&mut store, &component, &linker);
    assert!(
        instance.is_ok(),
        "component must instantiate with run export"
    );
}

/// Verifies that the component imports `gpio`, `button`, and `timing` interfaces.
///
/// # Panics
///
/// Panics if a required interface import is missing.
#[test]
fn test_wasm_imports_match_expected() {
    let engine = create_default_engine();
    let component = compile_component(&engine);
    let ty = component.component_type();
    let import_names: Vec<_> = ty
        .imports(&engine)
        .map(|(name, _)| name.to_string())
        .collect();
    assert!(
        import_names.iter().any(|n| n.contains("gpio")),
        "missing gpio interface"
    );
    assert!(
        import_names.iter().any(|n| n.contains("button")),
        "missing button interface"
    );
    assert!(
        import_names.iter().any(|n| n.contains("timing")),
        "missing timing interface"
    );
}

/// Verifies that all imports originate from the `embedded:platform` package.
///
/// # Panics
///
/// Panics if any import is not from the `embedded:platform` package.
#[test]
fn test_all_imports_from_embedded_platform() {
    let engine = create_default_engine();
    let component = compile_component(&engine);
    let ty = component.component_type();
    for (name, _) in ty.imports(&engine) {
        assert!(
            name.starts_with("embedded:platform/"),
            "import '{name}' must be from embedded:platform"
        );
    }
}

/// Verifies that the component has exactly 3 interface imports.
///
/// # Panics
///
/// Panics if the import count is not exactly 3.
#[test]
fn test_import_count_is_exactly_three() {
    let engine = create_default_engine();
    let component = compile_component(&engine);
    let ty = component.component_type();
    let count = ty.imports(&engine).count();
    assert_eq!(
        count, 3,
        "component must have exactly 3 interface imports, got {count}"
    );
}

/// Verifies that the component has exactly 1 export (`run`).
///
/// # Panics
///
/// Panics if the export count is not exactly 1.
#[test]
fn test_component_exports_exactly_one() {
    let engine = create_default_engine();
    let component = compile_component(&engine);
    let ty = component.component_type();
    let count = ty.exports(&engine).count();
    assert_eq!(
        count, 1,
        "component must have exactly 1 export (run), got {count}"
    );
}

/// Verifies that the Wasm component binary is under 16 KB.
///
/// # Panics
///
/// Panics if the component binary is 16 KB or larger.
#[test]
fn test_wasm_component_size_under_16kb() {
    assert!(
        WASM_BINARY.len() < 16_384,
        "Wasm component must be under 16 KB, got {} bytes",
        WASM_BINARY.len()
    );
}

/// Verifies that the `embedded:platform/gpio` import is present.
///
/// # Panics
///
/// Panics if the `embedded:platform/gpio` import is missing.
#[test]
fn test_gpio_import_name_is_correct() {
    let engine = create_default_engine();
    let component = compile_component(&engine);
    let ty = component.component_type();
    let import_names: Vec<_> = ty
        .imports(&engine)
        .map(|(name, _)| name.to_string())
        .collect();
    assert!(
        import_names.iter().any(|n| n == "embedded:platform/gpio"),
        "missing embedded:platform/gpio import, got {import_names:?}"
    );
}

/// Verifies that the `embedded:platform/button` import is present.
///
/// # Panics
///
/// Panics if the `embedded:platform/button` import is missing.
#[test]
fn test_button_import_name_is_correct() {
    let engine = create_default_engine();
    let component = compile_component(&engine);
    let ty = component.component_type();
    let import_names: Vec<_> = ty
        .imports(&engine)
        .map(|(name, _)| name.to_string())
        .collect();
    assert!(
        import_names.iter().any(|n| n == "embedded:platform/button"),
        "missing embedded:platform/button import, got {import_names:?}"
    );
}

/// Verifies that the `embedded:platform/timing` import is present.
///
/// # Panics
///
/// Panics if the `embedded:platform/timing` import is missing.
#[test]
fn test_timing_import_name_is_correct() {
    let engine = create_default_engine();
    let component = compile_component(&engine);
    let ty = component.component_type();
    let import_names: Vec<_> = ty
        .imports(&engine)
        .map(|(name, _)| name.to_string())
        .collect();
    assert!(
        import_names.iter().any(|n| n == "embedded:platform/timing"),
        "missing embedded:platform/timing import, got {import_names:?}"
    );
}

/// Verifies that the first host call is always a button read on pin 15.
///
/// # Panics
///
/// Panics if the first call is not `ButtonIsPressed(15)`.
#[test]
fn test_first_call_is_button_read() {
    let engine = create_fuel_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let mut store = create_fueled_store(&engine, 100_000, false);
    run_until_out_of_fuel(&mut store, &linker, &component);
    let calls = &store.data().calls;
    assert!(!calls.is_empty(), "must have at least one call");
    assert_eq!(
        calls[0],
        HostCall::ButtonIsPressed(15),
        "first call must be is_pressed(15)"
    );
}

/// Verifies that a pressed button results in `set_high(25)` calls.
///
/// # Panics
///
/// Panics if no `GpioSetHigh(25)` call is recorded when button is pressed.
#[test]
fn test_button_pressed_sets_led_high() {
    let engine = create_fuel_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let mut store = create_fueled_store(&engine, 100_000, true);
    run_until_out_of_fuel(&mut store, &linker, &component);
    let calls = &store.data().calls;
    let has_high = calls.iter().any(|c| matches!(c, HostCall::GpioSetHigh(25)));
    assert!(has_high, "pressed button must trigger set_high(25)");
}

/// Verifies that a released button results in `set_low(25)` calls.
///
/// # Panics
///
/// Panics if no `GpioSetLow(25)` call is recorded when button is released.
#[test]
fn test_button_released_sets_led_low() {
    let engine = create_fuel_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let mut store = create_fueled_store(&engine, 100_000, false);
    run_until_out_of_fuel(&mut store, &linker, &component);
    let calls = &store.data().calls;
    let has_low = calls.iter().any(|c| matches!(c, HostCall::GpioSetLow(25)));
    assert!(has_low, "released button must trigger set_low(25)");
}

/// Verifies that the polling pattern repeats: button read, GPIO set, delay.
///
/// # Panics
///
/// Panics if the polling cycle does not follow the expected 3-call pattern.
#[test]
fn test_polling_pattern_repeats() {
    let engine = create_fuel_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let mut store = create_fueled_store(&engine, 500_000, true);
    run_until_out_of_fuel(&mut store, &linker, &component);
    let calls = &store.data().calls;
    assert!(calls.len() >= 6, "need at least two full polling cycles");
    for chunk in calls.chunks_exact(3) {
        assert_eq!(chunk[0], HostCall::ButtonIsPressed(15));
        assert_eq!(chunk[1], HostCall::GpioSetHigh(25));
        assert_eq!(chunk[2], HostCall::DelayMs(10));
    }
}

/// Verifies that all delay calls use the expected 10ms value.
///
/// # Panics
///
/// Panics if any delay call does not use 10ms.
#[test]
fn test_delay_value_is_10ms() {
    let engine = create_fuel_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let mut store = create_fueled_store(&engine, 100_000, false);
    run_until_out_of_fuel(&mut store, &linker, &component);
    let calls = &store.data().calls;
    for call in calls {
        if let HostCall::DelayMs(ms) = call {
            assert_eq!(*ms, 10, "delay must always be 10ms");
        }
    }
}

/// Verifies that no unknown host call variants are recorded.
///
/// # Panics
///
/// Panics if an unrecognized host call variant is encountered.
#[test]
fn test_no_unexpected_host_calls() {
    let engine = create_fuel_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let mut store = create_fueled_store(&engine, 100_000, true);
    run_until_out_of_fuel(&mut store, &linker, &component);
    let calls = &store.data().calls;
    for call in calls {
        match call {
            HostCall::GpioSetHigh(_)
            | HostCall::GpioSetLow(_)
            | HostCall::ButtonIsPressed(_)
            | HostCall::DelayMs(_) => {}
        }
    }
}

/// Verifies that fuel metering halts the infinite polling loop.
///
/// # Panics
///
/// Panics if fuel retrieval fails or fuel is not nearly exhausted.
#[test]
fn test_fuel_metering_halts_infinite_loop() {
    let engine = create_fuel_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let mut store = create_fueled_store(&engine, 1_000, false);
    run_until_out_of_fuel(&mut store, &linker, &component);
    let remaining = store.get_fuel().expect("get fuel");
    assert!(
        remaining < 10,
        "fuel must be nearly exhausted, got {remaining}"
    );
}

/// Verifies that all GPIO calls target pin 25 exclusively.
///
/// # Panics
///
/// Panics if any GPIO call targets a pin other than 25.
#[test]
fn test_gpio_pin_is_always_25() {
    let engine = create_fuel_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let mut store = create_fueled_store(&engine, 500_000, true);
    run_until_out_of_fuel(&mut store, &linker, &component);
    let calls = &store.data().calls;
    for call in calls {
        match call {
            HostCall::GpioSetHigh(pin) | HostCall::GpioSetLow(pin) => {
                assert_eq!(*pin, 25, "GPIO pin must always be 25");
            }
            _ => {}
        }
    }
}

/// Verifies that all button reads target pin 15 exclusively.
///
/// # Panics
///
/// Panics if any button read targets a pin other than 15.
#[test]
fn test_button_pin_is_always_15() {
    let engine = create_fuel_engine();
    let component = compile_component(&engine);
    let linker = build_test_linker(&engine);
    let mut store = create_fueled_store(&engine, 500_000, false);
    run_until_out_of_fuel(&mut store, &linker, &component);
    let calls = &store.data().calls;
    for call in calls {
        if let HostCall::ButtonIsPressed(pin) = call {
            assert_eq!(*pin, 15, "button pin must always be 15");
        }
    }
}
