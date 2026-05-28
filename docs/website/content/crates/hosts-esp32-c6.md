## When to reach for this

Look here when you want Reticulum running on a microcontroller — a
sensor node, a portable handset, a battery-powered repeater, a
hobbyist board on your desk.

The ESP32-C6 is the reference target: a $5 RISC-V chip with
built-in WiFi 6, Bluetooth 5, and 802.15.4 radios. Anywhere it
fits, the engine fits.

## What you get

A bare-metal host adapter that drives the Reticulum engine without
an operating system, without a heap allocator, and without any
external dependencies. The clock comes from the chip's hardware
timer; transports come from whatever the board has wired (USB
serial, ESP-NOW, BLE — depending on the build).

This crate is also the gate that keeps the engine honest. Every
time `personal-rns` adds a public surface, this host is the
check: if the change can't compile here, it has broken the
embedded constraint. Other microcontroller targets (RP2040,
nRF52, STM32) will follow the same pattern.

## Status

The C6 firmware builds and runs against the current engine; a
shared `bbp-bramble` codec already carries real Reticulum
announces between two boards over USB, ESP-NOW, and BLE. Phone
integration through a companion daemon comes next.
