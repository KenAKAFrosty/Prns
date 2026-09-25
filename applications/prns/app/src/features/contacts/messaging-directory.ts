import * as Bindings from "@prns-internal/expo";
import type { ContactListOutcome, LxmfPeerSummary } from "@prns-internal/expo";
import { useCallback, useEffect, useRef, useState } from "react";

import { useContactRuntime } from "@/native/contact-runtime-context";
import { useDevelopmentRuntime } from "@/native/development-runtime-context";

export type DirectoryFailure = string | Bindings.NativeStoragePreparationError;

/** Saved contacts remain available independently of the running discovery generation. */
export function useMessagingDirectory() {
  const { runtime } = useContactRuntime();
  const development = useDevelopmentRuntime();
  const { listLxmfPeers } = development;
  const nodeRunning =
    development.phase === "ready" &&
    development.snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Running;
  const generation = development.snapshot?.generationId;
  const revision = development.snapshot?.revision;
  const [outcome, setOutcome] = useState<ContactListOutcome | null>(null);
  const [failure, setFailure] = useState<DirectoryFailure | null>(null);
  const [pending, setPending] = useState(false);
  const [peers, setPeers] = useState<readonly LxmfPeerSummary[]>([]);
  const [discoveryFailure, setDiscoveryFailure] = useState<string | null>(null);
  const [discoveryPending, setDiscoveryPending] = useState(false);
  const contactsRequest = useRef(0);
  const peersRequest = useRef(0);

  const refreshContacts = useCallback(async () => {
    if (runtime === null) return;
    const request = ++contactsRequest.current;
    setPending(true);
    setFailure(null);
    try {
      const next = await runtime.listContacts();
      if (request === contactsRequest.current) {
        setOutcome(next);
        if (next.tag === Bindings.ContactListOutcome_Tags.DevelopmentResetRequired) {
          setFailure(
            new Bindings.NativeStoragePreparationError(
              Bindings.NativeStoragePreparationOutcome.DevelopmentResetRequired.new({
                reason: next.inner.reason,
              }),
            ),
          );
        } else if (next.tag === Bindings.ContactListOutcome_Tags.DevelopmentUnavailable) {
          setFailure("Contacts could not be loaded. Try again.");
        }
      }
    } catch (error) {
      if (request === contactsRequest.current) {
        setFailure(
          error instanceof Bindings.NativeStoragePreparationError
            ? error
            : "Contacts could not be loaded. Try again.",
        );
      }
    } finally {
      if (request === contactsRequest.current) setPending(false);
    }
  }, [runtime]);

  const refreshDiscovery = useCallback(async () => {
    const request = ++peersRequest.current;
    if (!nodeRunning) {
      setPeers([]);
      setDiscoveryFailure(null);
      setDiscoveryPending(false);
      return;
    }
    setDiscoveryPending(true);
    const result = await listLxmfPeers();
    if (request !== peersRequest.current) return;
    if (
      result.type === "outcome" &&
      result.outcome.tag === Bindings.LxmfPeerListOutcome_Tags.Listed
    ) {
      setPeers(result.outcome.inner.peers);
      setDiscoveryFailure(null);
    } else {
      setPeers([]);
      setDiscoveryFailure("Discovered contacts could not be refreshed. Try again.");
    }
    setDiscoveryPending(false);
  }, [listLxmfPeers, nodeRunning]);

  useEffect(() => {
    void refreshContacts();
    return () => {
      contactsRequest.current += 1;
    };
  }, [refreshContacts]);

  useEffect(() => {
    // A stopped/replaced generation must never publish a previous peer list.
    void generation;
    void revision;
    void refreshDiscovery();
    return () => {
      peersRequest.current += 1;
    };
  }, [generation, revision, refreshDiscovery]);

  const refresh = async () => {
    await Promise.all([refreshContacts(), refreshDiscovery()]);
  };
  return {
    outcome,
    contacts:
      outcome?.tag === Bindings.ContactListOutcome_Tags.Listed ? outcome.inner.contacts : [],
    failure,
    pending,
    peers,
    discoveryFailure,
    discoveryPending,
    nodeRunning,
    refreshContacts,
    refreshDiscovery,
    refresh,
  };
}

export function lastHeardLabel(ageMillis: bigint): string {
  if (ageMillis < 60_000n) return "Heard just now";
  if (ageMillis < 3_600_000n) {
    const minutes = ageMillis / 60_000n;
    return `Heard ${minutes} ${minutes === 1n ? "minute" : "minutes"} ago`;
  }
  const hours = ageMillis / 3_600_000n;
  return `Heard ${hours} ${hours === 1n ? "hour" : "hours"} ago`;
}
