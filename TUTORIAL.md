# Tutorial: Line-by-Line Walkthrough

This document is a complete, function-by-function walkthrough of every source
file in the **embedded-wasm-button-rp2350** project. It is designed to be read
from beginning to end as a standalone tutorial — no prior knowledge of the
codebase is assumed.

The project runs a WebAssembly Component Model runtime (Wasmtime with the Pulley
interpreter) directly on the RP2350 bare-metal. A precompiled WASM component
reads a button on GPIO15 and mirrors its state to the onboard LED on GPIO25
through typed WIT interfaces.

We will walk through files in this order:

1. **WIT contract** — the typed interfaces the host provides and the guest implements
2. **platform.rs** — thread-local storage glue for Wasmtime on bare-metal
3. **uart.rs** — UART0 driver for diagnostic output
4. **led.rs** — GPIO output driver for controlling the LED
5. **button.rs** — GPIO input driver for reading the button
6. **main.rs** — firmware entry point that ties everything together
7. **build.rs** — AOT compilation pipeline that cross-compiles WASM to Pulley bytecode
8. **wasm-app/src/lib.rs** — the WASM guest component (application logic)

---

## 1. The WIT Contract (`wit/world.wit`)

Everything starts with the WIT (WebAssembly Interface Type) definition. This
file declares the typed interface contract between the firmware (host) and the
WASM component (guest). The guest calls into the host to interact with hardware;
the host provides the actual GPIO control and timing.

```wit
package embedded:platform;
```

The package declaration scopes all interfaces and worlds under the
`embedded:platform` namespace. Both `wit-bindgen` (guest side) and
`wasmtime::component::bindgen!` (host side) use this to generate the module
hierarchy `embedded::platform::gpio`, `embedded::platform::button`, and
`embedded::platform::timing`.

### The `gpio` Interface

```wit
interface gpio {
    set-high: func(pin: u32);
    set-low: func(pin: u32);
}
```

The `gpio` interface provides two functions for controlling digital output pins.
The `pin` parameter is a hardware GPIO number — the WASM guest passes `25` for
the onboard LED. The host maps this number to the actual HAL pin registered in
`led.rs`. Using a `u32` pin number rather than a typed pin object keeps the WIT
interface hardware-agnostic: the same component could target any board that
implements these host functions.

### The `button` Interface

```wit
interface button {
    is-pressed: func(pin: u32) -> bool;
}
```

The `button` interface provides a single function for reading digital input
pins. The guest passes a pin number (e.g., `15`), and the host returns whether
the button is currently pressed. The return type is a simple `bool` — the
active-low logic (pull-up resistor, ground when pressed) is handled entirely on
the host side in `button.rs`, so the guest only sees a clean "pressed or not"
abstraction.

### The `timing` Interface

```wit
interface timing {
    delay-ms: func(ms: u32);
}
```

The `timing` interface provides a blocking delay. The WASM guest calls
`delay-ms(10)` on each poll cycle to avoid busy-spinning. On the host side this
is implemented with `cortex_m::asm::delay()` using CPU cycle counting at
150 MHz rather than the HAL timer, which can hang on the RP2350.

### The `button-led` World

```wit
world button-led {
    import gpio;
    import button;
    import timing;
    export run: func();
}
```

The world ties everything together. The guest *imports* three interfaces (the
host provides them) and *exports* a single `run` function (the guest provides
it). The world name is `button-led` rather than `button` because WIT has a name
collision when the world shares its name with one of its imported interfaces —
`wit-bindgen` cannot resolve `embedded::platform::button` when the world itself
is also called `button`. The hyphenated name `button-led` becomes `ButtonLed` in
the generated Rust bindings.

---

## 2. Platform Glue (`src/platform.rs`)

This is the simplest file in the project and establishes the bare-metal context.
Wasmtime requires thread-local storage (TLS) symbols even on platforms without
an OS. On this single-threaded RP2350, TLS is a global atomic pointer.

### Module Header

```rust
#![no_std] // (inherited from main.rs — platform.rs is a module, not a crate root)
```

The module-level docstring explains the purpose: provide the minimum TLS symbols
for Wasmtime's `no_std` runtime. The file imports `core::ptr` for the null
pointer constant and `core::sync::atomic::{AtomicPtr, Ordering}` for the
lock-free pointer.

### `TLS_VALUE`

```rust
static TLS_VALUE: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());
```

A single global atomic pointer, initialized to null. Wasmtime's internal
execution engine reads and writes this pointer to track per-thread context.
Since the RP2350 is single-threaded, there is exactly one "thread" and one
pointer — no allocation, no synchronization overhead beyond the atomic itself.
The `Ordering::Relaxed` is sufficient because there is only one core accessing
TLS.

### `wasmtime_tls_get`

```rust
#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_tls_get() -> *mut u8 {
    TLS_VALUE.load(Ordering::Relaxed)
}
```

Returns the current TLS pointer. The `#[unsafe(no_mangle)]` attribute (Rust 2024
syntax) prevents the compiler from mangling the symbol name so that Wasmtime's
internal code can call it by its C name. The `pub extern "C"` makes it callable
with C ABI. This function is never called directly from our Rust code — Wasmtime
calls it internally during WASM execution.

### `wasmtime_tls_set`

```rust
#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_tls_set(ptr: *mut u8) {
    TLS_VALUE.store(ptr, Ordering::Relaxed);
}
```

Stores a new TLS pointer value. Mirrors `wasmtime_tls_get` — Wasmtime calls
this to update the per-thread context pointer. Together, these two functions are
the complete platform abstraction layer that makes Wasmtime run on bare-metal.

---

## 3. UART Driver (`src/uart.rs`)

The UART driver provides diagnostic output over UART0 at 115200 baud. It has
two operating modes: HAL-based functions for normal operation and raw
register-based functions for the panic handler (where HAL abstractions may be
unavailable). This is a shared plug-and-play module — it is byte-for-byte
identical across all repos in the embedded-wasm collection.

### Module Header

```rust
#![allow(dead_code)]
```

The `dead_code` allow is required because this is a shared module. Not every
repo uses every function (e.g., `read_byte` may not be called in this project),
but the module must remain identical across repos.

### Constants and Type Aliases

```rust
const UART0_BASE: u32 = 0x4007_0000;
```

The UART0 peripheral base address, used by the panic handler functions that
write directly to hardware registers. The HAL-based functions do not need this
constant — they use the PAC's typed register interface.

```rust
pub type Uart0 = hal::uart::UartPeripheral<...>;
```

A type alias for the fully configured UART0 peripheral. The HAL uses type-level
state encoding — the generic parameters encode that the UART is `Enabled`, uses
`UART0` (not `UART1`), and is connected to GPIO0 (TX) and GPIO1 (RX) with no
pull resistors. This alias avoids repeating the verbose type throughout the file.

### `UART` Static

```rust
static UART: Mutex<RefCell<Option<Uart0>>> = Mutex::new(RefCell::new(None));
```

The global UART peripheral follows the standard embedded Rust pattern:
`Mutex<RefCell<Option<T>>>`. The `Mutex` provides interrupt-safe access via
`critical_section::with`, the `RefCell` provides interior mutability, and the
`Option` allows late initialization (the UART is `None` until `store_global` is
called during startup).

### `init`

```rust
pub fn init(uart0, resets, clocks, tx_pin, rx_pin) -> Uart0
```

Creates and configures UART0 at 115200 baud. The function accepts only the two
UART pins (GPIO0 and GPIO1) rather than the entire `Pins` struct — this keeps
`uart.rs` decoupled from the rest of the GPIO allocation. The `main.rs` caller
retains ownership of all other pins.

Inside, the function reconfigures the two pins from their default `FunctionNull`
state to `FunctionUart`, constructs the `UartPeripheral`, and enables it with
8N1 (8 data bits, no parity, 1 stop bit) at 115200 baud. The peripheral clock
frequency is passed in so the HAL can compute the correct baud rate divisors.

### `store_global`

```rust
pub fn store_global(uart: Uart0)
```

Stores the initialized UART into the global `UART` static. This is called
exactly once during firmware startup. After this call, `write_msg` and other
HAL-based functions can access the UART through the mutex.

### `write_msg`

```rust
pub fn write_msg(msg: &[u8])
```

Writes a message to UART0 using the HAL peripheral. The function converts bare
`\n` to `\r\n` for proper terminal display. It acquires the global UART through
a critical section, iterates over each byte, and uses `write_full_blocking` for
each byte. The `unwrap` on `as_ref()` panics if called before `store_global` —
this is intentional: calling `write_msg` before UART initialization is a
programming error.

### `read_byte`

```rust
pub fn read_byte() -> u8
```

Reads a single byte from UART0 using blocking I/O. This function spins until a
byte is available in the RX FIFO by using `nb::block!` on the HAL's
`read_raw` method. Although this function is not called in the button project,
it is part of the shared module and must remain present.

### `write_byte`

```rust
pub fn write_byte(byte: u8)
```

Writes a single byte to UART0 using the HAL peripheral. Similar to `write_msg`
but for individual bytes. Used by other modules that need character-at-a-time
output.

### `panic_init`

```rust
pub fn panic_init()
```

Initializes UART0 at 115200 baud using direct register writes. This function
exists for the panic handler, which cannot rely on the HAL being in a consistent
state. It deaserts the UART0 and IO_BANK0 resets, configures GPIO0 and GPIO1
for UART function (function select value 2), and programs the baud rate
divisors.

The baud rate calculation for 115200 at 150 MHz: `150_000_000 / (16 × 115200) =
81.380...`, giving an integer divisor of 81 and a fractional divisor of
`round(0.380 × 64) = 24`. The line control register is configured for 8 data
bits (bits [6:5] = 0b11) and FIFO enable (bit 4). The control register enables
UART (bit 0), TX (bit 8), and RX (bit 9).

Every register address is declared as a local `const` with a docstring inside
the function body — the project requires docstrings on all items including
locals.

### `panic_write_byte`

```rust
pub fn panic_write_byte(byte: u8)
```

Writes a single byte to UART0 using direct register access. Spins until the TX
FIFO has space (UARTFR bit 5 = TXFF clear), then writes the byte to UARTDR.
This is safe to call from the panic handler because it does not depend on any
HAL state.

### `panic_write`

```rust
pub fn panic_write(msg: &[u8])
```

Writes a byte slice to UART0 via `panic_write_byte`, converting `\n` to
`\r\n`. This is the panic handler's equivalent of `write_msg`. The separation
between HAL-based and register-based functions allows diagnostic output in any
firmware state — even if the HAL is corrupted or the heap is exhausted.

---

## 4. LED / GPIO Output Driver (`src/led.rs`)

The LED driver provides output pin control through a type-erased pin map. Pins
are stored by their hardware GPIO number so WASM code can address them by
number. This is a shared plug-and-play module identical to the one in the blinky
project.

### Module Header and Imports

```rust
#![allow(dead_code)]
extern crate alloc;
```

The `dead_code` allow is needed because shared modules may have unused functions
in specific projects. The `extern crate alloc` enables heap-backed collections
(`Box`, `BTreeMap`) — required because the pin map entries are trait objects that
must be heap-allocated.

The imports bring in `Box` for heap allocation, `BTreeMap` for ordered pin
storage, `RefCell` and `Mutex` for the global peripheral pattern, `Infallible`
for infallible GPIO error types, and `OutputPin` from `embedded-hal` for the
trait object.

### `PinBox` Type Alias

```rust
type PinBox = Box<dyn OutputPin<Error = Infallible> + Send>;
```

A type alias that erases the concrete pin type into a trait object. The
`embedded-hal` `OutputPin` trait defines `set_high` and `set_low`. The `Error =
Infallible` bound means GPIO operations on this platform cannot fail. The `Send`
bound is required because the `Mutex` requires its contents to be `Send`.

This type erasure is critical: without it, the module would need to be generic
over the specific HAL pin type (which encodes the GPIO bank, pin number, and
function in the type system), making the module impossible to share across
projects that use different pins.

### `PINS` Static

```rust
static PINS: Mutex<RefCell<BTreeMap<u8, PinBox>>> = Mutex::new(RefCell::new(BTreeMap::new()));
```

The global pin map. A `BTreeMap` keyed by hardware GPIO number maps pin numbers
to trait-object pin drivers. Using `BTreeMap` instead of `HashMap` avoids
pulling in the hasher, which adds code size on `no_std`. The
`Mutex<RefCell<...>>` pattern provides interrupt-safe interior mutability, same
as the UART static.

### `store_pin`

```rust
pub fn store_pin(gpio_num: u8, pin: impl OutputPin<Error = Infallible> + Send + 'static)
```

Registers a GPIO output pin in the global map. The `impl` parameter accepts any
concrete pin type that implements `OutputPin` — the function boxes it into a
`PinBox` and inserts it into the `BTreeMap`. The caller passes the pin number
alongside the pin object so the map knows which GPIO number to associate with
this driver.

In this project, `main.rs` calls `led::store_pin(25, pins.gpio25.into_push_pull_output())`
to register the onboard LED. Adding another LED would be a single additional
`store_pin` call in `main.rs` — zero changes to `led.rs`.

### `set_high`

```rust
pub fn set_high(gpio_num: u8)
```

Looks up the pin by GPIO number and calls `pin.set_high()` on the trait object.
The `expect("pin not registered")` panics if the pin was never registered — this
is a programming error, not a runtime condition. The `let _ =` discards the
`Result<(), Infallible>` return value, which can never be `Err`.

### `set_low`

```rust
pub fn set_low(gpio_num: u8)
```

Identical to `set_high` but calls `pin.set_low()`. Together, these two functions
are the complete host-side implementation behind the WIT `gpio::set-high` and
`gpio::set-low` imports.

---

## 5. Button / GPIO Input Driver (`src/button.rs`)

The button driver is the input counterpart of `led.rs`. It provides GPIO input
pin reading through the same type-erased pin map pattern. Pins are stored by
hardware GPIO number and read through the `InputPin` trait from `embedded-hal`.

### Module Header and Imports

```rust
#![allow(dead_code)]
extern crate alloc;
```

The structure mirrors `led.rs` exactly: `dead_code` for shared module
compatibility, `extern crate alloc` for heap-backed collections. The key
difference is in the trait import: `embedded_hal::digital::InputPin` instead of
`OutputPin`.

### `PinBox` Type Alias

```rust
type PinBox = Box<dyn InputPin<Error = Infallible> + Send>;
```

Same type-erasure pattern as `led.rs`, but for input pins. The `InputPin` trait
provides `is_high()` and `is_low()` methods. The `Error = Infallible` bound
means reading the pin cannot fail on this platform.

### `PINS` Static

```rust
static PINS: Mutex<RefCell<BTreeMap<u8, PinBox>>> = Mutex::new(RefCell::new(BTreeMap::new()));
```

A separate pin map from `led.rs` — input pins and output pins are stored in
different maps in different modules. This prevents accidentally calling
`set_high` on an input pin or `is_pressed` on an output pin.

### `store_pin`

```rust
pub fn store_pin(gpio_num: u8, pin: impl InputPin<Error = Infallible> + Send + 'static)
```

Registers a GPIO input pin. In this project, `main.rs` calls
`button::store_pin(15, pins.gpio15.into_pull_up_input())` to register the button
pin. The `into_pull_up_input()` call configures the internal pull-up resistor on
GPIO15 — when the button is not pressed, the pin reads high; when pressed, it is
grounded and reads low.

### `is_pressed`

```rust
pub fn is_pressed(gpio_num: u8) -> bool
```

Reads the pin state and returns whether the button is pressed. The function uses
`pin.is_low().unwrap_or(false)` — it calls `is_low()` rather than `is_high()`
because the button uses active-low logic with a pull-up resistor. When the
button is pressed, it connects GPIO15 to ground, so the pin reads low.

The `unwrap_or(false)` handles the `Result<bool, Infallible>` — since the error
type is `Infallible`, the `Err` branch can never execute, but `unwrap_or`
avoids an explicit `unwrap()` call and communicates the safe-default intent. This
function is the complete host-side implementation behind the WIT
`button::is-pressed` import.

---

## 6. Firmware Entry Point (`src/main.rs`)

This is the heart of the firmware. It ties together all the hardware drivers,
implements the WIT host traits, sets up the Wasmtime runtime, and executes the
WASM component. Every other file feeds into this one.

### Crate Attributes

```rust
#![no_std]
#![no_main]
```

The `#![no_std]` attribute excludes the standard library — this is bare-metal
firmware with no OS. The `#![no_main]` tells the compiler there is no standard
`main` function signature; the entry point is defined by `#[hal::entry]` (the
Cortex-M runtime).

### Extern Crate and Module Declarations

```rust
extern crate alloc;
```

Enables the `alloc` crate for heap-backed collections (`Vec`, `Box`, `String`).
The actual allocator is set up in `init_heap`.

```rust
mod button;
mod led;
mod platform;
mod uart;
```

Four module declarations pull in the driver files. Each `mod` statement has a
`///` doc comment describing the module's purpose. The `platform` module provides
Wasmtime TLS glue, `uart` provides diagnostic output, `led` provides GPIO output
control, and `button` provides GPIO input reading.

### Imports

The imports bring in:

- `core::panic::PanicInfo` — the panic handler signature type
- `embedded_alloc::LlffHeap as Heap` — the linked-list first-fit heap allocator
- `rp235x_hal as hal` — the RP2350 HAL crate, aliased for brevity
- `wasmtime::component::{Component, HasSelf}` — the Component Model loader and
  a phantom type marker for the linker
- `wasmtime::{Config, Engine, Store}` — the core runtime types

### WIT Binding Generation

```rust
wasmtime::component::bindgen!({
    world: "button-led",
    path: "wit",
});
```

This macro generates host-side Rust types from the WIT definition. It produces:

- A `ButtonLed` struct (from the `button-led` world name) with `add_to_linker`
  and `instantiate` methods
- Trait definitions `embedded::platform::gpio::Host`,
  `embedded::platform::button::Host`, and `embedded::platform::timing::Host`
  that the firmware must implement
- A `call_run` method on the instantiated component

The world name `button-led` becomes `ButtonLed` in PascalCase. If the world were
named `button`, it would collide with the `button` interface — this is a WIT
limitation.

### Constants

```rust
const XOSC_CRYSTAL_FREQ: u32 = 12_000_000;
```

The external crystal oscillator frequency. The Pico 2 uses a 12 MHz crystal.
This value is passed to `init_clocks_and_plls` to configure the PLL to produce
the 150 MHz system clock.

```rust
const HEAP_SIZE: usize = 262_144;
```

The heap size: 256 KiB of the available 512 KiB RAM. Wasmtime needs substantial
heap space for the WASM linear memory, the component instance, and internal data
structures. The remaining 256 KiB is used for the stack and static data.

```rust
const WASM_BINARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/button.cwasm"));
```

The precompiled Pulley bytecode, embedded into the firmware binary at compile
time. The `build.rs` script produces `button.cwasm` and places it in `OUT_DIR`;
`include_bytes!` reads it as a `&[u8]` constant. This constant is consumed by
`Component::deserialize` at runtime.

### Boot Metadata

```rust
#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();
```

The RP2350 Boot ROM looks for an image definition in the `.start_block` section
to determine how to boot the firmware. `secure_exe()` marks this as a secure
ARM executable. The `#[used]` attribute prevents the linker from discarding the
static even though no Rust code references it.

### `HostState`

```rust
struct HostState;
```

The host state struct that Wasmtime's `Store` holds. All hardware access goes
through global state in `led.rs`, `button.rs`, and `uart.rs`, so the host state
carries no fields. The WIT `Host` traits are implemented directly on this
zero-sized struct.

### `impl gpio::Host for HostState`

```rust
fn set_high(&mut self, pin: u32) {
    led::set_high(pin as u8);
    write_gpio_msg(pin as u8, true);
}

fn set_low(&mut self, pin: u32) {
    led::set_low(pin as u8);
    write_gpio_msg(pin as u8, false);
}
```

These two methods implement the WIT `gpio` interface. When the WASM guest calls
`gpio::set_high(25)`, Wasmtime routes the call here. The method delegates to
`led::set_high` for the actual GPIO control and then calls `write_gpio_msg` to
log the state change to UART0 (e.g., `"GPIO25 On\n"`).

### `impl button::Host for HostState`

```rust
fn is_pressed(&mut self, pin: u32) -> bool {
    button::is_pressed(pin as u8)
}
```

This method implements the WIT `button` interface. When the WASM guest calls
`button::is_pressed(15)`, Wasmtime routes the call here. The method delegates
directly to `button::is_pressed`, which reads the GPIO15 pin state and returns
`true` if the button is pressed (active-low). No UART logging is done for button
reads to avoid flooding the output at 10ms polling intervals.

### `impl timing::Host for HostState`

```rust
fn delay_ms(&mut self, ms: u32) {
    cortex_m::asm::delay(ms * 150_000);
}
```

Implements the WIT `timing` interface. The delay uses `cortex_m::asm::delay`
which counts CPU cycles — at 150 MHz, `150_000` cycles is approximately 1 ms.
This is used instead of the HAL `Timer::delay_ms` because the HAL timer can
hang on the RP2350. The multiplication `ms * 150_000` converts milliseconds to
CPU cycles.

### `panic`

```rust
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    uart::panic_init();
    uart::panic_write(b"\n!!! PANIC !!!\n");
    ...
    loop { cortex_m::asm::wfe(); }
}
```

The panic handler provides diagnostic output when the firmware crashes. It
calls `uart::panic_init()` to reinitialize UART0 from scratch using direct
register writes — this works even if the HAL state is corrupted. It then prints
the panic banner, file location, and message. The final `loop` with `wfe`
(wait-for-event) halts the CPU without busy-spinning, reducing power consumption
while the device is crashed.

### `init_heap`

```rust
fn init_heap() {
    use core::mem::MaybeUninit;
    static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
    unsafe { HEAP.init(&raw mut HEAP_MEM as usize, HEAP_SIZE) }
}
```

Initializes the global heap allocator. A `static mut` array of `MaybeUninit<u8>`
provides 256 KiB of backing memory. The `HEAP.init()` call from `embedded-alloc`
sets up the linked-list first-fit allocator over this region. This must be called
before any heap allocations occur — it is the first call in `main`.

The `HEAP_MEM` is declared as a local static inside the function rather than at
module level to keep the implementation detail contained. The `unsafe` is
required by the `embedded-alloc` API.

### `init_clocks`

```rust
fn init_clocks(xosc, clocks, pll_sys, pll_usb, resets, watchdog) -> ClocksManager
```

Initializes the RP2350's system clocks and PLLs from the 12 MHz external crystal
oscillator. The HAL's `init_clocks_and_plls` configures the system PLL to
produce 150 MHz (the default for the RP2350) and the USB PLL to produce 48 MHz.
The function returns a `ClocksManager` which is used to query the peripheral
clock frequency for UART baud rate calculation.

### `write_gpio_msg`

```rust
fn write_gpio_msg(pin: u8, high: bool)
```

Formats and writes a GPIO state change message to UART0. For pin 25 going high,
it outputs `"GPIO25 On\n"`. For pin 25 going low, it outputs `"GPIO25 Off\n"`.
The pin number is formatted by `fmt_u8` into a 3-byte buffer.

### `fmt_u8`

```rust
fn fmt_u8(mut n: u8, buf: &mut [u8; 3]) -> usize
```

Converts a `u8` to decimal ASCII digits. This custom formatter avoids pulling in
the `core::fmt` machinery, which adds significant code size on `no_std`. The
function writes digits into a temporary array in reverse order (extracting the
least significant digit first with `% 10`), then copies them into the output
buffer in the correct order.

### `init_hardware`

```rust
fn init_hardware()
```

Initializes all RP2350 hardware peripherals. This function is the single place
where GPIO pin allocation happens. It takes the PAC peripherals, creates the
watchdog and clocks, initializes SIO, and creates the GPIO pins struct. Then it
distributes pins to each driver:

- GPIO0 and GPIO1 go to `uart::init()` for UART TX/RX
- GPIO15 goes to `button::store_pin(15, pins.gpio15.into_pull_up_input())`  
- GPIO25 goes to `led::store_pin(25, pins.gpio25.into_push_pull_output())`

The `into_pull_up_input()` call on GPIO15 enables the internal pull-up resistor.
When the button is not pressed, the pin reads high. When pressed, the button
connects the pin to ground and it reads low. The `into_push_pull_output()` call
on GPIO25 configures it as a standard digital output for driving the LED.

This design means adding a new GPIO pin to the project is a one-line change in
`init_hardware` — no changes to any shared module.

### `create_engine`

```rust
fn create_engine() -> Engine
```

Creates a Wasmtime engine configured for the Pulley 32-bit interpreter on
bare-metal. Every setting must match the engine configuration in `build.rs`
exactly — `Component::deserialize` validates the configuration embedded in the
`.cwasm` header against the runtime engine. Any mismatch causes a panic.

The key settings:

- `target("pulley32")` — target the Pulley interpreter (not native code)
- `signals_based_traps(false)` — bare-metal has no OS signal handlers
- `memory_init_cow(false)` — no virtual memory copy-on-write support
- `memory_reservation(0)` / `memory_guard_size(0)` — no virtual-memory guard
  pages (embedded target with limited RAM)
- `max_wasm_stack(16384)` — 16 KiB WASM stack limit

### `create_component`

```rust
fn create_component(engine: &Engine) -> Component
```

Deserializes the precompiled Pulley component from the embedded bytes. The
`unsafe` block is required because `Component::deserialize` trusts that the
bytes are a valid serialized Wasmtime component — it does not perform full
validation for performance. This invariant is upheld because the bytes are
produced by our own build script.

### `build_linker`

```rust
fn build_linker(engine: &Engine) -> Linker<HostState>
```

Creates a component linker and registers all WIT interface implementations. The
`ButtonLed::add_to_linker` call (generated by `bindgen!`) connects the
`gpio::Host`, `button::Host`, and `timing::Host` trait implementations on
`HostState` to the linker. The `HasSelf<HostState>` type parameter is a phantom
marker — the closure `|state: &mut HostState| state` simply returns the state
itself (no wrapping or indirection).

### `execute_wasm`

```rust
fn execute_wasm(store, linker, component)
```

Instantiates the WASM component and calls its exported `run` function. The
`ButtonLed::instantiate` call creates a live component instance by linking all
imports to the host implementations. The `call_run` method invokes the guest's
`run` function, which enters the infinite polling loop. Since `run` never
returns (it loops forever reading the button), this function also never returns
during normal operation.

### `run_wasm`

```rust
fn run_wasm() -> !
```

Orchestrates the complete WASM runtime startup. Creates the engine, deserializes
the component, creates a `Store` with `HostState`, builds the linker, and calls
`execute_wasm`. The trailing `loop { cortex_m::asm::wfe(); }` is a safety net
in case `execute_wasm` returns (which should not happen since the guest loops
forever).

### `main`

```rust
#[hal::entry]
fn main() -> ! {
    init_heap();
    init_hardware();
    run_wasm()
}
```

The firmware entry point, marked with `#[hal::entry]` which expands to the
Cortex-M reset handler. The boot sequence is:

1. `init_heap()` — set up the 256 KiB heap allocator (must come first)
2. `init_hardware()` — configure clocks, UART, button, and LED
3. `run_wasm()` — start the Wasmtime runtime (never returns)

---

## 7. Build Script (`build.rs`)

The build script runs on the host machine during `cargo build`. It compiles the
WASM guest application, wraps it as a Component Model component, and
AOT-compiles it to Pulley bytecode so the device does not need to compile WASM
at runtime.

### `setup_output_dir`

```rust
fn setup_output_dir() -> PathBuf
```

Reads the `OUT_DIR` environment variable (set by Cargo) and registers it as a
linker search path. The output directory is where both the linker script
(`memory.x`) and the compiled Pulley bytecode (`button.cwasm`) are placed.

### `write_linker_script`

```rust
fn write_linker_script(out: &Path)
```

Copies the `rp2350.x` memory layout file to the output directory as `memory.x`.
The Cortex-M linker script (`link.x` from `cortex-m-rt`) expects a `memory.x`
file that defines the FLASH and RAM memory regions. The `rp2350.x` file defines
2 MiB of FLASH starting at `0x10000000` and 512 KiB of RAM starting at
`0x20000000`.

### `compile_wasm_app`

```rust
fn compile_wasm_app()
```

Invokes `cargo build` on the `wasm-app` sub-crate targeting
`wasm32-unknown-unknown`. The critical detail is `.env_remove("CARGO_ENCODED_RUSTFLAGS")`
— without this, the parent's RUSTFLAGS (which contain `--nmagic` and `-Tlink.x`
for the ARM target) would leak into the WASM build and cause linker errors for
the WASM target.

### `create_pulley_engine`

```rust
fn create_pulley_engine() -> Engine
```

Creates a Wasmtime engine configured identically to the runtime engine in
`main.rs`. Every `Config` setting must match exactly between build-time and
runtime engines. This engine is used for AOT cross-compilation — it targets
`pulley32` so that Cranelift generates Pulley bytecode instead of native ARM
instructions.

### `compile_wasm_to_pulley`

```rust
fn compile_wasm_to_pulley(out: &Path)
```

The core of the AOT pipeline. This function:

1. Reads the compiled `wasm_app.wasm` core module
2. Wraps it as a Component Model component using `ComponentEncoder` — this reads
   the type metadata that `wit-bindgen` embedded in the core module and produces
   a proper component with typed imports and exports
3. AOT-compiles the component to Pulley bytecode using `engine.precompile_component`
   — this invokes Cranelift to cross-compile WASM to Pulley instructions
4. Writes the serialized bytecode to `button.cwasm`

The `.cwasm` file contains the serialized Pulley bytecode plus a header with
the engine configuration. On the device, `Component::deserialize` validates
this header before loading the bytecode.

### `print_rerun_triggers`

```rust
fn print_rerun_triggers()
```

Registers file change triggers for Cargo's incremental build system. If any of
these files change, Cargo will re-run the build script:

- `rp2350.x` — linker memory layout
- `build.rs` — the build script itself
- `wasm-app/src/lib.rs` — the guest application source
- `wasm-app/Cargo.toml` — the guest application dependencies
- `wit/world.wit` — the WIT interface definition

### `main`

```rust
fn main() {
    let out = setup_output_dir();
    write_linker_script(&out);
    compile_wasm_app();
    compile_wasm_to_pulley(&out);
    print_rerun_triggers();
}
```

Calls each build step in sequence. The order matters: the linker script must be
written before linking starts, and the WASM app must be compiled before it can
be encoded and cross-compiled.

---

## 8. WASM Guest Component (`wasm-app/src/lib.rs`)

The guest component is the application logic that runs inside Wasmtime on the
device. It is compiled to `wasm32-unknown-unknown` and knows nothing about the
RP2350 — all hardware access happens through the WIT imports.

### Crate Attributes

```rust
#![no_std]
```

The WASM guest runs in a constrained environment with no OS. The `no_std`
attribute excludes the standard library. Unlike the firmware, the guest does NOT
need `#![no_main]` because WASM crates are libraries (`crate-type = ["cdylib"]`),
not executables.

### Allocator

```rust
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;
```

The guest needs a heap allocator because the Component Model's canonical ABI
requires a `cabi_realloc` function for transferring data across the WASM
boundary. The `dlmalloc` crate provides a compact allocator suitable for WASM
targets. Without this, the component would fail to instantiate.

### Imports

```rust
use embedded::platform::button;
use embedded::platform::gpio;
use embedded::platform::timing;
```

These imports bring the WIT-generated bindings into scope. Each module
corresponds to a WIT interface: `button` provides `is_pressed`, `gpio` provides
`set_high` and `set_low`, and `timing` provides `delay_ms`. These functions are
not implemented in the guest — they are host imports that Wasmtime routes to the
`HostState` trait implementations in the firmware.

### Binding Generation

```rust
wit_bindgen::generate!({
    world: "button-led",
    path: "../wit",
});
```

This macro generates guest-side Rust bindings from the WIT definition. It
produces the `embedded::platform::gpio`, `embedded::platform::button`, and
`embedded::platform::timing` modules with function stubs, and it produces a
`Guest` trait with a `run` method that the component must implement. It also
generates the `export!` macro used below.

### `ButtonApp` and `export!`

```rust
struct ButtonApp;
export!(ButtonApp);
```

The `ButtonApp` struct is the concrete type that implements the `Guest` trait.
The `export!` macro registers it as the component's implementation — it generates
the glue code that Wasmtime calls when invoking the `run` export.

### `impl Guest for ButtonApp`

```rust
fn run() {
    const BUTTON_PIN: u32 = 15;
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
```

The entire application logic. Two local constants define the GPIO pin numbers —
these are the guest's decision, not the host's. The infinite loop polls the button
every 10 ms. On each iteration:

1. `button::is_pressed(15)` asks the host to read GPIO15
2. If pressed, `gpio::set_high(25)` asks the host to turn on the LED
3. If released, `gpio::set_low(25)` asks the host to turn off the LED
4. `timing::delay_ms(10)` blocks for 10 ms before the next poll

The 10 ms polling interval provides basic debouncing — mechanical buttons
bounce for roughly 5–20 ms, and the 10 ms poll period means a bounce is unlikely
to cause a visible flicker. The WASM guest is completely decoupled from the
hardware; it could be tested on a desktop by providing mock host implementations.

### Panic Handler

```rust
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
```

A minimal panic handler that spins forever. The WASM environment has no UART or
diagnostic output — if the guest panics, Wasmtime traps and the firmware's panic
handler takes over. The `spin_loop` hint tells the CPU to reduce power
consumption while spinning.

---

## Summary: The Complete Execution Flow

1. **Power on** — the RP2350 Boot ROM reads the `IMAGE_DEF` from the
   `.start_block` section and boots the secure ARM executable.

2. **`main()`** — the Cortex-M reset handler calls `main`, which begins the
   three-step initialization sequence.

3. **`init_heap()`** — allocates a 256 KiB static memory region and initializes
   the linked-list first-fit heap allocator.

4. **`init_hardware()`** — configures the 150 MHz system clock from the 12 MHz
   crystal, initializes UART0 at 115200 baud on GPIO0/GPIO1, registers GPIO15
   as a pull-up input in `button.rs`, and registers GPIO25 as a push-pull
   output in `led.rs`.

5. **`create_engine()`** — builds a Wasmtime engine targeting the Pulley 32-bit
   interpreter with bare-metal settings (no signals, no guard pages, 16 KiB
   stack).

6. **`create_component()`** — deserializes the `button.cwasm` Pulley bytecode
   that was AOT-compiled by `build.rs` and embedded via `include_bytes!`.

7. **`build_linker()`** — registers the `gpio::Host`, `button::Host`, and
   `timing::Host` implementations on `HostState` with the component linker.

8. **`execute_wasm()`** — instantiates the WASM component, linking its three
   imports to the host implementations, and calls the exported `run` function.

9. **`run()` (guest)** — enters an infinite loop that polls GPIO15 every 10 ms
   and mirrors the button state to GPIO25: pressed -> LED on, released ->
   LED off. Each poll cycle crosses the WASM/host boundary three times
   (`is_pressed` -> `set_high`/`set_low` -> `delay_ms`).
