//! The app-facing view of the runtime — the one seam an application reads the
//! engine's state through.
//!
//! An app (a status display, a metrics exporter, a control UI) never touches
//! engine internals; it reads a [`RuntimeSnapshot`]. The [`Manifold`] decides
//! what is surfaced here — today liveness, Reticulum traffic, and tracked
//! destinations per interface; the set grows as apps need more, but it stays a
//! deliberate, named view rather than a window onto raw engine state. The
//! per-platform runtime publishes a fresh snapshot each cycle (e.g. into a
//! `Watch` a display subscribes to), so the app reacts to engine changes
//! without polling engine internals.
//!
//! [`Manifold`]: super::Manifold

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
    /// Routing-table destinations reachable via this interface.
    ///
    /// Until the routing table records the interface a destination was learned
    /// on (a planned `RouteEntry` column), this is the whole table — exact
    /// while a node runs a single interface, and the field the per-interface
    /// split lands on once that column exists.
    pub tracked_destinations: u32,
}

/// A whole-runtime view captured at the end of a drive cycle: one
/// [`InterfaceView`] per registered interface. Cheap to clone (fixed capacity,
/// no allocation) so a platform runtime can hand it across a channel.
#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub interfaces: HeaplessVec<InterfaceView, MAX_REGISTERED_INTERFACES>,
}
