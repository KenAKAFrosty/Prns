use std::collections::VecDeque;

use crate::interfaces::InterfaceId;
use crate::manifold::grant_lane::EXPEDITED_BURST;

use super::EgressQueue;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EgressOrigin {
    Ingress(InterfaceId),
    Internal,
}

struct PendingEgress {
    bytes: std::vec::Vec<u8>,
    origin: EgressOrigin,
}

pub(super) struct PendingEgressQueue {
    expedited: VecDeque<PendingEgress>,
    bulk: VecDeque<PendingEgress>,
    expedited_streak: usize,
}

impl PendingEgressQueue {
    pub(super) fn new() -> Self {
        Self {
            expedited: VecDeque::new(),
            bulk: VecDeque::new(),
            expedited_streak: 0,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.expedited.is_empty() && self.bulk.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.expedited.len().saturating_add(self.bulk.len())
    }

    pub(super) fn push(
        &mut self,
        queue: EgressQueue,
        bytes: std::vec::Vec<u8>,
        origin: EgressOrigin,
    ) {
        let pending = PendingEgress { bytes, origin };
        match queue {
            EgressQueue::Expedited => self.expedited.push_back(pending),
            EgressQueue::Bulk => self.bulk.push_back(pending),
        }
    }

    pub(super) fn pop(&mut self) -> Option<(EgressQueue, std::vec::Vec<u8>)> {
        if self.expedited_streak < EXPEDITED_BURST {
            if let Some(pending) = self.expedited.pop_front() {
                self.expedited_streak += 1;
                return Some((EgressQueue::Expedited, pending.bytes));
            }
        }
        if let Some(pending) = self.bulk.pop_front() {
            self.expedited_streak = 0;
            return Some((EgressQueue::Bulk, pending.bytes));
        }
        let pending = self.expedited.pop_front()?;
        self.expedited_streak = EXPEDITED_BURST;
        Some((EgressQueue::Expedited, pending.bytes))
    }

    pub(super) fn blocks_source(&self, source: InterfaceId) -> bool {
        self.expedited.iter().chain(&self.bulk).any(|pending| {
            matches!(pending.origin, EgressOrigin::Internal)
                || pending.origin == EgressOrigin::Ingress(source)
        })
    }
}
