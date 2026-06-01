//! The app-facing view of the runtime — the one seam an application reads the
//! engine's state through.
//!
//! An app (a status display, a metrics exporter, a control UI) never touches
//! engine internals; it reads a [`RuntimeSnapshot`]. The [`Runtime`] decides
//! what is surfaced here — today liveness, Reticulum traffic, and tracked
//! destinations per interface; the set grows as apps need more, but it stays a
//! deliberate, named view rather than a window onto raw engine state. The
//! runtime's [`run`] loop publishes a fresh snapshot each cycle (e.g. into a
//! `Watch` a display subscribes to), so the app reacts to engine changes
//! without polling engine internals.
//!
//! [`Runtime`]: super::Runtime
//! [`run`]: super::run

use heapless::Vec as HeaplessVec;

use crate::engine::MAX_REGISTERED_INTERFACES;
use crate::interfaces::InterfaceId;

/// One interface's slice of a [`RuntimeSnapshot`].
///
/// The byte counts are *Reticulum* traffic only — packets the engine actually
/// ingested from or emitted to this interface. A sub-interface's own chatter
/// (discovery beacons, link keepalives) never crosses the engine seam, so it is
/// structurally excluded; this is the fabric-level flow, not the wire-level
/// flow.
///
/// `PartialEq` lets an app cheaply tell whether anything it shows has changed
/// since the last snapshot (e.g. to decide whether to redraw or sleep a panel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceView {
    pub id: InterfaceId,
    /// Whether the interface's link is up right now (from its worker's health).
    pub online: bool,
    /// Cumulative Reticulum bytes the engine has ingested from this interface
    /// since boot (wrapping).
    pub reticulum_rx_bytes: u64,
    /// Cumulative Reticulum bytes the engine has emitted to this interface
    /// since boot (wrapping).
    pub reticulum_tx_bytes: u64,
    /// Routing-table destinations reachable via this interface — the routes
    /// whose accepted announce arrived on it (RNS's `receiving_interface`).
    /// A node with several interfaces sees each one's own share here, not the
    /// global total.
    pub tracked_destinations: u32,
}

/// A whole-runtime view captured at the end of a drive cycle: one
/// [`InterfaceView`] per registered interface. Cheap to clone (fixed capacity,
/// no allocation) so a platform runtime can hand it across a channel.
#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub interfaces: HeaplessVec<InterfaceView, MAX_REGISTERED_INTERFACES>,
}
