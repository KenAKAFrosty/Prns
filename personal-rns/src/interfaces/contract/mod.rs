//! The contract every concrete interface signs.
//!
//! The base [`Interface`] trait says what any interface must do; the
//! per-medium refinements ([`PointToPointInterface`],
//! [`SharedBroadcastInterface`]) mark what a given kind can do on top of it.
//! The inert vocabulary these traits traffic in (`Capabilities`,
//! `ConnectionState`, `InterfaceId`, `MediumKind`, `InterfaceMode`) lives at
//! the [`interfaces`](crate::interfaces) root, not here — the contract is the
//! behavior, those are the nouns it speaks in.

mod interface;
mod point_to_point;
mod shared_broadcast;

pub use interface::Interface;
pub use point_to_point::PointToPointInterface;
pub use shared_broadcast::SharedBroadcastInterface;
