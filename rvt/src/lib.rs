//! Multi-node simulation of the Personal Reticulum engine on virtual time.
//!
//! A bespoke driver, not an `EngineDriver`: it advances a virtual clock, moves
//! packets across a virtual wire, and runs each node's engine via the public
//! `ingest`/`tick` primitives — faithful to `step`, which calls the same two in
//! the same order. Deterministic: given the same inputs, every run is identical,
//! which is what makes it a debugger for the protocol rather than just a demo.
//!
//! UI-agnostic by design (pure logic, no rendering deps), so the same core
//! drives a desktop window today and a zero-install web build later.

use personal_rns::engine::{ingest, tick, EngineState, InboundPacket, InstantMillis};

/// A simulated node: a label, its engine state, and the packets the wire has
/// delivered to it but not yet ingested.
pub struct SimNode {
    pub label: String,
    pub state: EngineState,
    inbound: Vec<(InstantMillis, Vec<u8>)>,
}

impl SimNode {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            state: EngineState::default(),
            inbound: Vec::new(),
        }
    }

    pub fn tick_count(&self) -> u64 {
        self.state.tick_count()
    }

    pub fn ingested_packet_count(&self) -> u64 {
        self.state.ingested_packet_count()
    }
}

/// A packet traveling the wire toward node `to`, due at `deliver_at_ms`.
#[derive(Clone)]
pub struct InFlight {
    pub to: usize,
    /// The originating node, or `None` for an externally injected packet.
    pub source: Option<usize>,
    pub deliver_at_ms: u64,
    pub bytes: Vec<u8>,
}

/// The whole simulation: a virtual clock, the nodes, and packets in flight.
pub struct Sim {
    pub now_ms: u64,
    pub nodes: Vec<SimNode>,
    pub in_flight: Vec<InFlight>,
    tick_ms: u64,
    latency_ms: u64,
}

impl Sim {
    /// A sim with one node per label, advancing `tick_ms` of virtual time per
    /// step, and delivering wire packets after `latency_ms`.
    pub fn new(labels: &[&str], tick_ms: u64, latency_ms: u64) -> Self {
        Self {
            now_ms: 0,
            nodes: labels.iter().map(|label| SimNode::new(*label)).collect(),
            in_flight: Vec::new(),
            tick_ms,
            latency_ms,
        }
    }

    /// Inject a packet from outside toward node `to`; it arrives after one wire
    /// latency. (Until the engine emits packets, this is how the wire gets
    /// traffic — e.g. seeding a real announce.)
    pub fn inject(&mut self, to: usize, bytes: Vec<u8>) {
        self.in_flight.push(InFlight {
            to,
            source: None,
            deliver_at_ms: self.now_ms + self.latency_ms,
            bytes,
        });
    }

    /// Advance one step: move the clock, deliver every due wire packet into its
    /// recipient's inbound queue, then `ingest` + `tick` every node.
    pub fn step_engine(&mut self) {
        self.now_ms += self.tick_ms;
        let now = InstantMillis(self.now_ms);

        let mut i = 0;
        while i < self.in_flight.len() {
            if self.in_flight[i].deliver_at_ms <= self.now_ms {
                let packet = self.in_flight.swap_remove(i);
                self.nodes[packet.to].inbound.push((now, packet.bytes));
            } else {
                i += 1;
            }
        }

        for node in &mut self.nodes {
            let inbound = core::mem::take(&mut node.inbound);
            let batch: Vec<InboundPacket> = inbound
                .iter()
                .map(|(arrival, bytes)| InboundPacket {
                    arrival: *arrival,
                    bytes: bytes.as_slice(),
                })
                .collect();
            // The outputs carry no emitted packets yet; once the engine emits,
            // step collects them here and routes them across the wire.
            let _ = ingest(&mut node.state, &batch);
            let _ = tick(&mut node.state, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_deterministically_across_runs() {
        let run = || {
            let mut sim = Sim::new(&["n0", "n1", "n2"], 100, 50);
            sim.inject(1, vec![1, 2, 3]);
            for _ in 0..5 {
                sim.step_engine();
            }
            sim
        };
        let a = run();
        let b = run();
        for (na, nb) in a.nodes.iter().zip(&b.nodes) {
            assert_eq!(na.tick_count(), nb.tick_count());
            assert_eq!(na.ingested_packet_count(), nb.ingested_packet_count());
        }
    }

    #[test]
    fn injected_packet_is_delivered_after_latency_and_ingested() {
        let mut sim = Sim::new(&["n0", "n1"], 100, 50);
        sim.inject(1, vec![0xAA]);

        // Latency 50 < first step's 100ms, so it lands on step 1.
        sim.step_engine();
        assert_eq!(sim.nodes[1].ingested_packet_count(), 1);
        assert_eq!(sim.nodes[0].ingested_packet_count(), 0);
        assert_eq!(sim.nodes[0].tick_count(), 1);

        // No further injection: counts hold, ticks keep climbing.
        sim.step_engine();
        assert_eq!(sim.nodes[1].ingested_packet_count(), 1);
        assert_eq!(sim.nodes[1].tick_count(), 2);
    }
}
