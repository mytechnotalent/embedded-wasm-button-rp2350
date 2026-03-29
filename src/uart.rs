//! SPDX-License-Identifier: MIT
//!
//! Copyright (c) 2026 Kevin Thomas
//!
//! # UART0 Driver for RP2350 (Pico 2)
//!
//! Provides HAL-based UART0 initialization and shared UART0 access via
//! `critical_section::Mutex`. Also includes raw register-based UART output
//! for use in the panic handler where HAL abstractions are unavailable.

#![allow(dead_code)]

/// Interior mutability for the UART peripheral.
use core::cell::RefCell;
/// Interrupt-safe mutex for bare-metal concurrency.
use critical_section::Mutex;
/// Typed frequency value for clock configuration.
use fugit::HertzU32;
/// Trait for reading peripheral clock frequencies.
use hal::Clock;
/// Blocking adapter for non-blocking embedded-hal I/O.
use nb::block;
/// RP2350 HAL shorthand.
use rp235x_hal as hal;

/// UART0 base address for direct register access in the panic handler.
const UART0_BASE: u32 = 0x4007_0000;

/// Type alias for the configured UART0 peripheral.
pub type Uart0 = hal::uart::UartPeripheral<
    hal::uart::Enabled,
    hal::pac::UART0,
    (
        hal::gpio::Pin<hal::gpio::bank0::Gpio0, hal::gpio::FunctionUart, hal::gpio::PullNone>,
        hal::gpio::Pin<hal::gpio::bank0::Gpio1, hal::gpio::FunctionUart, hal::gpio::PullNone>,
    ),
>;

/// Global UART peripheral behind a critical-section mutex for safe shared access.
static UART: Mutex<RefCell<Option<Uart0>>> = Mutex::new(RefCell::new(None));

/// Creates and configures UART0 at 115200 baud with GPIO0 (TX) and GPIO1 (RX).
///
/// Accepts only the two UART pins so callers retain ownership of all other
/// GPIO pins. This keeps `uart.rs` plug-and-play across projects regardless
/// of which extra pins each project uses.
///
/// # Arguments
///
/// * `uart0` - UART0 peripheral from the PAC.
/// * `resets` - Resets peripheral for UART reset control.
/// * `clocks` - Clocks manager for peripheral clock frequency.
/// * `tx_pin` - GPIO0 pin (will be reconfigured for UART TX).
/// * `rx_pin` - GPIO1 pin (will be reconfigured for UART RX).
///
/// # Returns
///
/// The configured UART0 peripheral.
///
/// # Panics
///
/// Panics if UART configuration fails.
pub fn init(
    uart0: hal::pac::UART0,
    resets: &mut hal::pac::RESETS,
    clocks: &hal::clocks::ClocksManager,
    tx_pin: hal::gpio::Pin<hal::gpio::bank0::Gpio0, hal::gpio::FunctionNull, hal::gpio::PullDown>,
    rx_pin: hal::gpio::Pin<hal::gpio::bank0::Gpio1, hal::gpio::FunctionNull, hal::gpio::PullDown>,
) -> Uart0 {
    let uart_pins = (
        tx_pin.reconfigure::<hal::gpio::FunctionUart, hal::gpio::PullNone>(),
        rx_pin.reconfigure::<hal::gpio::FunctionUart, hal::gpio::PullNone>(),
    );
    hal::uart::UartPeripheral::new(uart0, uart_pins, resets)
        .enable(
            hal::uart::UartConfig::new(
                HertzU32::from_raw(115_200),
                hal::uart::DataBits::Eight,
                None,
                hal::uart::StopBits::One,
            ),
            clocks.peripheral_clock.freq(),
        )
        .expect("configure UART0")
}

/// Stores the UART peripheral in a global mutex for shared access.
///
/// Must be called exactly once during firmware initialization.
///
/// # Arguments
///
/// * `uart` - Configured UART0 peripheral.
pub fn store_global(uart: Uart0) {
    critical_section::with(|cs| {
        UART.borrow(cs).replace(Some(uart));
    });
}

/// Writes a message to UART0 via the HAL peripheral.
///
/// Sends each byte using blocking writes. Converts `\n` to `\r\n`
/// for proper terminal display.
///
/// # Arguments
///
/// * `msg` - Byte slice to transmit.
///
/// # Panics
///
/// Panics if called before `store_global`.
pub fn write_msg(msg: &[u8]) {
    critical_section::with(|cs| {
        let cell = UART.borrow(cs);
        let uart = cell.borrow();
        let uart = uart.as_ref().unwrap();
        for &b in msg {
            if b == b'\n' {
                uart.write_full_blocking(b"\r");
            }
            uart.write_full_blocking(&[b]);
        }
    });
}

/// Reads a single byte from UART0 via the HAL peripheral (blocking).
///
/// Spins until a byte is available in the RX FIFO, then returns it.
///
/// # Returns
///
/// The byte read from UART0.
///
/// # Panics
///
/// Panics if called before `store_global`.
pub fn read_byte() -> u8 {
    critical_section::with(|cs| {
        let cell = UART.borrow(cs);
        let mut uart = cell.borrow_mut();
        let uart = uart.as_mut().unwrap();
        let mut buf = [0u8; 1];
        let _ = block!(uart.read_raw(&mut buf));
        buf[0]
    })
}

/// Writes a single byte to UART0 via the HAL peripheral (blocking).
///
/// # Arguments
///
/// * `byte` - The byte to transmit over UART0.
///
/// # Panics
///
/// Panics if called before `store_global`.
pub fn write_byte(byte: u8) {
    critical_section::with(|cs| {
        let cell = UART.borrow(cs);
        let uart = cell.borrow();
        uart.as_ref().unwrap().write_full_blocking(&[byte]);
    });
}

/// Initializes UART0 at 115200 baud via direct register writes.
///
/// Configures GPIO0 (TX) and GPIO1 (RX) for UART function, takes UART0
/// out of reset, and sets the baud rate divisors for 115200 baud at the
/// default 150 MHz peripheral clock. Used in the panic handler where HAL
/// abstractions are unavailable.
///
/// # Safety
///
/// Uses `unsafe` to write directly to hardware registers. Safe to call
/// from the panic handler on this single-threaded platform.
pub fn panic_init() {
    /// RESETS peripheral base address.
    const RESETS_BASE: u32 = 0x4002_0000;
    /// Pointer to the RESETS atomic-clear register for deasserting resets.
    const RESET_CLR: *mut u32 = (RESETS_BASE + 0x3000) as *mut u32;
    /// Pointer to the RESET_DONE register for checking reset completion.
    const RESET_DONE: *const u32 = (RESETS_BASE + 0x0008) as *const u32;
    /// IO_BANK0 peripheral base address for GPIO control.
    const IO_BANK0_BASE: u32 = 0x4002_8000;
    /// Pointer to the GPIO0 function select control register.
    const GPIO0_CTRL: *mut u32 = (IO_BANK0_BASE + 0x004) as *mut u32;
    /// Pointer to the GPIO1 function select control register.
    const GPIO1_CTRL: *mut u32 = (IO_BANK0_BASE + 0x00C) as *mut u32;
    /// Pointer to the UART integer baud rate divisor register.
    const UARTIBRD: *mut u32 = (UART0_BASE + 0x024) as *mut u32;
    /// Pointer to the UART fractional baud rate divisor register.
    const UARTFBRD: *mut u32 = (UART0_BASE + 0x028) as *mut u32;
    /// Pointer to the UART line control register (data bits, FIFO enable).
    const UARTLCR_H: *mut u32 = (UART0_BASE + 0x02C) as *mut u32;
    /// Pointer to the UART control register (enable, TX/RX).
    const UARTCR: *mut u32 = (UART0_BASE + 0x030) as *mut u32;
    unsafe {
        core::ptr::write_volatile(RESET_CLR, (1 << 26) | (1 << 6));
        while core::ptr::read_volatile(RESET_DONE) & (1 << 26) == 0 {}
        while core::ptr::read_volatile(RESET_DONE) & (1 << 6) == 0 {}
        core::ptr::write_volatile(GPIO0_CTRL, 2);
        core::ptr::write_volatile(GPIO1_CTRL, 2);
        core::ptr::write_volatile(UARTIBRD, 81);
        core::ptr::write_volatile(UARTFBRD, 24);
        core::ptr::write_volatile(UARTLCR_H, (0b11 << 5) | (1 << 4));
        core::ptr::write_volatile(UARTCR, (1 << 0) | (1 << 8) | (1 << 9));
    }
}

/// Writes a single byte to UART0 via direct register access.
///
/// Spins until the TX FIFO has space (UARTFR bit 5 clear), then writes
/// the byte to the data register.
///
/// # Arguments
///
/// * `byte` - The byte to transmit over UART0.
///
/// # Safety
///
/// Uses `unsafe` to read and write directly to UART0 hardware registers.
pub fn panic_write_byte(byte: u8) {
    /// Pointer to the UART data register for transmitting bytes.
    const UARTDR: *mut u32 = UART0_BASE as *mut u32;
    /// Pointer to the UART flag register for checking TX FIFO status.
    const UARTFR: *const u32 = (UART0_BASE + 0x018) as *const u32;
    unsafe {
        while core::ptr::read_volatile(UARTFR) & (1 << 5) != 0 {}
        core::ptr::write_volatile(UARTDR, byte as u32);
    }
}

/// Writes a byte slice to UART0 via direct register access.
///
/// Sends each byte sequentially, blocking until the TX FIFO has space.
/// Converts `\n` to `\r\n` for proper terminal display.
///
/// # Arguments
///
/// * `msg` - Byte slice to transmit over UART0.
pub fn panic_write(msg: &[u8]) {
    for &b in msg {
        if b == b'\n' {
            panic_write_byte(b'\r');
        }
        panic_write_byte(b);
    }
}
