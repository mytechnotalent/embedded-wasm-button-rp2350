//! SPDX-License-Identifier: MIT
//!
//! Copyright (c) 2026 Kevin Thomas
//!
//! # Wasm Button Firmware for RP2350 (Pico 2)
//!
//! This firmware runs a WebAssembly Component Model runtime on the RP2350
//! bare-metal using Wasmtime with the Pulley interpreter. A precompiled Wasm
//! component reads GPIO15 (button) and controls GPIO25 (onboard LED) through
//! typed WIT interfaces (`embedded:platform/gpio`, `embedded:platform/button`,
//! and `embedded:platform/timing`).

#![no_std]
#![no_main]

// Enable the global allocator for heap-backed collections.
extern crate alloc;

/// Button GPIO input driver.
mod button;
/// LED GPIO control abstraction.
mod led;
/// WIT host-import implementations for GPIO, button, and timing.
mod platform;
/// UART peripheral setup and I/O helpers.
mod uart;

/// Panic handler signature type.
use core::panic::PanicInfo;
/// Linked-list first-fit heap allocator.
use embedded_alloc::LlffHeap as Heap;
/// RP2350 HAL shorthand.
use rp235x_hal as hal;
/// Component Model loader and linker traits.
use wasmtime::component::{Component, HasSelf};
/// Wasmtime runtime core types.
use wasmtime::{Config, Engine, Store};

// Generate host-side bindings for the `button-led` WIT world.
wasmtime::component::bindgen!({
    world: "button-led",
    path: "wit",
});

/// Global heap allocator backed by a statically allocated memory region.
///
/// Uses the linked-list first-fit allocation strategy from `embedded-alloc`.
#[global_allocator]
static HEAP: Heap = Heap::empty();

/// External crystal oscillator frequency in Hz.
const XOSC_CRYSTAL_FREQ: u32 = 12_000_000;

/// Heap size in bytes (256 KiB of the available 512 KiB RAM).
const HEAP_SIZE: usize = 262_144;

/// Precompiled Pulley bytecode for the Wasm component, embedded at build time.
const WASM_BINARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/button.cwasm"));

/// RP2350 boot metadata placed in the `.start_block` section for the Boot ROM.
#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

/// Host state providing WIT interface implementations via the wasmtime store.
///
/// All hardware access goes through global state (led.rs, button.rs, uart.rs),
/// so the host state carries no fields. The WIT `Host` traits are implemented
/// directly on this struct.
struct HostState;

impl embedded::platform::gpio::Host for HostState {
    /// Sets the specified GPIO pin high and logs the state change to UART0.
    ///
    /// # Arguments
    ///
    /// * `pin` - Hardware GPIO pin number.
    fn set_high(&mut self, pin: u32) {
        led::set_high(pin as u8);
        write_gpio_msg(pin as u8, true);
    }

    /// Sets the specified GPIO pin low and logs the state change to UART0.
    ///
    /// # Arguments
    ///
    /// * `pin` - Hardware GPIO pin number.
    fn set_low(&mut self, pin: u32) {
        led::set_low(pin as u8);
        write_gpio_msg(pin as u8, false);
    }
}

impl embedded::platform::button::Host for HostState {
    /// Returns whether the specified GPIO input pin is pressed.
    ///
    /// # Arguments
    ///
    /// * `pin` - Hardware GPIO pin number.
    ///
    /// # Returns
    ///
    /// `true` if the button is pressed (active-low), `false` otherwise.
    fn is_pressed(&mut self, pin: u32) -> bool {
        button::is_pressed(pin as u8)
    }
}

impl embedded::platform::timing::Host for HostState {
    /// Blocks execution for the specified duration via CPU cycle counting.
    ///
    /// # Arguments
    ///
    /// * `ms` - Delay duration in milliseconds.
    fn delay_ms(&mut self, ms: u32) {
        cortex_m::asm::delay(ms * 150_000);
    }
}

/// Panic handler that outputs a diagnostic message over UART0.
///
/// Initializes UART0 from scratch (in case it was never set up) and
/// writes the panic location and message to UART0, then halts.
///
/// # Arguments
///
/// * `info` - Panic information containing the location and message.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    uart::panic_init();
    uart::panic_write(b"\n!!! PANIC !!!\n");
    if let Some(location) = info.location() {
        uart::panic_write(b"Location: ");
        uart::panic_write(location.file().as_bytes());
        uart::panic_write(b"\n");
    }
    if let Some(msg) = info.message().as_str() {
        uart::panic_write(b"Message: ");
        uart::panic_write(msg.as_bytes());
        uart::panic_write(b"\n");
    }
    loop {
        cortex_m::asm::wfe();
    }
}

/// Initializes the global heap allocator from a static memory region.
///
/// # Safety
///
/// Must be called exactly once before any heap allocations occur.
/// Uses `unsafe` to initialize the allocator with a raw pointer to static memory.
fn init_heap() {
    use core::mem::MaybeUninit;
    /// Static memory region backing the global heap allocator.
    static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
    unsafe { HEAP.init(&raw mut HEAP_MEM as usize, HEAP_SIZE) }
}

/// Initializes system clocks and PLLs from the external crystal oscillator.
///
/// # Arguments
///
/// * `xosc` - External oscillator peripheral.
/// * `clocks` - Clocks peripheral.
/// * `pll_sys` - System PLL peripheral.
/// * `pll_usb` - USB PLL peripheral.
/// * `resets` - Resets peripheral for subsystem reset control.
/// * `watchdog` - Watchdog timer used as the clock reference.
///
/// # Returns
///
/// The configured clocks manager for peripheral clock access.
///
/// # Panics
///
/// Panics if clock initialization fails.
fn init_clocks(
    xosc: hal::pac::XOSC,
    clocks: hal::pac::CLOCKS,
    pll_sys: hal::pac::PLL_SYS,
    pll_usb: hal::pac::PLL_USB,
    resets: &mut hal::pac::RESETS,
    watchdog: &mut hal::Watchdog,
) -> hal::clocks::ClocksManager {
    hal::clocks::init_clocks_and_plls(
        XOSC_CRYSTAL_FREQ,
        xosc,
        clocks,
        pll_sys,
        pll_usb,
        resets,
        watchdog,
    )
    .ok()
    .unwrap()
}

/// Writes a GPIO state change message to UART0 (e.g., "GPIO25 On\n").
///
/// # Arguments
///
/// * `pin` - GPIO pin number.
/// * `high` - `true` for On, `false` for Off.
fn write_gpio_msg(pin: u8, high: bool) {
    uart::write_msg(b"GPIO");
    let mut buf = [0u8; 3];
    let len = fmt_u8(pin, &mut buf);
    uart::write_msg(&buf[..len]);
    if high {
        uart::write_msg(b" On\n");
    } else {
        uart::write_msg(b" Off\n");
    }
}

/// Formats a `u8` as decimal digits into the provided buffer.
///
/// # Arguments
///
/// * `n` - The number to format.
/// * `buf` - Output buffer (must be at least 3 bytes).
///
/// # Returns
///
/// The number of digits written to `buf`.
fn fmt_u8(mut n: u8, buf: &mut [u8; 3]) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut i = 0;
    let mut tmp = [0u8; 3];
    while n > 0 {
        tmp[i] = b'0' + (n % 10);
        n /= 10;
        i += 1;
    }
    for j in 0..i {
        buf[j] = tmp[i - 1 - j];
    }
    i
}

/// Initializes all RP2350 hardware peripherals.
///
/// Sets up the watchdog, clocks, SIO, and GPIO pins. Passes only GPIO0
/// (TX) and GPIO1 (RX) to `uart::init()`, keeping all other pins under
/// `main.rs` control. Configures GPIO15 as a pull-up input for the button
/// and GPIO25 as a push-pull output for the onboard LED.
///
/// # Panics
///
/// Panics if the hardware peripherals have already been taken.
fn init_hardware() {
    let mut pac = hal::pac::Peripherals::take().unwrap();
    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);
    let clocks = init_clocks(
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    );
    let sio = hal::Sio::new(pac.SIO);
    let pins = hal::gpio::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );
    uart::store_global(uart::init(
        pac.UART0,
        &mut pac.RESETS,
        &clocks,
        pins.gpio0,
        pins.gpio1,
    ));
    button::store_pin(15, pins.gpio15.into_pull_up_input());
    led::store_pin(25, pins.gpio25.into_push_pull_output());
}

/// Creates a wasmtime engine configured for Pulley on bare-metal.
///
/// Explicitly targets `pulley32` to match the AOT cross-compilation in
/// `build.rs`. All settings must be identical between build-time and
/// runtime engines or `Component::deserialize` will fail. OS-dependent
/// features are disabled and memory limits are tuned for the RP2350's
/// 512 KiB RAM.
///
/// # Returns
///
/// A configured wasmtime `Engine` targeting the Pulley 32-bit interpreter.
///
/// # Panics
///
/// Panics if the engine configuration fails.
fn create_engine() -> Engine {
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

/// Deserializes the precompiled Pulley component from embedded bytes.
///
/// # Safety
///
/// Uses `unsafe` to call `Component::deserialize` which requires that the
/// embedded bytes are a valid serialized wasmtime component. This invariant
/// is upheld because the bytes are produced by our build script.
///
/// # Arguments
///
/// * `engine` - Engine with matching Pulley configuration.
///
/// # Returns
///
/// The deserialized wasmtime `Component`.
///
/// # Panics
///
/// Panics if the embedded Pulley bytecode is invalid.
fn create_component(engine: &Engine) -> Component {
    unsafe { Component::deserialize(engine, WASM_BINARY) }.expect("valid Pulley component")
}

/// Builds the component linker with all WIT interface bindings registered.
///
/// Uses the `bindgen!`-generated `ButtonLed::add_to_linker` to register
/// host implementations for `embedded:platform/gpio`,
/// `embedded:platform/button`, and `embedded:platform/timing`.
///
/// # Arguments
///
/// * `engine` - Wasm engine that the linker is associated with.
///
/// # Returns
///
/// A configured component `Linker` with all WIT interfaces registered.
///
/// # Panics
///
/// Panics if any interface fails to register.
fn build_linker(engine: &Engine) -> wasmtime::component::Linker<HostState> {
    let mut linker = wasmtime::component::Linker::new(engine);
    ButtonLed::add_to_linker::<HostState, HasSelf<HostState>>(
        &mut linker,
        |state: &mut HostState| state,
    )
    .expect("register WIT interfaces");
    linker
}

/// Instantiates the Wasm component and executes the exported `run` function.
///
/// # Arguments
///
/// * `store` - Wasm store holding the host state.
/// * `linker` - Component linker with WIT interfaces registered.
/// * `component` - Precompiled Wasm component to instantiate.
///
/// # Panics
///
/// Panics if instantiation fails or the `run` export is not found.
fn execute_wasm(
    store: &mut Store<HostState>,
    linker: &wasmtime::component::Linker<HostState>,
    component: &Component,
) {
    let instance =
        ButtonLed::instantiate(&mut *store, component, linker).expect("instantiate component");
    instance.call_run(&mut *store).expect("execute run");
}

/// Loads and runs the Wasm button component.
fn run_wasm() -> ! {
    let engine = create_engine();
    let component = create_component(&engine);
    let mut store = Store::new(&engine, HostState);
    let linker = build_linker(&engine);
    execute_wasm(&mut store, &linker, &component);
    loop {
        cortex_m::asm::wfe();
    }
}

/// Firmware entry point that initializes hardware and runs the Wasm button.
#[hal::entry]
fn main() -> ! {
    init_heap();
    init_hardware();
    run_wasm()
}
