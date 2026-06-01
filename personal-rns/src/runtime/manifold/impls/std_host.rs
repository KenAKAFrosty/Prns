//! The std inbound mailbox. A worker runs in its own OS thread and stamps
//! [`InboxEntry`]s into the mpsc mailbox the runtime drains. The mailbox lives
//! here, with its draining end — a worker is handed an [`InboundSender`] and
//! stamps into it; the host (e.g. `LinuxSync`) holds the [`InboundReceiver`].

use std::sync::mpsc::{Receiver, Sender};
use std::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;

/// One inbound packet a worker stamped: owned wire bytes plus the provenance the
/// runtime needs (which interface heard it, and when). Owned (not borrowed)
/// because it rides an mpsc channel from the worker thread to the runtime thread.
pub struct InboxEntry {
    pub arrived_at: InstantMillis,
    pub source: InterfaceId,
    pub bytes: Vec<u8>,
}

/// The stamping end a worker holds.
pub type InboundSender = Sender<InboxEntry>;
/// The draining end the host holds.
pub type InboundReceiver = Receiver<InboxEntry>;
