//! Concrete [`Interface`](crate::interfaces::Interface) implementations the
//! core ships.
//!
//! Each one is a faithful tx/rx actor honoring the contract, never a source of
//! routing or fanout decisions. [`loopback`] is the in-process family (alloc /
//! threaded / no_alloc variants); real-transport impls (USB serial, TCP, LoRa,
//! BLE) live in host bodies, not here, since they pull in platform I/O the core
//! stays free of.

pub mod loopback;
