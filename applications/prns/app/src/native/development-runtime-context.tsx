import * as Bindings from "@prns-internal/expo";
import type {
  AnnounceRemoteControlTargetInput,
  RemoteControlAnnounceOutcome,
  AccessorySetupPickerOutcome,
  AccessorySetupStatus,
  AnnounceLxmfOutcome,
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
import { useAndroidRuntime, type AndroidRuntimeView } from "./use-android-runtime";

export type RuntimeCommandResult<Outcome> =
  | { readonly type: "outcome"; readonly outcome: Outcome }
  | {
      readonly type: "operationFailure";
      readonly detail: string;
      readonly storagePreparation?: import("@prns-internal/native-bindings").NativeStoragePreparationOutcome;
    };

export type DevelopmentRuntimeView = {
  readonly announceTarget: (
    input: AnnounceRemoteControlTargetInput,
  ) => Promise<RuntimeCommandResult<RemoteControlAnnounceOutcome>>;
  readonly availability: RuntimeProvider["availability"];
  readonly accessorySetup: AccessorySetupStatus | null;
  readonly accessorySetupFailure: string | null;
  readonly androidRuntime: AndroidRuntimeView | null;
  readonly phase: "unavailable" | "starting" | "ready" | "failed";
  readonly snapshot: DevelopmentNodeSnapshot | null;
  readonly lifecycleFailure: string | null;
  readonly backgroundFailure: string | null;
  readonly canStartNode: boolean;
  readonly startNode: () => void;
  readonly canStopNode: boolean;
  readonly stoppingNode: boolean;
  readonly stopFailure: string | null;
  readonly stopNode: () => Promise<void>;
  readonly showAccessorySetupPicker: () => Promise<
    RuntimeCommandResult<AccessorySetupPickerOutcome>
  >;
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
  readonly refreshActive?: boolean;
}>;

const DevelopmentRuntimeContext = createContext<DevelopmentRuntimeView | null>(null);

export function DevelopmentRuntimeProvider({
  children,
  provider,
  refreshIntervalMillis = 750,
  refreshActive = true,
}: DevelopmentRuntimeProviderProps) {
  const selectedProvider = provider ?? runtimeProvider;
  const androidCapability =
    "androidRuntime" in selectedProvider ? selectedProvider.androidRuntime : undefined;
  const androidRuntime = useAndroidRuntime(androidCapability);
  const [phase, setPhase] = useState<DevelopmentRuntimeView["phase"]>(
    selectedProvider.availability.type === "available" ? "starting" : "unavailable",
  );
  const [snapshot, setSnapshot] = useState<DevelopmentNodeSnapshot | null>(null);
  const [lifecycleFailure, setLifecycleFailure] = useState<string | null>(null);
  const [backgroundFailure, setBackgroundFailure] = useState<string | null>(null);
  const [accessorySetup, setAccessorySetup] = useState<AccessorySetupStatus | null>(null);
  const [accessorySetupFailure, setAccessorySetupFailure] = useState<string | null>(null);
  const [startRequest, setStartRequest] = useState(0);
  const [stoppingNode, setStoppingNode] = useState(false);
  const [stopFailure, setStopFailure] = useState<string | null>(null);
  const stopOwner = useRef<object | null>(null);
  const stopProvider = useRef<RuntimeProvider | null>(null);
  const stopRevision = useRef(0);
  const acquisitionPending = useRef(true);
  const previousRelease = useRef(Promise.resolve());
  const session = useRef<DevelopmentRuntimeSession | null>(null);
  const latestRevision = useRef<bigint | null>(null);
  const refreshActiveState = useRef(refreshActive);
  refreshActiveState.current = refreshActive;
  const canStartNode =
    selectedProvider.availability.type === "available" &&
    selectedProvider.availability.platform === "android" &&
    !stoppingNode &&
    phase !== "starting" &&
    snapshot?.runtime !== Bindings.DevelopmentNodeRuntime.Starting &&
    snapshot?.runtime !== Bindings.DevelopmentNodeRuntime.Stopping &&
    androidRuntime.status?.service !== "starting" &&
    androidRuntime.status?.service !== "stopping" &&
    (phase === "failed" ||
      snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Stopped ||
      snapshot?.runtime === Bindings.DevelopmentNodeRuntime.Failed);
  const startAllowed = useRef(canStartNode);
  startAllowed.current = canStartNode;
  const canStopNode =
    selectedProvider.availability.type === "available" &&
    selectedProvider.availability.platform === "android" &&
    phase !== "starting" &&
    !stoppingNode &&
    ((snapshot !== null && snapshot.runtime !== Bindings.DevelopmentNodeRuntime.Stopped) ||
      (androidRuntime.status !== null && androidRuntime.status.service !== "stopped"));
  const stopAllowed = useRef(canStopNode);
  stopAllowed.current = canStopNode;

  const startNode = useCallback(() => {
    if (!startAllowed.current || acquisitionPending.current || stopOwner.current !== null) return;
    // Admit synchronously, before React renders the pending state, so two presses
    // cannot create two runtime subscriptions or start requests.
    acquisitionPending.current = true;
    setStopFailure(null);
    setPhase("starting");
    setStartRequest((request) => request + 1);
  }, []);
  const accessorySetupAcquisitionState = availableProviderRequiresAccessorySetup(selectedProvider)
    ? accessorySetupFailure !== null || accessorySetup?.phase === "failed"
      ? "failed"
      : accessorySetup?.phase === "ready" &&
          (accessorySetup.nativeStart === "running" ||
            (accessorySetup.picker === "idle" && accessorySetup.nativeStart !== "stopping"))
        ? "ready"
        : "waiting"
    : "notRequired";

  const publishSnapshot = useCallback((next: DevelopmentNodeSnapshot) => {
    if (latestRevision.current !== null && next.revision <= latestRevision.current) {
      return;
    }
    latestRevision.current = next.revision;
    setSnapshot(next);
  }, []);

  useEffect(() => {
    stopProvider.current = selectedProvider;
    stopOwner.current = null;
    setStoppingNode(false);
    setStopFailure(null);
    return () => {
      stopProvider.current = null;
      stopOwner.current = null;
    };
  }, [selectedProvider]);

  const stopNode = useCallback(async () => {
    if (
      !stopAllowed.current ||
      stopProvider.current !== selectedProvider ||
      acquisitionPending.current ||
      stopOwner.current !== null ||
      !("runtime" in selectedProvider)
    )
      return;
    const owner = {};
    stopOwner.current = owner;
    stopRevision.current += 1;
    setStoppingNode(true);
    setStopFailure(null);
    try {
      // Android's Expo stop delegates to the service owner, just like the
      // notification action. It clears restart intent and drains platform work.
      const outcome = await selectedProvider.runtime.stopDevelopmentNode();
      if (stopOwner.current !== owner) return;
      if (outcome.tag === Bindings.DevelopmentNodeStopOutcome_Tags.Failed)
        setStopFailure(outcome.inner.detail);
    } catch (failure) {
      if (stopOwner.current === owner) setStopFailure(formatFailure(failure));
    } finally {
      if (stopOwner.current === owner) {
        try {
          const next = await selectedProvider.runtime.readDevelopmentNodeSnapshot();
          if (stopOwner.current === owner) publishSnapshot(next);
        } catch (failure) {
          if (stopOwner.current === owner) setBackgroundFailure(formatFailure(failure));
        }
        await androidRuntime.refresh();
        if (stopOwner.current === owner) {
          stopOwner.current = null;
          setStoppingNode(false);
        }
      }
    }
  }, [androidRuntime.refresh, publishSnapshot, selectedProvider]);

  useEffect(() => {
    setAccessorySetup(null);
    setAccessorySetupFailure(null);
    if (!("accessorySetup" in selectedProvider) || selectedProvider.accessorySetup === undefined) {
      return;
    }
    const setup = selectedProvider.accessorySetup;
    let mounted = true;
    let latestStatusRevision: number | null = null;
    const publishAccessorySetup = (next: AccessorySetupStatus) => {
      if (!mounted || (latestStatusRevision !== null && next.revision <= latestStatusRevision)) {
        return;
      }
      latestStatusRevision = next.revision;
      setAccessorySetup(next);
      setAccessorySetupFailure(null);
    };
    const subscription = setup.addStatusListener((next) => {
      publishAccessorySetup(next);
    });
    void setup.readStatus().then(
      (next) => {
        publishAccessorySetup(next);
      },
      (failure: unknown) => {
        if (mounted && latestStatusRevision === null) {
          setAccessorySetupFailure(formatFailure(failure));
        }
      },
    );
    return () => {
      mounted = false;
      subscription.remove();
    };
  }, [selectedProvider]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: startRequest explicitly requests a fresh scope after the previous observer scope is closed.
  useEffect(() => {
    acquisitionPending.current = true;
    session.current = null;
    latestRevision.current = null;
    setSnapshot(null);
    setLifecycleFailure(null);
    setBackgroundFailure(null);

    if (!("acquire" in selectedProvider)) {
      setPhase("unavailable");
      return;
    }

    if (availableProviderRequiresAccessorySetup(selectedProvider)) {
      if (accessorySetupAcquisitionState === "failed") {
        setPhase("failed");
        return;
      }
      if (accessorySetupAcquisitionState !== "ready") {
        setPhase("starting");
        return;
      }
    }

    const availableProvider = selectedProvider;
    setPhase("starting");
    let mounted = true;
    const scope = Scope.makeUnsafe("sequential");
    const acquisition = availableProvider
      .acquire({
        refreshIntervalMillis,
        shouldRefresh: () => refreshActiveState.current,
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

    // Reuse the provider's start options and process-owned lifetime. Closing the
    // old scope only releases its observers; a service event never starts a node.
    void previousRelease.current
      .then(async () => (mounted ? Effect.runPromise(acquisition) : null))
      .then(
        (acquired) => {
          if (!mounted || acquired === null) {
            return;
          }
          acquisitionPending.current = false;
          session.current = acquired;
          publishSnapshot(acquired.initialSnapshot);
          setPhase("ready");
        },
        (failure: unknown) => {
          if (!mounted) {
            return;
          }
          acquisitionPending.current = false;
          setLifecycleFailure(formatFailure(failure));
          setPhase("failed");
        },
      );

    return () => {
      mounted = false;
      acquisitionPending.current = true;
      session.current = null;
      previousRelease.current = Promise.all([
        previousRelease.current,
        Effect.runPromise(Scope.close(scope, Exit.void)),
      ]).then(() => undefined);
    };
  }, [
    accessorySetupAcquisitionState,
    publishSnapshot,
    refreshIntervalMillis,
    selectedProvider,
    startRequest,
  ]);

  const unavailableResult = useCallback(
    <Outcome,>(): RuntimeCommandResult<Outcome> => ({
      type: "operationFailure",
      detail:
        selectedProvider.availability.type === "unavailable"
          ? `This feature is not available on ${selectedProvider.availability.platform} yet.`
          : "This device is still getting ready.",
    }),
    [selectedProvider.availability],
  );

  const run = useCallback(
    async <Outcome,>(
      operation: (active: DevelopmentRuntimeSession) => Effect.Effect<Outcome, unknown>,
    ): Promise<RuntimeCommandResult<Outcome>> => {
      const active = session.current;
      const admittedStopRevision = stopRevision.current;
      if (active === null || acquisitionPending.current || stopOwner.current !== null) {
        return unavailableResult();
      }
      let result: RuntimeCommandResult<Outcome>;
      try {
        result = { type: "outcome", outcome: await Effect.runPromise(operation(active)) };
      } catch (failure) {
        result = commandFailure(failure);
      }
      // A manual refresh or command can outlive its observer scope. Its result
      // must not republish an old generation after an explicit start or release.
      return session.current === active &&
        !acquisitionPending.current &&
        admittedStopRevision === stopRevision.current
        ? result
        : {
            type: "operationFailure",
            detail: "This device's node changed while the request was running. Try again.",
          };
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
      if (
        result.type === "outcome" &&
        result.outcome.tag === Bindings.RemoteControlPairingCommandOutcome_Tags.Accepted
      ) {
        publishSnapshot(result.outcome.inner.snapshot);
      }
      return result;
    },
    [publishSnapshot, run],
  );

  const approvePairing = useCallback(
    async (input: RemoteControlPairingDecisionInput) => {
      const result = await run((active) => active.runtime.approveRemoteControlPairing(input));
      if (
        result.type === "outcome" &&
        result.outcome.tag === Bindings.RemoteControlPairingCommandOutcome_Tags.Accepted
      ) {
        publishSnapshot(result.outcome.inner.snapshot);
      }
      return result;
    },
    [publishSnapshot, run],
  );

  const rejectPairing = useCallback(
    async (input: RemoteControlPairingDecisionInput) => {
      const result = await run((active) => active.runtime.rejectRemoteControlPairing(input));
      if (
        result.type === "outcome" &&
        result.outcome.tag === Bindings.RemoteControlPairingCommandOutcome_Tags.Accepted
      ) {
        publishSnapshot(result.outcome.inner.snapshot);
      }
      return result;
    },
    [publishSnapshot, run],
  );

  const describeTarget = useCallback(
    async (input: DescribeRemoteControlTargetInput) => {
      const result = await run((active) => active.runtime.describeRemoteControlTarget(input));
      if (
        result.type === "outcome" &&
        result.outcome.tag === Bindings.RemoteControlDescribeOutcome_Tags.Described
      ) {
        publishSnapshot(result.outcome.inner.snapshot);
      }
      return result;
    },
    [publishSnapshot, run],
  );

  const announceTarget = useCallback(
    async (input: AnnounceRemoteControlTargetInput) => {
      const result = await run((active) => active.runtime.announceRemoteControlTarget(input));
      if (
        result.type === "outcome" &&
        result.outcome.tag === Bindings.RemoteControlAnnounceOutcome_Tags.Accepted
      ) {
        publishSnapshot(result.outcome.inner.snapshot);
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
        return commandFailure(failure);
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
        return commandFailure(failure);
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

  const showAccessorySetupPicker = useCallback(async () => {
    if (!("accessorySetup" in selectedProvider) || selectedProvider.accessorySetup === undefined) {
      return unavailableResult<AccessorySetupPickerOutcome>();
    }
    try {
      return {
        type: "outcome" as const,
        outcome: await selectedProvider.accessorySetup.showPicker(),
      };
    } catch (failure) {
      return { type: "operationFailure" as const, detail: formatFailure(failure) };
    }
  }, [selectedProvider, unavailableResult]);

  const value = useMemo<DevelopmentRuntimeView>(
    () => ({
      availability: selectedProvider.availability,
      accessorySetup,
      accessorySetupFailure,
      androidRuntime: androidCapability === undefined ? null : androidRuntime,
      phase,
      snapshot,
      lifecycleFailure,
      backgroundFailure,
      canStartNode,
      startNode,
      canStopNode,
      stoppingNode,
      stopFailure,
      stopNode,
      showAccessorySetupPicker,
      refreshSnapshot,
      initiatePairing,
      approvePairing,
      rejectPairing,
      describeTarget,
      announceTarget,
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
      accessorySetup,
      accessorySetupFailure,
      androidRuntime,
      androidCapability,
      backgroundFailure,
      canStartNode,
      startNode,
      canStopNode,
      stoppingNode,
      stopFailure,
      stopNode,
      describeTarget,
      announceTarget,
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
      showAccessorySetupPicker,
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

export function routeConsumesDevelopmentSnapshot(pathname: string): boolean {
  return (
    pathname === "/nodes" ||
    pathname.startsWith("/nodes/") ||
    pathname === "/inbox" ||
    pathname.startsWith("/inbox/")
  );
}

export function useDevelopmentRuntime(): DevelopmentRuntimeView {
  const value = useContext(DevelopmentRuntimeContext);
  if (value === null) {
    throw new Error("useDevelopmentRuntime must be used inside DevelopmentRuntimeProvider");
  }
  return value;
}

function commandFailure(
  failure: unknown,
): Extract<RuntimeCommandResult<never>, { type: "operationFailure" }> {
  return {
    type: "operationFailure",
    detail: formatFailure(failure),
    ...(failure instanceof Bindings.NativeStoragePreparationError
      ? { storagePreparation: failure.outcome }
      : {}),
  };
}

function formatFailure(failure: DevelopmentRuntimeFailure | unknown): string {
  if (failure instanceof Bindings.NativeStoragePreparationError) {
    return failure.outcome.tag === "DevelopmentResetRequired"
      ? "App data must be reset before it can be opened. Open Recovery in Settings."
      : "Storage is temporarily unavailable. Your data has not changed. Try again.";
  }
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

function availableProviderRequiresAccessorySetup(provider: RuntimeProvider): provider is Extract<
  RuntimeProvider,
  { readonly availability: { readonly type: "available" } }
> & {
  readonly accessorySetup: NonNullable<
    Extract<
      RuntimeProvider,
      { readonly availability: { readonly type: "available" } }
    >["accessorySetup"]
  >;
} {
  return "accessorySetup" in provider && provider.accessorySetup !== undefined;
}
