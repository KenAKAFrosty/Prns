use std::collections::VecDeque;

use crate::manifold::grant_lane::EXPEDITED_BURST;

use super::handshake::PendingHandshake;
use super::EgressQueue;

struct PendingEgress {
    bytes: std::vec::Vec<u8>,
    handshake: Option<PendingHandshake>,
    completion: crate::manifold::grant_lane::FrameCompletion,
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
        handshake: Option<PendingHandshake>,
        completion: crate::manifold::grant_lane::FrameCompletion,
    ) {
        let pending = PendingEgress {
            bytes,
            handshake,
            completion,
        };
        match queue {
            EgressQueue::Expedited => self.expedited.push_back(pending),
            EgressQueue::Bulk => self.bulk.push_back(pending),
        }
    }

    pub(super) fn pop(
        &mut self,
    ) -> Option<(
        EgressQueue,
        std::vec::Vec<u8>,
        Option<PendingHandshake>,
        crate::manifold::grant_lane::FrameCompletion,
    )> {
        if self.expedited_streak < EXPEDITED_BURST {
            if let Some(pending) = self.expedited.pop_front() {
                self.expedited_streak += 1;
                return Some((
                    EgressQueue::Expedited,
                    pending.bytes,
                    pending.handshake,
                    pending.completion,
                ));
            }
        }
        if let Some(pending) = self.bulk.pop_front() {
            self.expedited_streak = 0;
            return Some((
                EgressQueue::Bulk,
                pending.bytes,
                pending.handshake,
                pending.completion,
            ));
        }
        let pending = self.expedited.pop_front()?;
        self.expedited_streak = EXPEDITED_BURST;
        Some((
            EgressQueue::Expedited,
            pending.bytes,
            pending.handshake,
            pending.completion,
        ))
    }

    pub(super) fn clear_observed(&mut self, mut observe: impl FnMut(PendingHandshake)) -> usize {
        for pending in self.expedited.iter().chain(self.bulk.iter()) {
            if let Some(handshake) = pending.handshake {
                observe(handshake);
            }
        }
        let cleared = self.len();
        self.expedited.clear();
        self.bulk.clear();
        self.expedited_streak = 0;
        cleared
    }
}
