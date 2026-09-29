use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::interfaces::{InterfaceId, StatusView};

use super::{RegisteredInterface, RetiredMemberFrameAccounting};

pub(in crate::runtime::node_facade) enum StatusRegistration {
    Unreported,
    Reported { epoch: u64, view: StatusView },
}

impl StatusRegistration {
    pub(super) fn new(epoch: u64, view: Option<&StatusView>) -> Self {
        match view {
            Some(view) => Self::Reported {
                epoch,
                view: view.clone(),
            },
            None => Self::Unreported,
        }
    }

    pub(super) fn retire(
        self,
        interfaces: &Arc<Mutex<HashMap<InterfaceId, RegisteredInterface>>>,
        id: InterfaceId,
        supervisor: Option<InterfaceId>,
    ) {
        let Self::Reported { epoch, view } = self else {
            return;
        };
        let Ok(mut map) = interfaces.lock() else {
            return;
        };
        // Registration is synchronous; a replacement may already be visible while this run's
        // teardown is still queued. Its final counters belong to the departing run, not that entry.
        if map
            .get(&id)
            .is_some_and(|registered| registered.attachment_epoch == epoch)
        {
            map.remove(&id);
        }
        let Some(kept) = supervisor.and_then(|supervisor| map.get_mut(&supervisor)) else {
            return;
        };
        let vitals = view();
        let mut retired_frames = if vitals.is_empty() {
            RetiredMemberFrameAccounting::Incomplete
        } else {
            RetiredMemberFrameAccounting::Unseen
        };
        let (rx, tx) = vitals.into_iter().fold((0u64, 0u64), |(rx, tx), vitals| {
            retired_frames.include(match vitals.frame_accounting {
                Some(accounting) => RetiredMemberFrameAccounting::Complete(accounting),
                None => RetiredMemberFrameAccounting::Incomplete,
            });
            (
                rx.saturating_add(vitals.rx_bytes),
                tx.saturating_add(vitals.tx_bytes),
            )
        });
        kept.retired_member_bytes.rx = kept.retired_member_bytes.rx.saturating_add(rx);
        kept.retired_member_bytes.tx = kept.retired_member_bytes.tx.saturating_add(tx);
        kept.retired_member_frame_accounting.include(retired_frames);
    }
}
