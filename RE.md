# Reverse Engineering: embedded-wasm-button-rp2350

## Table of Contents

1. [Binary Overview](#1-binary-overview)
2. [ELF Header](#2-elf-header)
3. [Section Layout](#3-section-layout)
4. [Memory Map & Segments](#4-memory-map--segments)
5. [Boot Sequence](#5-boot-sequence)
6. [Vector Table](#6-vector-table)
7. [Firmware Function Map](#7-firmware-function-map)
8. [Hardware Register Access](#8-hardware-register-access)
9. [Pulley Interpreter Deep Dive](#9-pulley-interpreter-deep-dive)
10. [Embedded cwasm Blob](#10-embedded-cwasm-blob)
11. [Host-Guest Call Flow](#11-host-guest-call-flow)
12. [RE Observations](#12-re-observations)
13. [Pulley Instruction Set Architecture](#13-pulley-instruction-set-architecture)
14. [Pulley Bytecode Disassembly](#14-pulley-bytecode-disassembly)
15. [Ghidra Analysis Walkthrough](#15-ghidra-analysis-walkthrough)

---

## 1. Binary Overview

| Property       | Value                                    |
| -------------- | ---------------------------------------- |
| File           | `embedded-wasm-button-rp2350`            |
| Size on disk   | 1,184,720 bytes (1.13 MiB)               |
| Format         | ELF32 ARM little-endian                  |
| ABI            | EABI5, hard-float                        |
| Target         | ARMv8-M Mainline (Cortex-M33)            |
| MCU            | RP2350 (Raspberry Pi Pico 2)             |
| Stripped       | No (symbol table + string table present) |
| Text functions | 2,392                                    |

The binary is a bare-metal `no_std` Rust firmware that hosts a Wasmtime
Component Model runtime with the Pulley bytecode interpreter. A
precompiled Wasm component reads GPIO15 (button) every 10 ms and drives
GPIO25 (LED) accordingly — LED stays on while the button is held. Host
bindings expose `gpio`, `button`, and `timing` WIT interfaces.

---

## 2. ELF Header

```
Magic:   7f 45 4c 46 01 01 01 03 00 00 00 00 00 00 00 00
Class:                             ELF32
Data:                              2's complement, little endian
Version:                           1 (current)
OS/ABI:                            UNIX - GNU
Type:                              EXEC (Executable file)
Machine:                           ARM
Entry point address:               0x1000010d
Flags:                             0x5000400, Version5 EABI, hard-float ABI
Program headers:                   6 (at offset 52)
Section headers:                   16 (at offset 1,184,080)
```

**Entry Point**: `0x1000010d` — the `Reset` handler in `.text`. The LSB is
set (0x0D vs 0x0C) to indicate Thumb mode, required by ARMv8-M. The actual
code starts at `0x1000010c`.

---

## 3. Section Layout

```
Nr  Name            Type        Addr        Size      Flags  Description
 1  .vector_table   PROGBITS    0x10000000  0x000f8   A      ARM exception + interrupt vectors
 2  .start_block    PROGBITS    0x100000f8  0x00014   AR     RP2350 IMAGE_DEF boot metadata
 3  .text           PROGBITS    0x1000010c  0x85f48   AX     All executable code (548 KiB)
 4  .bi_entries     PROGBITS    0x10086054  0x00000   A      Binary info entries (empty)
 5  .rodata         PROGBITS    0x10086058  0x1e050   AMSR   Read-only data (120 KiB)
 6  .data           PROGBITS    0x20000000  0x00024   WA     Initialized globals (36 bytes)
 7  .gnu.sgstubs    PROGBITS    0x100a40d0  0x00000   A      Secure gateway stubs (empty)
 8  .bss            NOBITS      0x20000028  0x400b4   WA     Zero-init data (256 KiB)
 9  .uninit         NOBITS      0x200400dc  0x00000   WA     Uninitialized memory (empty)
10  .end_block      PROGBITS    0x100a40d0  0x00000   WA     Block end marker (empty)
13  .symtab         SYMTAB      file only   0x1db40          Symbol table (7,580 entries)
15  .strtab         STRTAB      file only   0x60c0e          String table (389 KiB)
```

### Size Breakdown

| Region         | Section         | Size          | % of 4 MiB Flash     |
| -------------- | --------------- | ------------- | -------------------- |
| Code           | `.text`         | 548,680 B     | 13.1%                |
| Constants      | `.rodata`       | 122,960 B     | 2.9%                 |
| Vectors        | `.vector_table` | 248 B         | <0.1%                |
| Boot meta      | `.start_block`  | 20 B          | <0.1%                |
| Init data      | `.data`         | 36 B          | <0.1%                |
| **Flash used** |                 | **671,944 B** | **16.0%**            |
| **Flash free** |                 | 3,522,360 B   | 84.0%                |
| BSS (RAM)      | `.bss`          | 262,324 B     | 51.2% of 512 KiB RAM |

The button variant is ~4 KiB larger than the base blinky due to the
`button` module (`is_pressed` + `store_pin`) and an additional WIT
interface.

---

## 4. Memory Map & Segments

```
Segment  VirtAddr     PhysAddr     MemSiz   Flags  Contents
  0      0x10000000   0x10000000   0x0010c  R      .vector_table + .start_block
  1      0x1000010c   0x1000010c   0x85f48  R E    .text (executable code)
  2      0x10086054   0x10086054   0x1e054  R      .rodata (constants + cwasm blob)
  3      0x20000000   0x100a40a8   0x00024  RW     .data (LMA in flash, VMA in RAM)
  4      0x20000028   0x20000028   0x400b4  RW     .bss (zero-filled at boot)
  5      0x00000000   0x00000000   0x00000  RW     GNU_STACK (zero-size)
```

### Physical Address Space

```
Flash (XIP):  0x10000000 - 0x100a40cb  (672 KiB used of 4 MiB)
              +-- 0x10000000  Vector table (248 B)
              +-- 0x100000f8  IMAGE_DEF boot block (20 B)
              +-- 0x1000010c  .text starts (Reset handler)
              +-- 0x10086058  .rodata starts
              +-- 0x1008a809  Embedded cwasm (Pulley ELF, ~25 KiB)
              +-- 0x100a40a8  .data initializers (36 B, copied to RAM)

RAM (SRAM):   0x20000000 - 0x200400db  (256 KiB used of 512 KiB)
              +-- 0x20000000  .data (36 B: UART state, TLS value)
              +-- 0x20000028  HEAP_MEM (262,144 B = 256 KiB)
              +-- 0x20040048  TLS_VALUE (4 B)
              +-- 0x2004004c  led::PINS (16 B)
              +-- 0x20040060  button::PINS (16 B)
              +-- 0x20040070  HEAP allocator struct (32 B)

Stack:        0x20080000  Initial SP (top of 512 KiB SRAM, grows down)
```

---

## 5. Boot Sequence

### 5.1 RP2350 Boot ROM -> IMAGE_DEF

The RP2350 Boot ROM scans flash for a valid image definition block. Our
`.start_block` section at `0x100000f8` contains:

```
d3deffff 42012110 ff010000 00000000 793512ab
```

This is `hal::block::ImageDef::secure_exe()` — it tells the Boot ROM
this is a secure ARM executable.

### 5.2 Vector Table -> Reset Handler

```
Word 0: 0x20080000  <- Initial Stack Pointer
Word 1: 0x1000010d  <- Reset vector (Thumb-mode)
```

### 5.3 Reset Handler (0x1000010c)

```armasm
Reset:
    bl      DefaultPreInit          ; No-op
    ; --- Zero .bss (0x20000028 -> 0x200400dc) ---
    ; --- Copy .data from flash (0x100a40a8) to RAM (0x20000000) ---
    ; --- Enable FPU ---
    bl      main
    udf     #0
```

### 5.4 `main()` (0x10007b78)

```armasm
main:
    bl      __cortex_m_rt_main      ; at 0x10006c50
```

### 5.5 `__cortex_m_rt_main` (0x10006c50)

```
    ; Enable SIO GPIO outputs for GPIO15 (button) and GPIO25 (LED)
    ; Enable FPU
    bl      init_heap               ; Initialize 256 KiB heap
    bl      init_hardware           ; Clocks, UART, GPIO15, GPIO25, SysTick
    bl      run_wasm                ; Run the Wasm button component (never returns)
```

---

## 6. Vector Table

The vector table at `0x10000000` is 248 bytes (62 entries):

```
Offset  Vector              Handler          Address
0x0000  Initial SP          —                0x20080000
0x0004  Reset               Reset            0x1000010d
0x0008  NMI                 DefaultHandler   0x1007dac5
0x000c  HardFault           HardFault_       0x1008604d
0x0040+ IRQ0-IRQ51          DefaultHandler   0x1007dac5
```

All exception/IRQ vectors point to `DefaultHandler` (infinite loop)
except HardFault (also infinite loop). No peripheral interrupts are used.

---

## 7. Firmware Function Map

### 7.1 Application Functions

| Address      | Size  | Symbol               | Purpose                                             |
| ------------ | ----- | -------------------- | --------------------------------------------------- |
| `0x1000010c` | 0x3e  | `Reset`              | BSS zero, .data copy, FPU enable, call main         |
| `0x10006b38` | 0x118 | `init_hardware`      | Clocks, UART0, GPIO15, GPIO25, SysTick              |
| `0x10006c50` | 0x6c  | `__cortex_m_rt_main` | SIO enable, FPU, init_heap, init_hardware, run_wasm |
| `0x10006cbc` | 0x842 | `run_wasm`           | Create engine, deserialize cwasm, run guest         |
| `0x10007500` | 0x20  | `init_heap`          | Initialize 256 KiB linked-list heap                 |
| `0x10009d88` | 0xc2  | `led::set_high`      | Set GPIO via SIO, UART log                          |
| `0x10009cc4` | 0xc2  | `led::set_low`       | Clear GPIO via SIO, UART log                        |
| `0x10009e4c` | 0x108 | `led::store_pin`     | Store GPIO pin handle for LED module                |
| `0x1000ac60` | 0xc6  | `button::is_pressed` | Read GPIO input level via SIO                       |
| `0x1000ad28` | 0x108 | `button::store_pin`  | Store GPIO pin handle for button module             |
| `0x1000abb4` | 0xac  | `uart::write_msg`    | Blocking UART TX                                    |
| `0x1000aaa8` | 0x10c | `uart::init`         | UART0 peripheral init (115200 8N1)                  |
| `0x10007b78` | 0x8   | `main`               | Thin #[entry] wrapper                               |
| `0x1007dac4` | 0x6   | `DefaultHandler`     | Infinite loop (unhandled exception)                 |
| `0x1007dacc` | 0x6   | `DefaultPreInit`     | No-op (returns immediately)                         |
| `0x1008604c` | 0x6   | `HardFault`          | Infinite loop (hard fault)                          |

### 7.2 Button Module Design

The button module mirrors `led.rs` with a `store_pin` + `is_pressed` pair:

- `button::store_pin` (0x108 B): Stores a GPIO pin handle into
  `button::PINS` at `0x20040060`
- `button::is_pressed` (0xc6 B): Reads the SIO GPIO input register
  `GPIO_IN` at `0xd0000004`, masks the bit for GPIO15, returns `true`
  if set

The button pin (GPIO15) is configured as a pull-down input in
`init_hardware`, while the LED pin (GPIO25) is a push-pull output.

### 7.3 Wasmtime Runtime (Top by Size)

| Address      | Size     | Demangled Name                     |
| ------------ | -------- | ---------------------------------- |
| `0x10033f10` | 16,464 B | `OperatorCost::deserialize`        |
| `0x100694e8` | 16,456 B | `decode_one_extended`              |
| `0x100661cc` | 12,518 B | `Interpreter::run` (dispatch loop) |
| `0x1002f6b0` | 8,696 B  | `Metadata::check_cost`             |
| `0x1000ea2c` | 2,304 B  | `InterpreterRef::call`             |

### 7.4 BSS Layout

| Address      | Size      | Symbol         | Purpose                         |
| ------------ | --------- | -------------- | ------------------------------- |
| `0x20000028` | 262,144 B | `HEAP_MEM`     | Raw heap backing memory         |
| `0x20040048` | 4 B       | `TLS_VALUE`    | Wasmtime TLS shim (platform.rs) |
| `0x2004004c` | 16 B      | `led::PINS`    | GPIO pin handle for LED         |
| `0x20040060` | 16 B      | `button::PINS` | GPIO pin handle for button      |

---

## 8. Hardware Register Access

### 8.1 Peripheral Base Addresses

| Base Address | Peripheral | Usage in Firmware       |
| ------------ | ---------- | ----------------------- |
| `0x40020000` | RESETS     | Subsystem reset control |
| `0x40028000` | IO_BANK0   | GPIO function selection |
| `0x40030000` | PADS_BANK0 | Pad configuration       |
| `0x40040000` | XOSC       | Crystal oscillator      |
| `0x40048000` | PLL_SYS    | System PLL (150 MHz)    |
| `0x4004c000` | PLL_USB    | USB PLL (48 MHz)        |
| `0x40050000` | CLOCKS     | Clock generators        |
| `0x40070000` | UART0      | Debug serial (TX only)  |
| `0xd0000000` | SIO        | Single-cycle I/O (GPIO) |
| `0xe000e010` | SysTick    | System timer (delay)    |
| `0xe000ed88` | CPACR      | FPU access control      |

### 8.2 GPIO Control

Two GPIO pins are configured:

- **GPIO15 (Button)**: Pull-down input. Read via `SIO GPIO_IN` at
  `0xd0000004`. The `button::is_pressed` function masks bit 15 to check
  the button state.

- **GPIO25 (LED)**: Push-pull output. Written via `SIO GPIO_OUT_SET` at
  `0xd0000014` and `SIO GPIO_OUT_CLR` at `0xd0000018`.

```
button::is_pressed (0x1000ac60):
    ; SIO base = 0xd0000000
    ; GPIO_IN @ 0xd0000004
    ldr     r0, =0xd0000004         ; GPIO_IN register
    ldr     r1, [r0]                ; Read all GPIO levels
    tst     r1, #(1 << 15)          ; Test bit 15 (button pin)
    ; Return true if bit is set, false otherwise

led::set_high / led::set_low:
    ; GPIO_OUT_SET @ 0xd0000014, GPIO_OUT_CLR @ 0xd0000018
    ; Same SIO register pattern as blinky
```

### 8.3 SysTick Timer

Used for 10 ms delay between button polls. Same mechanism as all
embedded-wasm repos: reload value set for the desired delay at 150 MHz
system clock.

---

## 9. Pulley Interpreter Deep Dive

### 9.1 Interpreter Entry (`InterpreterRef::call`)

```
Location:  0x1000ea2c  (2,304 bytes)
```

Call sequence:

```
run_wasm()
  -> Button::instantiate()
    -> Button::call_run()
      -> InterpreterRef::call()     <-- native-to-Pulley boundary
        -> Vm::call_start()         ; Set up Pulley register file
        -> Vm::call_run()           ; Enter interpreter loop
          -> Interpreter::run()     ; Main dispatch loop
```

### 9.2 Main Dispatch Loop

```
Location:  0x100661cc  (12,518 bytes)
```

Same two-level dispatch scheme as all embedded-wasm repos: primary
opcodes (0x00-0xDB) handled by a jump table in `Interpreter::run`,
extended opcodes (0xDC prefix + 2-byte opcode) handled by
`decode_one_extended`.

---

## 10. Embedded cwasm Blob

### 10.1 Location and Format

The precompiled Pulley bytecode is embedded in `.rodata` at
`0x1008a809`. It is **25,384 bytes** (0x6328).

| Field  | Value                          |
| ------ | ------------------------------ |
| Magic  | `\x7fELF`                      |
| Class  | ELF64 (byte `02` at offset 4)  |
| Data   | Little-endian                  |
| Target | `pulley32-unknown-unknown-elf` |

The button cwasm is ~700 bytes larger than the base blinky because
it includes an additional host import call (`is_pressed`) and a
conditional branch in the guest loop.

### 10.2 Guest Code Logic

```
fn run() {
    let button_pin: u32 = 15;
    let led_pin: u32 = 25;
    loop {
        if call_import button::is_pressed(button_pin) {
            call_import gpio::set_high(led_pin)
        } else {
            call_import gpio::set_low(led_pin)
        }
        call_import timing::delay_ms(10)
    }
}
```

The guest polls the button state every 10 ms and mirrors it to the LED.

---

## 11. Host-Guest Call Flow

### 11.1 Button Press Detected

```
[Pulley VM]  Bytecode: call_indirect_host -> is_pressed(15)
    |
    v
[ARM Native]  button::is_pressed (0x1000ac60)
    |
    v
[Hardware]  SIO GPIO_IN @ 0xd0000004 -> bit 15 set = true

[Pulley VM]  Bytecode: br_if_xneq32 -> take set_high path
    |
    v
[Pulley VM]  Bytecode: call_indirect_host -> set_high(25)
    |
    v
[ARM Native]  led::set_high (0x10009d88)
    |
    v
[Hardware]  SIO GPIO_OUT_SET @ 0xd0000014 -> LED on

[Pulley VM]  Bytecode: call_indirect_host -> delay_ms(10)
    |
    v
[ARM Native]  SysTick countdown (10 ms)
```

### 11.2 Button Not Pressed

```
[Pulley VM]  Bytecode: call_indirect_host -> is_pressed(15)
    |
    v
[ARM Native]  button::is_pressed (0x1000ac60)
    |
    v
[Hardware]  SIO GPIO_IN @ 0xd0000004 -> bit 15 clear = false

[Pulley VM]  Bytecode: br_if_xneq32 -> take set_low path
    |
    v
[Pulley VM]  Bytecode: call_indirect_host -> set_low(25)
    |
    v
[ARM Native]  led::set_low (0x10009cc4)
    |
    v
[Hardware]  SIO GPIO_OUT_CLR @ 0xd0000018 -> LED off
```

---

## 12. RE Observations

### 12.1 Binary Composition

| Component                          | Approx Size | % of .text |
| ---------------------------------- | ----------- | ---------- |
| Wasmtime runtime                   | ~540 KiB    | 98.5%      |
| Pulley interpreter (run)           | 12.2 KiB    | 2.2%       |
| Pulley decoder (extended)          | 16.1 KiB    | 2.9%       |
| Application (led+button+uart+main) | ~8 KiB      | 1.5%       |

### 12.2 Button vs Blinky Differences

| Aspect         | Blinky         | Button                     |
| -------------- | -------------- | -------------------------- |
| GPIO outputs   | GPIO25 only    | GPIO25 (LED)               |
| GPIO inputs    | None           | GPIO15 (Button, pull-down) |
| WIT interfaces | gpio, timing   | gpio, button, timing       |
| Guest logic    | Toggle loop    | Poll-and-mirror            |
| cwasm size     | 24,680 B       | 25,384 B (+704 B)          |
| BSS difference | led::PINS only | led::PINS + button::PINS   |

### 12.3 Key Addresses Quick Reference

| Address      | What                                          |
| ------------ | --------------------------------------------- |
| `0x10000000` | Vector table (initial SP + exception vectors) |
| `0x100000f8` | RP2350 IMAGE_DEF boot block                   |
| `0x1000010c` | Reset handler (entry point)                   |
| `0x10006b38` | init_hardware                                 |
| `0x10006c50` | __cortex_m_rt_main                            |
| `0x10006cbc` | run_wasm                                      |
| `0x10007500` | init_heap                                     |
| `0x10009d88` | led::set_high (host binding)                  |
| `0x10009cc4` | led::set_low (host binding)                   |
| `0x1000ac60` | button::is_pressed (host binding)             |
| `0x1000abb4` | uart::write_msg                               |
| `0x1000aaa8` | uart::init                                    |
| `0x10007b78` | main (thin wrapper)                           |
| `0x100661cc` | Pulley Interpreter::run (dispatch loop)       |
| `0x100694e8` | Pulley decode_one_extended                    |
| `0x1000ea2c` | InterpreterRef::call (native->Pulley bridge)  |
| `0x1008a809` | Embedded cwasm blob (Pulley ELF)              |
| `0x1007dac4` | DefaultHandler (infinite loop)                |
| `0x1008604c` | HardFault (infinite loop)                     |
| `0xd0000004` | SIO GPIO_IN register (button read)            |
| `0xd0000014` | SIO GPIO_OUT_SET register                     |
| `0xd0000018` | SIO GPIO_OUT_CLR register                     |
| `0x20000028` | HEAP_MEM (256 KiB)                            |
| `0x2004004c` | led::PINS                                     |
| `0x20040060` | button::PINS                                  |
| `0x20080000` | Initial stack pointer                         |

---

## 13. Pulley Instruction Set Architecture

### 13.1 Overview

Pulley is Wasmtime's portable bytecode interpreter (wasmtime 43.0.0,
`pulley-interpreter` crate v43.0.0). It defines a register-based ISA
with variable-length instructions, designed for efficient interpretation
rather than native execution.

### 13.2 Encoding Format

**Primary opcodes** use a 1-byte opcode followed by operands:

```
[opcode:1] [operands:0-9]
```

There are **220 primary opcodes** (0x00-0xDB). Opcode `0xDC` is the
**ExtendedOp** sentinel — when the interpreter encounters it, it reads
a 2-byte extended opcode:

```
[0xDC] [ext_opcode:2] [operands:0-N]
```

There are **310 extended opcodes** (0x0000-0x0135) for SIMD, float
conversions, and complex operations.

### 13.3 Key Instructions

See the [embedded-wasm-servo-rp2350 RE.md](https://github.com/mytechnotalent/embedded-wasm-servo-rp2350)
§13 for the complete Pulley ISA reference. The button guest uses the same
instruction subset as the base blinky with the addition of conditional
branch instructions for the `if is_pressed` test.

---

## 14. Pulley Bytecode Disassembly

### 14.1 Guest::run() — Button Poll Loop

```
; function[N]: Guest::run()

push_frame_save <frame>, <callee-saved regs>

; Load VMContext and function pointers
xload32le_o32 x_heap, x0, 28          ; heap_base
xmov x_vmctx, x0                      ; save VMContext

; Load constants
xconst8 x_button, 15                  ; BUTTON_PIN = 15
xconst8 x_led, 25                     ; LED_PIN = 25
xconst8 x_delay_val, 10               ; DELAY_MS = 10

; Load host function pointers
xload32le_o32 x_is_pressed, x_vmctx, ...
xload32le_o32 x_set_high, x_vmctx, ...
xload32le_o32 x_set_low, x_vmctx, ...
xload32le_o32 x_delay, x_vmctx, ...

.loop:
    ; --- button::is_pressed(15) ---
    xmov x2, x_button                 ; x2 = 15
    call_indirect x_is_pressed         ; -> ARM: button::is_pressed

    ; --- Branch on result ---
    br_if_xeq32 x0, x_zero, .not_pressed

    ; --- Pressed: gpio::set_high(25) ---
    xmov x2, x_led
    call_indirect x_set_high
    jump .delay

.not_pressed:
    ; --- Not pressed: gpio::set_low(25) ---
    xmov x2, x_led
    call_indirect x_set_low

.delay:
    ; --- timing::delay_ms(10) ---
    xmov x2, x_delay_val
    call_indirect x_delay

    jump .loop                         ; Infinite poll loop
```

---

## 15. Ghidra Analysis Walkthrough

### 15.1 Import and Initial Analysis

1. **File -> Import File**: Select the ELF. Ghidra auto-detects
   `ARM:LE:32:v8T` (ARMv8 Thumb). Accept the defaults.

2. **Auto-analysis**: Ghidra identifies 2,392 functions from the symbol
   table.

3. **Analysis time**: ~30 seconds for this 1.13 MiB binary.

### 15.2 Symbol Tree Navigation

```
Functions/ (2,392 total)
+-- Reset                              0x1000010c
+-- main                               0x10007b78
+-- __cortex_m_rt_main                 0x10006c50
+-- embedded_wasm_button_rp2350::run_wasm    0x10006cbc
+-- led::set_high                      0x10009d88
+-- led::set_low                       0x10009cc4
+-- button::is_pressed                 0x1000ac60
+-- button::store_pin                  0x1000ad28
+-- uart::init                         0x1000aaa8
+-- uart::write_msg                    0x1000abb4
+-- pulley_interpreter::interp::Interpreter::run  0x100661cc
+-- pulley_interpreter::decode::decode_one_extended  0x100694e8
+-- InterpreterRef::call               0x1000ea2c
+-- ... (2,379 more)
```

### 15.3 Finding and Extracting the cwasm Blob

1. Navigate to `0x1008a809` in the Listing view
2. Ghidra shows the `7f 45 4c 46` (ELF magic) bytes
3. Right-click -> **Select Bytes** -> enter length 25384 (0x6328)
4. **File -> Export Selection** -> Binary format -> save as `button.cwasm`

### 15.4 Ghidra + G-Pulley: Full-Stack Analysis

With the [G-Pulley](https://github.com/mytechnotalent/G-Pulley) extension
installed, Ghidra can analyze **both** the ARM host firmware and the
Pulley guest bytecode:

| Aspect                  | ARM Host Code             | Pulley Guest Code (G-Pulley)        |
| ----------------------- | ------------------------- | ----------------------------------- |
| Disassembly             | Full ARM Thumb-2          | Full Pulley ISA mnemonics           |
| Function identification | Automatic from symbols    | Automatic (cwasm loader + analyzer) |
| Cross-references        | Full xref graph           | Function calls and branches         |
| Control flow            | CFG with switch detection | Branch and jump targets resolved    |
| Host call boundary      | `InterpreterRef::call`    | `call_indirect_host` instructions   |

**G-Pulley provides**:

- Custom ELF loader that extracts the `.cwasm` blob from the firmware
- SLEIGH processor spec for Pulley 32-bit and 64-bit ISA (Wasmtime v43.0.0)
- Post-load analyzer that discovers functions, trampolines, and host calls
- Full opcode decoding for all 220 primary + 310 extended Pulley opcodes

### 15.5 Recommended Ghidra Workflow

1. **Install G-Pulley**: Download from
   [G-Pulley releases](https://github.com/mytechnotalent/G-Pulley/releases).
   In Ghidra: **File -> Install Extensions -> + -> select zip**. Restart.

2. **Analyze the ARM firmware**: Import the ELF. Run auto-analysis.
   Follow Reset -> main -> `__cortex_m_rt_main` -> `run_wasm`.

3. **Examine host bindings**: Navigate to `button::is_pressed`
   (0x1000ac60) to see how GPIO15 is read via SIO. Check `led::set_high`
   and `led::set_low` for the GPIO25 control path.

4. **Trace the interpreter**: Start at `InterpreterRef::call` (0x1000ea2c),
   follow into `Interpreter::run` (0x100661cc) to see the Pulley dispatch
   loop.

5. **Analyze the Pulley bytecode**: Import the firmware ELF again using
   G-Pulley's cwasm loader (select "Pulley cwasm" format). G-Pulley
   extracts the embedded cwasm blob, disassembles all Pulley opcodes,
   and identifies guest functions including the button poll loop with
   its conditional branch.
