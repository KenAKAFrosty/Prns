# HaLoW simulation qualification

This slice exclusively hardens the native HaLoW software transport before the
next G4/Heltec desk session. It runs production nodes, source-MAC peer admission,
shared broadcast egress, direct unicast and authenticated Remote Control. Only
the datagram medium and its completion/failure boundaries are simulated. There
is no RF, firmware, range, throughput or installer qualification claim.

- [x] Bounded source/destination-aware datagram medium, directed reachability,
      delay/duplication/loss, manual time, stale-radio ownership and full traces.
- [x] Real-node replay across chain, shared-medium and asymmetric topologies;
      broadcast announces, unicast data, echo suppression and relay removal.
- [x] Peer cap, expiry/re-admission, receive pressure, stalled/failed sends,
      fatal receive teardown and explicit replacement, with unaffected-neighbor
      progress and complete cleanup.
- [x] Authenticated inspection and bounded app messages through actual HaLoW
      while Resources overlap; unauthorized admission remains silent.
- [x] Seeded repeated campaigns, bounded traces/resource checks, replayable
      inputs and retained failure evidence; focused owner and integration checks.

Use typed policies and explicit budgets. Avoid introducing shipping simulation
hooks, radio configuration mutation, automatic announcements or an independent
transport protocol. Findings require an independent correctness rationale before
changing production behavior. Automatic packet-socket rebinding is currently
absent; qualification must distinguish explicit replacement from auto-recovery.

All milestones are complete. The [qualification record](measurements/halow-qualification.md)
owns commands, measurements, replay formats and remaining hardware boundaries.
