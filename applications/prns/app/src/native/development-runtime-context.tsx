import type {
  ContactMutationOutcome,
  CancelLxmfMessageOutcome,
  DescribeRemoteControlTargetInput,
  DevelopmentNodeSnapshot,
  DevelopmentRuntime,
  DevelopmentRuntimeFailure,
  DevelopmentRuntimeSession,
  InitiateRemoteControlPairingInput,
  ListLxmfMessagesInput,
  LxmfMessageListOutcome,
  LxmfPeerListOutcome,
  MeasureLxmfTextInput,
  MeasureLxmfTextOutcome,
  AnnounceLxmfOutcome,
  RemoteControlDescribeOutcome,
  RemoteControlPairingCommandOutcome,
  RemoteControlPairingDecisionInput,
  RetryLxmfMessageOutcome,
  SendDirectTextInput,
  SendDirectTextOutcome,
} from "@prns-internal/expo";
import type { DestinationHash } from "personal-rns/contract";
import { Effect, Exit, Scope } from "effect";
import {
  createContext,
  type PropsWithChildren,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { runtimeProvider } from "@/native/runtime-provider";
import type { RuntimeProvider } from "./runtime-provider.types";

export type RuntimeCommandResult<Outcome> =
  | { readonly type: "outcome"; readonly outcome: Outcome }
  | { readonly type: "operationFailure"; readonly detail: string };

export type DevelopmentRuntimeView = {
  readonly availability: RuntimeProvider["availability"];
  readonly phase: "unavailable" | "starting" | "ready" | "failed";
  readonly snapshot: DevelopmentNodeSnapshot | null;
  readonly lifecycleFailure: string | null;
  readonly backgroundFailure: string | null;
  readonly refreshSnapshot: () => Promise<RuntimeCommandResult<DevelopmentNodeSnapshot>>;
  readonly initiatePairing: (
    input: InitiateRemoteControlPairingInput,
  ) => Promise<RuntimeCommandResult<RemoteControlPairingCommandOutcome>>;
  readonly approvePairing: (
    input: RemoteControlPairingDecisionInput,
  ) => Promise<RuntimeCommandResult<RemoteControlPairingCommandOutcome>>;
  readonly rejectPairing: (
    input: RemoteControlPairingDecisionInput,
  ) => Promise<RuntimeCommandResult<RemoteControlPairingCommandOutcome>>;
  readonly describeTarget: (
    input: DescribeRemoteControlTargetInput,
  ) => Promise<RuntimeCommandResult<RemoteControlDescribeOutcome>>;
  readonly saveObservedDestination: (
    destination: DestinationHash,
  ) => Promise<RuntimeCommandResult<ContactMutationOutcome>>;
  readonly listLxmfPeers: () => Promise<RuntimeCommandResult<LxmfPeerListOutcome>>;
  readonly listLxmfMessages: (
    input: ListLxmfMessagesInput,
  ) => Promise<RuntimeCommandResult<LxmfMessageListOutcome>>;
  readonly retryLxmfMessage: (
    localRecordId: bigint,
  ) => Promise<RuntimeCommandResult<RetryLxmfMessageOutcome>>;
  readonly cancelLxmfMessage: (
    localRecordId: bigint,
  ) => Promise<RuntimeCommandResult<CancelLxmfMessageOutcome>>;
  readonly announceLxmf: () => Promise<RuntimeCommandResult<AnnounceLxmfOutcome>>;
  readonly measureLxmfText: (
    input: MeasureLxmfTextInput,
  ) => Promise<RuntimeCommandResult<MeasureLxmfTextOutcome>>;
  readonly sendDirectText: (
    input: SendDirectTextInput,
  ) => Promise<RuntimeCommandResult<SendDirectTextOutcome>>;
};

type DevelopmentRuntimeProviderProps = PropsWithChildren<{
  readonly provider?: RuntimeProvider;
  readonly refreshIntervalMillis?: number;
}>;

const DevelopmentRuntimeContext = createContext<DevelopmentRuntimeView | null>(null);

export function DevelopmentRuntimeProvider({
  children,
  provider,
  refreshIntervalMillis = 750,
}: DevelopmentRuntimeProviderProps) {
  const selectedProvider = provider ?? runtimeProvider;
  const [phase, setPhase] = useState<DevelopmentRuntimeView["phase"]>(
    selectedProvider.availability.type === "available" ? "starting" : "unavailable",
  );
  const [snapshot, setSnapshot] = useState<DevelopmentNodeSnapshot | null>(null);
  const [lifecycleFailure, setLifecycleFailure] = useState<string | null>(null);
  const [backgroundFailure, setBackgroundFailure] = useState<string | null>(null);
  const session = useRef<DevelopmentRuntimeSession | null>(null);
  const latestRevision = useRef<bigint | null>(null);

  const publishSnapshot = useCallback((next: DevelopmentNodeSnapshot) => {
    if (latestRevision.current !== null && next.revision <= latestRevision.current) {
      return;
    }
    latestRevision.current = next.revision;
    setSnapshot(next);
  }, []);

  useEffect(() => {
    session.current = null;
    latestRevision.current = null;
    setSnapshot(null);
    setLifecycleFailure(null);
    setBackgroundFailure(null);

    if (!("acquire" in selectedProvider)) {
      setPhase("unavailable");
      return;
    }

    const availableProvider = selectedProvider;
    setPhase("starting");
    let mounted = true;
    const scope = Scope.makeUnsafe("sequential");
    const acquisition = availableProvider
      .acquire({
        refreshIntervalMillis,
        onSnapshot: (next) => {
          if (mounted) {
            publishSnapshot(next);
          }
        },
        onBackgroundFailure: (failure) => {
          if (mounted) {
            setBackgroundFailure(formatFailure(failure));
          }
        },
      })
      .pipe(Scope.provide(scope));

    void Effect.runPromise(acquisition).then(
      (acquired) => {
        if (!mounted) {
          return;
        }
        session.current = acquired;
        publishSnapshot(acquired.initialSnapshot);
        setPhase("ready");
      },
      (failure: unknown) => {
        if (!mounted) {
          return;
        }
        setLifecycleFailure(formatFailure(failure));
        setPhase("failed");
      },
    );

    return () => {
      mounted = false;
      session.current = null;
      void Effect.runPromise(Scope.close(scope, Exit.void));
    };
  }, [publishSnapshot, refreshIntervalMillis, selectedProvider]);

  const unavailableResult = useCallback(
    <Outcome,>(): RuntimeCommandResult<Outcome> => ({
      type: "operationFailure",
      detail:
        selectedProvider.availability.type === "unavailable"
          ? `The ${selectedProvider.availability.platform} native runtime is not implemented.`
          : "The native development node has not finished starting.",
    }),
    [selectedProvider.availability],
  );

  const run = useCallback(
    async <Outcome,>(
      operation: (active: DevelopmentRuntimeSession) => Effect.Effect<Outcome, unknown>,
    ): Promise<RuntimeCommandResult<Outcome>> => {
      const active = session.current;
      if (active === null) {
        return unavailableResult();
      }
      try {
        return { type: "outcome", outcome: await Effect.runPromise(operation(active)) };
      } catch (failure) {
        return { type: "operationFailure", detail: formatFailure(failure) };
      }
    },
    [unavailableResult],
  );

  const refreshSnapshot = useCallback(async () => {
    const result = await run((active) => active.runtime.readDevelopmentNodeSnapshot);
    if (result.type === "outcome") {
      publishSnapshot(result.outcome);
    }
    return result;
  }, [publishSnapshot, run]);

  const initiatePairing = useCallback(
    async (input: InitiateRemoteControlPairingInput) => {
      const result = await run((active) => active.runtime.initiateRemoteControlPairing(input));
      if (result.type === "outcome" && result.outcome.type === "accepted") {
        publishSnapshot(result.outcome.snapshot);
      }
      return result;
    },
    [publishSnapshot, run],
  );

  const approvePairing = useCallback(
    async (input: RemoteControlPairingDecisionInput) => {
      const result = await run((active) => active.runtime.approveRemoteControlPairing(input));
      if (result.type === "outcome" && result.outcome.type === "accepted") {
        publishSnapshot(result.outcome.snapshot);
      }
      return result;
    },
    [publishSnapshot, run],
  );

  const rejectPairing = useCallback(
    async (input: RemoteControlPairingDecisionInput) => {
      const result = await run((active) => active.runtime.rejectRemoteControlPairing(input));
      if (result.type === "outcome" && result.outcome.type === "accepted") {
        publishSnapshot(result.outcome.snapshot);
      }
      return result;
    },
    [publishSnapshot, run],
  );

  const describeTarget = useCallback(
    async (input: DescribeRemoteControlTargetInput) => {
      const result = await run((active) => active.runtime.describeRemoteControlTarget(input));
      if (result.type === "outcome" && result.outcome.type === "described") {
        publishSnapshot(result.outcome.snapshot);
      }
      return result;
    },
    [publishSnapshot, run],
  );

  const saveObservedDestination = useCallback(
    async (destination: DestinationHash): Promise<RuntimeCommandResult<ContactMutationOutcome>> => {
      if (!("runtime" in selectedProvider) || session.current === null) {
        return unavailableResult();
      }
      try {
        return {
          type: "outcome",
          outcome: await selectedProvider.runtime.saveObservedDestination(destination),
        };
      } catch (failure) {
        return { type: "operationFailure", detail: formatFailure(failure) };
      }
    },
    [selectedProvider, unavailableResult],
  );

  const runAvailable = useCallback(
    async <Outcome,>(
      operation: (runtime: DevelopmentRuntime) => Promise<Outcome>,
    ): Promise<RuntimeCommandResult<Outcome>> => {
      if (!("runtime" in selectedProvider)) {
        return unavailableResult();
      }
      try {
        return { type: "outcome", outcome: await operation(selectedProvider.runtime) };
      } catch (failure) {
        return { type: "operationFailure", detail: formatFailure(failure) };
      }
    },
    [selectedProvider, unavailableResult],
  );

  const runGenerationBound = useCallback(
    async <Outcome,>(
      operation: (runtime: DevelopmentRuntime) => Promise<Outcome>,
    ): Promise<RuntimeCommandResult<Outcome>> => {
      if (session.current === null) {
        return unavailableResult();
      }
      return runAvailable(operation);
    },
    [runAvailable, unavailableResult],
  );

  const listLxmfPeers = useCallback(
    () => runGenerationBound((runtime) => runtime.listLxmfPeers()),
    [runGenerationBound],
  );

  const listLxmfMessages = useCallback(
    (input: ListLxmfMessagesInput) => runAvailable((runtime) => runtime.listLxmfMessages(input)),
    [runAvailable],
  );

  const retryLxmfMessage = useCallback(
    (localRecordId: bigint) => runAvailable((runtime) => runtime.retryLxmfMessage(localRecordId)),
    [runAvailable],
  );

  const cancelLxmfMessage = useCallback(
    (localRecordId: bigint) => runAvailable((runtime) => runtime.cancelLxmfMessage(localRecordId)),
    [runAvailable],
  );

  const announceLxmf = useCallback(
    () => runGenerationBound((runtime) => runtime.announceLxmf()),
    [runGenerationBound],
  );

  const measureLxmfText = useCallback(
    (input: MeasureLxmfTextInput) =>
      runGenerationBound((runtime) => runtime.measureLxmfText(input)),
    [runGenerationBound],
  );

  const sendDirectText = useCallback(
    (input: SendDirectTextInput) => runGenerationBound((runtime) => runtime.sendDirectText(input)),
    [runGenerationBound],
  );

  const value = useMemo<DevelopmentRuntimeView>(
    () => ({
      availability: selectedProvider.availability,
      phase,
      snapshot,
      lifecycleFailure,
      backgroundFailure,
      refreshSnapshot,
      initiatePairing,
      approvePairing,
      rejectPairing,
      describeTarget,
      saveObservedDestination,
      listLxmfPeers,
      listLxmfMessages,
      retryLxmfMessage,
      cancelLxmfMessage,
      announceLxmf,
      measureLxmfText,
      sendDirectText,
    }),
    [
      approvePairing,
      backgroundFailure,
      describeTarget,
      initiatePairing,
      lifecycleFailure,
      phase,
      refreshSnapshot,
      rejectPairing,
      saveObservedDestination,
      listLxmfPeers,
      listLxmfMessages,
      retryLxmfMessage,
      cancelLxmfMessage,
      announceLxmf,
      measureLxmfText,
      sendDirectText,
      selectedProvider.availability,
      snapshot,
    ],
  );

  return (
    <DevelopmentRuntimeContext.Provider value={value}>
      {children}
    </DevelopmentRuntimeContext.Provider>
  );
}

export function useDevelopmentRuntime(): DevelopmentRuntimeView {
  const value = useContext(DevelopmentRuntimeContext);
  if (value === null) {
    throw new Error("useDevelopmentRuntime must be used inside DevelopmentRuntimeProvider");
  }
  return value;
}

function formatFailure(failure: DevelopmentRuntimeFailure | unknown): string {
  if (
    failure !== null &&
    typeof failure === "object" &&
    "detail" in failure &&
    typeof failure.detail === "string" &&
    failure.detail.length > 0
  ) {
    return failure.detail;
  }
  if (failure instanceof Error) {
    return failure.message.length > 0 ? failure.message : failure.name;
  }
  return String(failure);
}
