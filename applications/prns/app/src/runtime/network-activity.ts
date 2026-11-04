import { useCallback, useEffect, useRef, useState } from "react";
import { prns_host_uniffi as host } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";

import { discoverContact, retainDiscoveries } from "./contact-discovery";
import type { DiscoveredContact } from "./contact-discovery";

export type HeardAnnouncement = {
  id: bigint; observedAt: number;
  event: Extract<host.DiagnosticEvent, { tag: host.DiagnosticEvent_Tags.AnnounceHeard }>;
};
export type NetworkActivity = {
  generation: string | undefined; announcements: readonly HeardAnnouncement[];
  evicted: number; dropped: bigint; error: string | undefined;
  discovered: readonly DiscoveredContact[]; discoveryError: string | undefined;
};
const empty: NetworkActivity = { generation: undefined, announcements: [], evicted: 0, dropped: 0n, error: undefined, discovered: [], discoveryError: undefined };
const windowSize = 200;

/** One bounded UI projection over the existing native diagnostic stream. Native
 * drops remain visible; this window is not delivery or an exhaustive audit log. */
export function useNetworkActivity(owner: owners.ServiceRuntimeLike | undefined, generation: string | undefined) {
  const [activity, setActivity] = useState<NetworkActivity>(empty);
  const rawEpoch = useRef(0);
  const discoveryEpoch = useRef(0);
  const clearRawEvents = useCallback(() => {
    rawEpoch.current++;
    setActivity((previous) => ({ ...previous, announcements: [], evicted: 0 }));
  }, []);
  const clearDiscoveries = useCallback(() => {
    discoveryEpoch.current++;
    setActivity((previous) => ({ ...previous, discovered: [], discoveryError: undefined }));
  }, []);
  useEffect(() => {
    if (owner === undefined || generation === undefined) { setActivity(empty); return; }
    const abort = new AbortController();
    setActivity({ ...empty, generation });
    let stream: host.EventStreamLike | undefined;
    let streamClosed = false;
    const closeStream = () => {
      if (stream === undefined || streamClosed) return;
      streamClosed = true;
      stream.closeStream();
    };
    let sequence = 0n;
    const expiry = setInterval(() => setActivity((previous) => {
      if (previous.generation !== generation) return previous;
      const discovered = retainDiscoveries(previous.discovered, [], Date.now());
      return discovered.length === previous.discovered.length ? previous : { ...previous, discovered };
    }), 60_000);
    void (async () => {
      try {
        const client = owner.host();
        stream = client.diagnostics();
        if (abort.signal.aborted) return;
        while (!abort.signal.aborted && await stream.ready({ signal: abort.signal })) {
          const batch: HeardAnnouncement[] = [];
          const rawTicket = rawEpoch.current;
          const discoveryTicket = discoveryEpoch.current;
          let dropped = 0n;
          for (let count = 0; count < 64; count++) {
            const event = stream.tryNext();
            if (event === undefined) break;
            if (event.tag !== host.HostEvent_Tags.Diagnostic) continue;
            const diagnostic = event.inner.event;
            if (diagnostic.tag === host.DiagnosticEvent_Tags.AnnounceHeard) batch.push({ id: ++sequence, observedAt: Date.now(), event: diagnostic });
            else if (diagnostic.tag === host.DiagnosticEvent_Tags.DiagnosticsDropped) {
              dropped += (diagnostic.inner.count.high << 64n) | diagnostic.inner.count.low;
            }
          }
          if (abort.signal.aborted) return;
          if (batch.length > 0 || dropped > 0n) setActivity((previous) => {
            if (abort.signal.aborted || previous.generation !== generation) return previous;
            if (rawTicket !== rawEpoch.current) return dropped > 0n ? { ...previous, dropped: previous.dropped + dropped } : previous;
            const all = [...batch].reverse().concat(previous.announcements);
            return { ...previous, announcements: all.slice(0, windowSize),
              evicted: Math.min(Number.MAX_SAFE_INTEGER, previous.evicted + Math.max(0, all.length - windowSize)), dropped: previous.dropped + dropped };
          });
          const discovered: DiscoveredContact[] = [];
          let discoveryError: string | undefined;
          for (const observation of [...batch].reverse()) {
            if (abort.signal.aborted || discoveryTicket !== discoveryEpoch.current) break;
            try {
              const contact = await discoverContact(observation, (destination) => client.destinationPublicKey(destination, { signal: abort.signal }));
              if (contact !== undefined) discovered.push(contact);
            } catch { discoveryError = "Some contact announcements could not be checked. New announcements will be checked automatically."; }
          }
          if (!abort.signal.aborted && discoveryTicket === discoveryEpoch.current && (discovered.length > 0 || discoveryError !== undefined)) setActivity((previous) => {
            if (abort.signal.aborted || previous.generation !== generation || discoveryTicket !== discoveryEpoch.current) return previous;
            return { ...previous, discovered: retainDiscoveries(previous.discovered, discovered, Date.now()), discoveryError };
          });
          await new Promise<void>((resolve) => setTimeout(resolve, 0));
        }
      } catch (cause) {
        if (!abort.signal.aborted) setActivity((previous) => ({ ...previous, error: String(cause) }));
      } finally {
        closeStream();
        if (stream !== undefined && "uniffiDestroy" in stream && typeof stream.uniffiDestroy === "function") stream.uniffiDestroy();
      }
    })();
    return () => {
      abort.abort();
      // Release the exclusive lane before a replacement effect can claim it;
      // cancellation of the pending native read can finish asynchronously.
      closeStream();
      clearInterval(expiry);
    };
  }, [owner, generation]);
  return { activity, clearRawEvents, clearDiscoveries };
}
