//! Bounded, connection-local responsiveness policy. No timers, allocation, or I/O.
//!
//! A native capability read enables the dialer; a listener only enables enforcement
//! after receiving a valid probe on this settled physical session. Neither the
//! capability nor a reply authenticates the peer. Legacy and Columba sessions stay
//! disabled. A listener whose peer disappears before its first probe stays dormant.

use super::Control;

/// Magic, format version, and exact supported feature set. Unknown values disable
/// liveness rather than opting into a partially understood protocol.
pub const LIVENESS_CAPABILITY_BYTES: [u8; 6] = *b"PRNS\x01\x01";
pub const LIVENESS_IDLE_MS: u64 = 30_000;
pub const LIVENESS_OPERATION_MS: u64 = 30_000;
/// At most one reply admitted per interval, with only one retained reply slot.
pub const LIVENESS_REPLY_INTERVAL_MS: u64 = 1_000;

#[must_use]
pub fn supports_liveness_capability(value: &[u8]) -> bool {
    value == LIVENESS_CAPABILITY_BYTES
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LivenessMode {
    #[default]
    Disabled,
    /// May answer probes; no enforcement before the first probe on this session.
    Listener,
    /// A successful capability read on this physical connection permits probing.
    Initiator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessWriteKind {
    Probe,
    Reply,
}

/// One reserved local write. Retain its send future until completion or retirement;
/// do not restart it when an unrelated control message or challenger arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivenessWrite {
    pub kind: LivenessWriteKind,
    pub nonce: u64,
    /// Absolute caller-clock deadline, capped by any earlier policy expiration.
    pub deadline_ms: u64,
}

impl LivenessWrite {
    #[must_use]
    pub const fn control(self) -> Control {
        match self.kind {
            LivenessWriteKind::Probe => Control::Probe { nonce: self.nonce },
            LivenessWriteKind::Reply => Control::ProbeReply { nonce: self.nonce },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessAction {
    Wait,
    Send(LivenessWrite),
    /// Retire this exact session. This does not assert that the remote peer died.
    Retire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbePhase {
    Dormant,
    Idle,
    Queued,
    Sending,
    AwaitingReply,
    RepliedWhileSending,
}

/// One outstanding probe, one retained reply, and one active local write.
///
/// Supply milliseconds from one monotonic clock and fresh session-local random
/// entropy as `random_seed`. The nonce counter never wraps. Clock regression or
/// exhaustion retires an enabled session rather than reusing a correlation token.
/// Timers are anchored before submission; suspension does not renew their budget.
/// No data-traffic or challenger input can postpone them.
pub struct LivenessPolicy {
    next_nonce: u64,
    probe_nonce: u64,
    probe_deadline_ms: u64,
    probe_replied_at_ms: u64,
    next_probe_ms: u64,
    reply_nonce: u64,
    reply_deadline_ms: u64,
    reply_not_before_ms: u64,
    last_now_ms: u64,
    sending_deadline_ms: u64,
    mode: LivenessMode,
    phase: ProbePhase,
    sending: Option<LivenessWriteKind>,
    reply_pending: bool,
    nonce_exhausted: bool,
    retired: bool,
}

impl LivenessPolicy {
    #[must_use]
    pub const fn new(mode: LivenessMode, now_ms: u64, random_seed: u64) -> Self {
        Self {
            next_nonce: random_seed,
            probe_nonce: 0,
            probe_deadline_ms: 0,
            probe_replied_at_ms: 0,
            next_probe_ms: now_ms,
            reply_nonce: 0,
            reply_deadline_ms: 0,
            reply_not_before_ms: 0,
            last_now_ms: now_ms,
            sending_deadline_ms: 0,
            mode,
            phase: if matches!(mode, LivenessMode::Initiator) {
                ProbePhase::Idle
            } else {
                ProbePhase::Dormant
            },
            sending: None,
            reply_pending: false,
            nonce_exhausted: false,
            retired: false,
        }
    }

    /// Reserve at most one write. Re-polling never emits a second copy of an
    /// active write; the caller must report its exact completion with `sent`.
    pub fn poll(&mut self, now_ms: u64) -> LivenessAction {
        if !self.advance(now_ms) {
            return LivenessAction::Retire;
        }
        if self.sending.is_some() {
            return LivenessAction::Wait;
        }
        let (kind, nonce, mut deadline_ms) = if self.phase == ProbePhase::Queued {
            self.phase = ProbePhase::Sending;
            (
                LivenessWriteKind::Probe,
                self.probe_nonce,
                self.probe_deadline_ms,
            )
        } else if self.reply_pending {
            (
                LivenessWriteKind::Reply,
                self.reply_nonce,
                self.reply_deadline_ms,
            )
        } else {
            return LivenessAction::Wait;
        };
        if let Some(expiry) = self.probe_expiry_ms() {
            deadline_ms = deadline_ms.min(expiry);
        }
        if self.reply_pending {
            deadline_ms = deadline_ms.min(self.reply_deadline_ms);
        }
        self.sending = Some(kind);
        self.sending_deadline_ms = deadline_ms;
        LivenessAction::Send(LivenessWrite {
            kind,
            nonce,
            deadline_ms,
        })
    }

    /// Process a decoded control from this settled session. Unrelated messages,
    /// wrong/duplicate replies, and excess probes do not refresh any timer.
    pub fn receive(&mut self, now_ms: u64, message: &Control) {
        if !self.advance(now_ms) || self.mode == LivenessMode::Disabled {
            return;
        }
        match *message {
            Control::Probe { nonce } => {
                if self.phase == ProbePhase::Dormant && !self.schedule_probe(now_ms) {
                    return;
                }
                if !self.reply_pending && now_ms >= self.reply_not_before_ms {
                    let Some(deadline) = now_ms.checked_add(LIVENESS_OPERATION_MS) else {
                        self.retired = true;
                        return;
                    };
                    let Some(not_before) = now_ms.checked_add(LIVENESS_REPLY_INTERVAL_MS) else {
                        self.retired = true;
                        return;
                    };
                    self.reply_pending = true;
                    self.reply_nonce = nonce;
                    self.reply_deadline_ms = deadline;
                    self.reply_not_before_ms = not_before;
                }
            }
            Control::ProbeReply { nonce } if nonce == self.probe_nonce => match self.phase {
                ProbePhase::Sending => {
                    self.phase = ProbePhase::RepliedWhileSending;
                    self.probe_replied_at_ms = now_ms;
                }
                ProbePhase::AwaitingReply => {
                    self.schedule_probe(now_ms);
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Local platform completion is necessary, but is never a peer response.
    /// A stale/duplicate write receipt has no effect on a newer write.
    pub fn sent(&mut self, now_ms: u64, write: LivenessWrite, success: bool) {
        if !self.advance(now_ms) || self.current_write() != Some(write) {
            return;
        }
        if !success {
            self.retired = true;
            return;
        }
        self.sending = None;
        match write.kind {
            LivenessWriteKind::Reply => self.reply_pending = false,
            LivenessWriteKind::Probe => match self.phase {
                ProbePhase::Sending => self.phase = ProbePhase::AwaitingReply,
                ProbePhase::RepliedWhileSending => {
                    self.schedule_probe(self.probe_replied_at_ms);
                }
                _ => {}
            },
        }
    }

    /// Wake time for the next due probe or hard expiration. Call `poll` after
    /// every wake; a resumed runtime must expire stale state before processing
    /// a queued reply. During a send its emitted absolute deadline is sufficient.
    #[must_use]
    pub fn next_deadline_ms(&self) -> Option<u64> {
        if self.retired {
            return Some(self.last_now_ms);
        }
        let probe_deadline = match self.phase {
            ProbePhase::Dormant => None,
            ProbePhase::Idle => Some(self.next_probe_ms),
            _ => Some(self.probe_deadline_ms),
        };
        let reply_deadline = self.reply_pending.then_some(self.reply_deadline_ms);
        let send_deadline = self.sending.map(|_| self.sending_deadline_ms);
        [probe_deadline, reply_deadline, send_deadline]
            .into_iter()
            .flatten()
            .min()
    }

    fn current_write(&self) -> Option<LivenessWrite> {
        self.sending.map(|kind| LivenessWrite {
            kind,
            nonce: match kind {
                LivenessWriteKind::Probe => self.probe_nonce,
                LivenessWriteKind::Reply => self.reply_nonce,
            },
            deadline_ms: self.sending_deadline_ms,
        })
    }

    fn probe_expiry_ms(&self) -> Option<u64> {
        match self.phase {
            ProbePhase::Dormant => None,
            // Scheduling checked the cadence addition; saturating here gives a
            // finite bound even when the following operation budget cannot fit.
            ProbePhase::Idle => Some(self.next_probe_ms.saturating_add(LIVENESS_OPERATION_MS)),
            _ => Some(self.probe_deadline_ms),
        }
    }

    fn schedule_probe(&mut self, after_ms: u64) -> bool {
        if let Some(due) = after_ms.checked_add(LIVENESS_IDLE_MS) {
            self.next_probe_ms = due;
            self.phase = ProbePhase::Idle;
            true
        } else {
            self.retired = true;
            false
        }
    }

    fn advance(&mut self, now_ms: u64) -> bool {
        if self.retired {
            return false;
        }
        if self.phase != ProbePhase::Dormant && now_ms < self.last_now_ms {
            self.retired = true;
            return false;
        }
        self.last_now_ms = now_ms;
        if self
            .probe_expiry_ms()
            .is_some_and(|deadline| now_ms >= deadline)
            || (self.reply_pending && now_ms >= self.reply_deadline_ms)
            || (self.sending.is_some() && now_ms >= self.sending_deadline_ms)
        {
            self.retired = true;
            return false;
        }
        if self.phase == ProbePhase::Idle && now_ms >= self.next_probe_ms {
            let Some(deadline) = self.next_probe_ms.checked_add(LIVENESS_OPERATION_MS) else {
                self.retired = true;
                return false;
            };
            if self.nonce_exhausted {
                self.retired = true;
                return false;
            }
            self.probe_nonce = self.next_nonce;
            self.probe_deadline_ms = deadline;
            if let Some(next) = self.next_nonce.checked_add(1) {
                self.next_nonce = next;
            } else {
                self.nonce_exhausted = true;
            }
            self.phase = ProbePhase::Queued;
        }
        true
    }
}

#[cfg(test)]
mod tests;
