import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import type { PropsWithChildren } from "react";
import { prns_chat_app as app, prns_service_runtime as native } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners, prns_identities as identities } from "@prns/app-bindings";
import * as platform from "@prns/app-platform";
import { useViewState, navigationDefault, decodeNavigation } from "../view-state";
import { identityKey } from "../identity-reference";
import { useForeground } from "../use-foreground";
import { useIdentityActivities } from "./identity-activity";
import type { IdentityActivity } from "./identity-activity";
import { useNetworkActivity } from "./network-activity";
import type { NetworkActivity } from "./network-activity";
import { readPresentationResetPending } from "./reset-presentation";
import { useInstallationReset } from "./installation-reset";

function message(error: unknown): string { return error instanceof Error ? error.message : String(error); }
export type Installation = {
  owner: owners.ServiceRuntimeLike | undefined;
  status: native.ServiceRuntimeStatus | undefined;
  network: native.ServiceRuntimeNetworkStatus | undefined;
  networkKey: string | undefined;
  networkActivity: NetworkActivity;
  clearNetworkActivity: () => void;
  clearDiscoveredPeople?: (() => void) | undefined;
  mode: platform.InstallationMode | undefined;
  bluetooth: boolean;
  busy: "start" | "stop" | undefined;
  error: string | undefined;
  records: readonly identities.IdentityRecord[];
  contactsRevision: number;
  contactSaved: () => void;
  activity: Readonly<Record<string, IdentityActivity>>;
  catalogError: string | undefined;
  selected: string | undefined;
  selectionReady: boolean;
  select: (key: string) => void;
  reloadIdentities: () => Promise<void>;
  start: (mode: platform.InstallationMode, bluetooth: boolean) => Promise<void>;
  startImported: (reviewId: string, bluetooth: boolean) => Promise<void>;
  stop: () => Promise<void>;
  resetPending: boolean;
  blockForReset: () => void;
  resetFinished: () => void;
  reset: ReturnType<typeof useInstallationReset>;
  presentationEpoch: number;
  retryInitialization: () => void;
};
const Context = createContext<Installation | undefined>(undefined);
export function useInstallation(): Installation {
  const value = useContext(Context);
  if (value === undefined) throw new Error("The installation provider is missing");
  return value;
}

/** Process ownership is native. Route and component lifetimes never close it. */
export function InstallationProvider({ children }: PropsWithChildren) {
  const [contactsRevision, setContactsRevision] = useState(0);
  const contactSaved = useCallback(() => setContactsRevision((value) => value + 1), []);
  const [owner, setOwner] = useState<owners.ServiceRuntimeLike>();
  const [status, setStatus] = useState<native.ServiceRuntimeStatus>();
  const [network, setNetwork] = useState<native.ServiceRuntimeNetworkStatus>();
  const [mode, setMode] = useState<platform.InstallationMode>();
  const [bluetooth, setBluetooth] = useState(false);
  const [busy, setBusy] = useState<"start" | "stop">();
  const [error, setError] = useState<string>();
  const [resetPending, setResetPending] = useState(false);
  const [presentationEpoch, setPresentationEpoch] = useState(0);
  // Admission stays closed until the reset journal is known readable. A deep
  // link must not bypass a failed or unfinished startup storage check.
  const resetFence = useRef(true);
  const [initializationAttempt, setInitializationAttempt] = useState(0);
  const navigation = useViewState("prns.ui.navigation.v1", navigationDefault, decodeNavigation);
  const mounted = useRef(true);
  const generation = useRef(0);
  const operation = useRef<"start" | "stop" | undefined>(undefined);
  const retainedOwner = useRef<owners.ServiceRuntimeLike | undefined>(undefined);
  const observe = useCallback(() => {
    if (!mounted.current) return;
    try {
      const retained = operation.current === "start" ? app.appOwner() : retainedOwner.current ?? app.appOwner();
      const snapshot = retained.status();
      // A previous closed owner can remain published while its replacement is
      // being constructed. It is not the new Start's owner or completion.
      if (operation.current === "start" && snapshot.cleanupComplete) return;
      if (retainedOwner.current !== retained) {
        retainedOwner.current = retained;
        setOwner(retained);
      }
      setStatus(snapshot);
      setNetwork(retained.networkStatus());
    } catch { /* Construction has not published its owner yet. */ }
  }, []);
  const begin = useCallback(async (selectedMode: platform.InstallationMode, bluetooth: boolean, reviewId?: string) => {
    if (operation.current !== undefined || resetFence.current) return;
    const ticket = ++generation.current;
    operation.current = "start";
    // Restarting connections keeps the local owner and its accepted work.
    // Only a genuinely closed/missing installation needs a fresh UI binding.
    if (retainedOwner.current === undefined || retainedOwner.current.status().cleanupComplete) {
      retainedOwner.current = undefined;
      setOwner(undefined);
      setStatus(undefined);
      setNetwork(undefined);
    }
    setBusy("start");
    setError(undefined);
    try {
      const readiness = reviewId === undefined ? platform.start(selectedMode, bluetooth) : platform.startImported(reviewId, bluetooth);
      observe(); // Publication and readiness are separate native facts.
      await readiness;
      if (mounted.current && ticket === generation.current) { observe(); setMode("open"); }
    } catch (cause) {
      if (mounted.current && ticket === generation.current) {
        setError(message(cause)); observe();
        // Creation can publish part of the installation before reporting an
        // error. Subsequent attempts must use strict Open, never repeat Create.
        try {
          const present = await platform.installationPresent();
          if (mounted.current && ticket === generation.current) setMode(present ? "open" : "create");
        } catch { /* Preserve the original startup failure and retained owner. */ }
      }
    } finally {
      if (ticket === generation.current) {
        operation.current = undefined;
        if (mounted.current) setBusy(undefined);
      }
    }
  }, [observe]);
  const start = useCallback((selectedMode: platform.InstallationMode, bluetooth: boolean) => begin(selectedMode, bluetooth), [begin]);
  const startImported = useCallback((reviewId: string, bluetooth: boolean) => begin("create", bluetooth, reviewId), [begin]);
  useEffect(() => {
    let cancelled = false;
    mounted.current = true;
    void Promise.all([platform.installationPresent(), platform.bluetoothSelected(), readPresentationResetPending()]).then(([present, selected, pendingReset]) => {
      if (cancelled) return;
      setMode(present ? "open" : "create");
      setBluetooth(selected);
      resetFence.current = pendingReset;
      setResetPending(pendingReset);
      if (present && !pendingReset) void start("open", selected);
    }).catch((cause: unknown) => { if (!cancelled) setError(message(cause)); });
    const timer = setInterval(() => {
      if (operation.current !== undefined || retainedOwner.current !== undefined) observe();
    }, 250);
    return () => { cancelled = true; mounted.current = false; clearInterval(timer); };
  }, [start, observe, initializationAttempt]); // UI loss never stops native ownership.
  const stop = async () => {
    if (operation.current === "stop") return;
    // Stop supersedes this UI's Start waiter even before owner publication.
    // The native platform barrier owns fencing and actual cleanup.
    const ticket = ++generation.current;
    operation.current = "stop";
    setBusy("stop");
    setError(undefined);
    try {
      await platform.stop();
      const present = await platform.installationPresent();
      if (mounted.current && ticket === generation.current) {
        observe();
        setMode(present ? "open" : "create");
      }
    } catch (cause) {
      if (mounted.current && ticket === generation.current) { setError(message(cause)); observe(); }
    } finally {
      if (ticket === generation.current) {
        operation.current = undefined;
        if (mounted.current) setBusy(undefined);
      }
    }
  };
  const readyOwner = status?.phase === native.ServiceRuntimePhase.Ready ? owner : undefined;
  const [records, setRecords] = useState<readonly identities.IdentityRecord[]>([]);
  const [catalogError, setCatalogError] = useState<string>();
  const catalogEpoch = useRef(0);
  const reloadIdentities = useCallback(async () => {
    const ticket = ++catalogEpoch.current;
    if (readyOwner === undefined) return;
    try {
      const rows = await readyOwner.identities().identities();
      if (mounted.current && ticket === catalogEpoch.current) { setRecords(rows); setCatalogError(undefined); }
    } catch (cause) {
      if (mounted.current && ticket === catalogEpoch.current) setCatalogError(message(cause));
    }
  }, [readyOwner]);
  useEffect(() => {
    setRecords([]); setCatalogError(undefined);
    if (readyOwner === undefined) return;
    let reading = false;
    const read = async () => { if (reading) return; reading = true; try { await reloadIdentities(); } finally { reading = false; } };
    void read();
    const timer = setInterval(() => { void read(); }, 1500);
    return () => { clearInterval(timer); catalogEpoch.current++; };
  }, [readyOwner, reloadIdentities]);
  const selected = records.some((record) => identityKey(record) === navigation.value.identity)
    ? navigation.value.identity : records[0] === undefined ? undefined : identityKey(records[0]);
  const select = useCallback((identity: string) => navigation.change((previous) => ({ ...previous, identity })), [navigation.change]);
  const foreground = useForeground();
  const activity = useIdentityActivities(readyOwner, records, foreground);
  const networkKey = network !== undefined && network.phase === native.ServiceRuntimeNetworkPhase.Ready ? network.generation.toString() : undefined;
  const observations = useNetworkActivity(readyOwner, networkKey);
  const blockForReset = useCallback(() => { resetFence.current = true; setResetPending(true); }, []);
  const resetFinished = useCallback(() => {
    generation.current++; catalogEpoch.current++;
    operation.current = undefined; retainedOwner.current = undefined;
    setOwner(undefined); setStatus(undefined); setNetwork(undefined); setRecords([]);
    setBusy(undefined); setError(undefined); setCatalogError(undefined);
    setMode("create"); setBluetooth(false);
    setPresentationEpoch((value) => value + 1);
    navigation.discard();
    resetFence.current = false; setResetPending(false);
  }, [navigation.discard]);
  const reset = useInstallationReset(resetPending, blockForReset, resetFinished);
  return <Context.Provider value={{ owner: readyOwner, status, network, networkKey, networkActivity: observations.activity, clearNetworkActivity: observations.clearRawEvents, clearDiscoveredPeople: observations.clearDiscoveries,
    mode, bluetooth, busy, error, records, catalogError,
    selected, selectionReady: navigation.ready, select, contactsRevision, contactSaved, activity, reloadIdentities, start, startImported, stop,
    resetPending, blockForReset, resetFinished, reset, presentationEpoch,
    retryInitialization: () => { if (mode === undefined && operation.current === undefined) { setError(undefined); setInitializationAttempt((value) => value + 1); } } }}>{children}</Context.Provider>;
}
