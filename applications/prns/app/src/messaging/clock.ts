import { prns_groups_core as protocol } from "@prns/app-bindings";

export function deviceClock(): protocol.GroupClockReading {
  // Native retry/clock checks remain authoritative and can refuse an incorrect
  // device clock. This display/execution adapter owns no mutation machinery.
  const now = BigInt(Math.floor(Date.now() / 1000));
  return new protocol.GroupClockReading.Trusted({ lower: now, upper: now });
}
