import * as Bindings from "@prns-internal/expo";
import type {
  AccessorySetupRuntime,
  AccessorySetupStatus,
  AndroidRuntimeStatus,
  DevelopmentNodeSnapshot,
  DevelopmentRuntime,
  DevelopmentRuntimeScopeOptions,
  EffectDevelopmentRuntime,
} from "@prns-internal/expo";
import {
  DevelopmentRuntimeStartError as StartError,
  scopedDevelopmentRuntime,
} from "@prns-internal/expo";
import { act, fireEvent, render, waitFor } from "@testing-library/react-native";
import { Effect } from "effect";
import { destinationHash, identityHash, interfaceId } from "personal-rns/contract";
import { useLocalSearchParams } from "expo-router";
import { type ReactNode, useEffect } from "react";
import { AppState, type AppStateStatus } from "react-native";
import {
  DevelopmentRuntimeProvider,
  type DevelopmentRuntimeView,
  routeConsumesDevelopmentSnapshot,
  useDevelopmentRuntime,
} from "@/native/development-runtime-context";
import type { RuntimeProvider } from "@/native/runtime-provider.types";
import * as developmentRuntimeContext from "@/native/development-runtime-context";
import { ManagedNodeScreen } from "./managed-node-screen";
import { LocalNodeScreen, NodesScreen } from "./nodes-screen";
import { PairNodeScreen } from "./pair-node-screen";
jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
  useLocalSearchParams: jest.fn(() => ({ nodeId: "44444444444444444444444444444444" })),
  useRouter: () => ({ replace: jest.fn() }),
}));
const observedDestination = destinationHash(new Uint8Array(16).fill(0x33));
const observedIdentity = identityHash(new Uint8Array(16).fill(0x44));
type PairingState = DevelopmentNodeSnapshot["pairing"];
type PairingCandidates = DevelopmentNodeSnapshot["pairingCandidates"];
const readyAccessorySetup: AccessorySetupRuntime = {
  readStatus: async () => ({
    phase: "ready",
    picker: "idle",
    authorizedAccessoryCount: 1,
    nativeStart: "running",
    restorationLaunchRequested: false,
    revision: 1,
    lastError: null,
  }),
  showPicker: async () => ({ type: "completed" }),
  addStatusListener: () => ({ remove: jest.fn() }),
};
function pairingCandidate(
  candidateId: string,
  displayName: string | null,
  expiresInMillis = 90000n,
): PairingCandidates[number] {
  return {
    candidateId,
    displayName: displayName ?? undefined,
    observedAtMillis: 1n,
    expiresAtMillis: 2n,
    expiresInMillis,
  };
}
function snapshot(
  revision: bigint,
  includeObservation = false,
  pairing: PairingState = Bindings.RemoteControlPairingState.Searching.new(),
  pairedTargets: DevelopmentNodeSnapshot["pairedTargets"] = [],
  pairingCandidates: PairingCandidates = [],
): DevelopmentNodeSnapshot {
  return {
    contractFingerprint: "test-contract",
    revision,
    runtime: Bindings.DevelopmentNodeRuntime.Running,
    primaryIdentity: Bindings.PrimaryIdentityState.Present.new({
      identityHash: identityHash(new Uint8Array(16).fill(0x11)),
    }),
    localHost: Bindings.LocalHostState.Running.new({
      host: {
        revision,
        backend: {
          backend: "Native",
          capabilities: ["Bluetooth"],
          interfaceKinds: ["AutomaticBluetoothLe"],
        },
        interfaces: [
          {
            interfaceId: interfaceId(new Uint8Array(8).fill(0x22)),
            name: "Bluetooth Auto",
            kind: "AutomaticBluetoothLe",
            health: "Connected",
            rxBytes: 3n,
            txBytes: 4n,
            routeCount: 0,
            linkCount: 0,
            transportedLinkCount: 0,
          },
        ],
        routes: [],
        activeLinkCount: 0,
        destinationIdentities: includeObservation
          ? [{ destination: observedDestination, identity: observedIdentity }]
          : [],
        runtime: {
          running: true,
          uptimeMillis: 5,
          interfaceCount: 1,
          onlineInterfaceCount: 1,
          routeCount: 0,
          linkCount: 0,
          transportedLinkCount: 0,
          rxBytes: 3n,
          txBytes: 4n,
          rxBps: 0,
          txBps: 0,
        },
        persistence: {
          persistent: true,
          restored: true,
          lastFlushCause: "Startup",
        },
      },
    }),
    lxmf: { state: Bindings.LxmfHealthState.Ready, inboundOverflowCount: 0n },
    controllerIdentityFingerprint: undefined,
    pairing,
    pairingCandidates,
    pairedTargets,
    lastAnnouncement: undefined,
    generationId: 0n,
    activeOperation: undefined,
    failure: undefined,
  };
}
function fakeProvider(
  stop: jest.Mock,
  overrides: Partial<DevelopmentRuntime> = {},
  includeObservation = false,
  pairing: PairingState = Bindings.RemoteControlPairingState.Searching.new(),
  pairedTargets: DevelopmentNodeSnapshot["pairedTargets"] = [],
  pairingCandidates: PairingCandidates = [],
): RuntimeProvider {
  const initial = snapshot(2n, includeObservation, pairing, pairedTargets, pairingCandidates);
  const runtime: DevelopmentRuntime = {
    inspectDevelopmentIdentity: async () => initial.primaryIdentity,
    previewIdentityImport: async () => Bindings.IdentityImportPreviewOutcome.InvalidLength.new(),
    createGeneratedIdentity: async () => Bindings.IdentityCreationOutcome.AlreadyExists.new(),
    createImportedIdentity: async () => Bindings.IdentityCreationOutcome.AlreadyExists.new(),
    startDevelopmentNode: async () =>
      Bindings.DevelopmentNodeStartOutcome.Started.new({
        snapshot: initial,
      }),
    readDevelopmentNodeSnapshot: async () =>
      snapshot(1n, false, pairing, pairedTargets, pairingCandidates),
    initiateRemoteControlPairing: async () =>
      Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    approveRemoteControlPairing: async () => Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    rejectRemoteControlPairing: async () => Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    describeRemoteControlTarget: async () => Bindings.RemoteControlDescribeOutcome.Busy.new(),
    announceRemoteControlTarget: async () => Bindings.RemoteControlAnnounceOutcome.Busy.new(),
    saveObservedDestination: async () => Bindings.ContactMutationOutcome.NotObserved.new(),
    createManualContact: async () => Bindings.ContactMutationOutcome.NotFound.new(),
    setContactAlias: async () => Bindings.ContactMutationOutcome.NotFound.new(),
    setContactPinned: async () => Bindings.ContactMutationOutcome.NotFound.new(),
    deleteContact: async () => Bindings.ContactMutationOutcome.NotFound.new(),
    getContact: async () => Bindings.ContactLookupOutcome.NotFound.new(),
    listContacts: async () =>
      Bindings.ContactListOutcome.Listed.new({
        contacts: [],
      }),
    listLxmfPeers: async () =>
      Bindings.LxmfPeerListOutcome.Listed.new({
        peers: [],
      }),
    listLxmfMessages: async () =>
      Bindings.LxmfMessageListOutcome.Listed.new({
        messages: [],
      }),
    retryLxmfMessage: async () => Bindings.RetryLxmfMessageOutcome.NotFound.new(),
    cancelLxmfMessage: async () => Bindings.CancelLxmfMessageOutcome.NotFound.new(),
    announceLxmf: async () => Bindings.AnnounceLxmfOutcome.Announced,
    measureLxmfText: async () =>
      Bindings.MeasureLxmfTextOutcome.Measured.new({
        wireBytes: 113,
        remainingBytes: 318,
      }),
    sendDirectText: async () =>
      Bindings.SendDirectTextOutcome.Accepted.new({
        localRecordId: 1n,
      }),
    stopDevelopmentNode: async () => {
      stop();
      return Bindings.DevelopmentNodeStopOutcome.Stopped.new();
    },
    resetDevelopmentData: async () => Bindings.DevelopmentNodeStopOutcome.AlreadyStopped.new(),
    ...overrides,
  };
  const effectRuntime: EffectDevelopmentRuntime = {
    startDevelopmentNode: (input) => Effect.promise(() => runtime.startDevelopmentNode(input)),
    readDevelopmentNodeSnapshot: Effect.promise(runtime.readDevelopmentNodeSnapshot),
    initiateRemoteControlPairing: (input) =>
      Effect.promise(() => runtime.initiateRemoteControlPairing(input)),
    approveRemoteControlPairing: (input) =>
      Effect.promise(() => runtime.approveRemoteControlPairing(input)),
    rejectRemoteControlPairing: (input) =>
      Effect.promise(() => runtime.rejectRemoteControlPairing(input)),
    describeRemoteControlTarget: (input) =>
      Effect.promise(() => runtime.describeRemoteControlTarget(input)),
    announceRemoteControlTarget: (input) =>
      Effect.promise(() => runtime.announceRemoteControlTarget(input)),
    stopDevelopmentNode: Effect.promise(runtime.stopDevelopmentNode),
    resetDevelopmentData: Effect.promise(runtime.resetDevelopmentData),
  };
  return {
    availability: { type: "available", platform: "ios" },
    accessorySetup: readyAccessorySetup,
    runtime,
    acquire: (options) =>
      Effect.acquireRelease(
        Effect.sync(() => {
          options.onSnapshot(initial);
          options.onSnapshot(
            snapshot(1n, includeObservation, pairing, pairedTargets, pairingCandidates),
          );
          return { runtime: effectRuntime, initialSnapshot: initial };
        }),
        () => Effect.promise(runtime.stopDevelopmentNode).pipe(Effect.asVoid),
      ),
  };
}
function RuntimeViewProbe({
  publish,
}: {
  readonly publish: (view: DevelopmentRuntimeView) => void;
}) {
  const view = useDevelopmentRuntime();
  useEffect(() => publish(view), [publish, view]);
  return null;
}
function androidRestartFixture() {
  const stop = jest.fn();
  const base = fakeProvider(stop);
  if (!("runtime" in base)) throw new Error("expected native fixture");
  let currentSnapshot = snapshot(2n);
  let status: AndroidRuntimeStatus = {
    revision: 1,
    bluetoothPermission: "granted",
    backgroundDiscovery: "granted",
    connectionNotification: "enabled",
    bluetoothRadio: "on",
    locationServices: "on",
    service: "running",
    lastError: null,
  };
  let receiveStatus: ((next: AndroidRuntimeStatus) => void) | undefined;
  const observers: DevelopmentRuntimeScopeOptions[] = [];
  const release = jest.fn<Effect.Effect<void>, []>(() => Effect.void);
  const start = jest.fn<
    ReturnType<DevelopmentRuntime["startDevelopmentNode"]>,
    Parameters<DevelopmentRuntime["startDevelopmentNode"]>
  >(async () =>
    Bindings.DevelopmentNodeStartOutcome.Started.new({
      snapshot: snapshot(4n),
    }),
  );
  start.mockResolvedValueOnce(
    Bindings.DevelopmentNodeStartOutcome.Started.new({
      snapshot: currentSnapshot,
    }),
  );
  const runtime = {
    ...base.runtime,
    stopDevelopmentNode: jest.fn(base.runtime.stopDevelopmentNode),
    startDevelopmentNode: start,
    readDevelopmentNodeSnapshot: jest.fn(async () => currentSnapshot),
  };
  const provider: RuntimeProvider = {
    availability: { type: "available", platform: "android" },
    runtime,
    androidRuntime: {
      readStatus: async () => status,
      requestBluetoothPermissions: async () => status,
      requestBackgroundBluetoothPermission: async () => status,
      requestConnectionNotificationPermission: async () => status,
      addStatusListener: (listener) => {
        receiveStatus = listener;
        return { remove: jest.fn() };
      },
    },
    acquire: (options) =>
      Effect.gen(function* () {
        observers.push(options);
        yield* Effect.addFinalizer(() => release());
        return yield* scopedDevelopmentRuntime(runtime, {
          ...options,
          nativeLifetime: "process",
          developmentTcpTarget: "127.0.0.1:4242",
        });
      }),
  };
  return {
    provider,
    runtime,
    start,
    stop,
    release,
    observers,
    emit: (next: DevelopmentNodeSnapshot) => {
      currentSnapshot = next;
      observers.at(-1)?.onSnapshot(next);
    },
    status: (service: AndroidRuntimeStatus["service"]) => {
      status = { ...status, revision: status.revision + 1, service };
      receiveStatus?.(status);
    },
  };
}
function stoppedSnapshot(revision = 3n): DevelopmentNodeSnapshot {
  return {
    ...snapshot(revision),
    runtime: Bindings.DevelopmentNodeRuntime.Stopped,
    localHost: Bindings.LocalHostState.Stopped.new({
      lastStartFailure: undefined,
    }),
    pairing: Bindings.RemoteControlPairingState.BluetoothUnavailable.new(),
  };
}
describe("Foundation 1 Nodes runtime binding", () => {
  beforeEach(() => {
    jest
      .mocked(useLocalSearchParams)
      .mockReturnValue({ nodeId: "44444444444444444444444444444444" });
  });
  it.each([NodesScreen, LocalNodeScreen])(
    "stops Android through its service-backed SDK method from %p without restarting",
    async (Screen) => {
      const fixture = androidRestartFixture();
      let finishStop:
        | ((outcome: Awaited<ReturnType<DevelopmentRuntime["stopDevelopmentNode"]>>) => void)
        | undefined;
      fixture.runtime.stopDevelopmentNode.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishStop = resolve;
          }),
      );
      const publish = jest.fn<void, [DevelopmentRuntimeView]>();
      const view = render(
        <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
          <Screen />
          <RuntimeViewProbe publish={publish} />
        </DevelopmentRuntimeProvider>,
      );
      await waitFor(() => expect(publish.mock.calls.at(-1)?.[0].phase).toBe("ready"));
      const current = publish.mock.calls.at(-1)?.[0];
      await act(async () => {
        fireEvent.press(view.getByRole("button", { name: "Stop node" }));
        void current?.stopNode();
        void current?.stopNode();
      });
      expect(fixture.runtime.stopDevelopmentNode).toHaveBeenCalledTimes(1);
      expect(view.getByRole("button", { name: "Stopping…" })).toBeDisabled();
      await act(async () => {
        fixture.emit(stoppedSnapshot());
        fixture.status("stopped");
      });
      expect(view.queryByRole("button", { name: "Start node" })).toBeNull();
      current?.startNode();
      expect(fixture.start).toHaveBeenCalledTimes(1);
      await act(async () => finishStop?.(Bindings.DevelopmentNodeStopOutcome.Stopped.new()));
      await waitFor(() => expect(view.getByRole("button", { name: "Start node" })).toBeTruthy());
      expect(view.getByRole("button", { name: "Stop node" })).toBeDisabled();
      expect(fixture.start).toHaveBeenCalledTimes(1);
      expect(fixture.release).not.toHaveBeenCalled();
      fireEvent.press(view.getByRole("button", { name: "Start node" }));
      await waitFor(() => expect(fixture.start).toHaveBeenCalledTimes(2));
      expect(fixture.runtime.stopDevelopmentNode).toHaveBeenCalledTimes(1);
      view.unmount();
    },
  );
  it("keeps an Android Stop failure retryable without exposing native diagnostics", async () => {
    const fixture = androidRestartFixture();
    fixture.runtime.stopDevelopmentNode.mockResolvedValueOnce(
      Bindings.DevelopmentNodeStopOutcome.Failed.new({
        stage: Bindings.DevelopmentNodeStopStage.Node,
        detail: "internal drain did not finish",
      }),
    );
    const publish = jest.fn<void, [DevelopmentRuntimeView]>();
    const view = render(
      <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
        <NodesScreen />
        <RuntimeViewProbe publish={publish} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByRole("button", { name: "Stop node" })).toBeEnabled());
    fireEvent.press(view.getByRole("button", { name: "Stop node" }));
    await waitFor(() => expect(view.getByText("The node could not stop. Try again.")).toBeTruthy());
    await waitFor(() => expect(view.getByRole("button", { name: "Stop node" })).toBeEnabled());
    expect(view.queryByText("internal drain did not finish")).toBeNull();
    expect(publish.mock.calls.at(-1)?.[0].stopFailure).toBe("internal drain did not finish");
    fireEvent.press(view.getByRole("button", { name: "Stop node" }));
    await waitFor(() => expect(fixture.runtime.stopDevelopmentNode).toHaveBeenCalledTimes(2));
    expect(fixture.start).toHaveBeenCalledTimes(1);
    view.unmount();
  });
  it("fences a manual snapshot that completes after Stop without replacing its observer", async () => {
    const fixture = androidRestartFixture();
    let finishRead: ((next: DevelopmentNodeSnapshot) => void) | undefined;
    fixture.runtime.readDevelopmentNodeSnapshot.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishRead = resolve;
        }),
    );
    const publish = jest.fn<void, [DevelopmentRuntimeView]>();
    const view = render(
      <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
        <NodesScreen />
        <RuntimeViewProbe publish={publish} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(publish.mock.calls.at(-1)?.[0].phase).toBe("ready"));
    const read = publish.mock.calls.at(-1)?.[0].refreshSnapshot();
    await waitFor(() =>
      expect(fixture.runtime.readDevelopmentNodeSnapshot).toHaveBeenCalledTimes(1),
    );
    await act(async () => {
      fixture.emit(stoppedSnapshot());
      await publish.mock.calls.at(-1)?.[0].stopNode();
    });
    expect(publish.mock.calls.at(-1)?.[0].stoppingNode).toBe(false);
    await act(async () => {
      finishRead?.(snapshot(99n));
      expect(await read).toEqual({
        type: "operationFailure",
        detail: "This device's node changed while the request was running. Try again.",
      });
    });
    expect(publish.mock.calls.at(-1)?.[0].snapshot?.runtime).toBe(
      Bindings.DevelopmentNodeRuntime.Stopped,
    );
    expect(publish.mock.calls.at(-1)?.[0].snapshot?.revision).toBe(3n);
    expect(fixture.start).toHaveBeenCalledTimes(1);
    expect(fixture.release).not.toHaveBeenCalled();
    view.unmount();
  });
  it("does not publish or restart when a pending Android Stop completes after unmount", async () => {
    const fixture = androidRestartFixture();
    let finishStop:
      | ((outcome: Awaited<ReturnType<DevelopmentRuntime["stopDevelopmentNode"]>>) => void)
      | undefined;
    fixture.runtime.stopDevelopmentNode.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishStop = resolve;
        }),
    );
    const publish = jest.fn<void, [DevelopmentRuntimeView]>();
    const view = render(
      <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
        <NodesScreen />
        <RuntimeViewProbe publish={publish} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByRole("button", { name: "Stop node" })).toBeEnabled());
    const current = publish.mock.calls.at(-1)?.[0];
    fireEvent.press(view.getByRole("button", { name: "Stop node" }));
    view.unmount();
    await waitFor(() => expect(fixture.release).toHaveBeenCalledTimes(1));
    const publishedBeforeCompletion = publish.mock.calls.length;
    await act(async () => {
      finishStop?.(Bindings.DevelopmentNodeStopOutcome.Stopped.new());
      await current?.stopNode();
    });
    expect(publish).toHaveBeenCalledTimes(publishedBeforeCompletion);
    expect(fixture.runtime.stopDevelopmentNode).toHaveBeenCalledTimes(1);
    expect(fixture.runtime.readDevelopmentNodeSnapshot).not.toHaveBeenCalled();
    expect(fixture.start).toHaveBeenCalledTimes(1);
  });
  it.each([NodesScreen, LocalNodeScreen])(
    "explicitly restarts a stopped Android node from %p",
    async (Screen) => {
      const fixture = androidRestartFixture();
      const publish = jest.fn<void, [DevelopmentRuntimeView]>();
      const view = render(
        <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
          <Screen />
          <RuntimeViewProbe publish={publish} />
        </DevelopmentRuntimeProvider>,
      );
      await waitFor(() => expect(fixture.start).toHaveBeenCalledTimes(1));
      await act(async () => {
        fixture.emit(stoppedSnapshot());
        fixture.status("stopped");
      });
      expect(view.getByText("This device's node is stopped")).toBeTruthy();
      expect(view.queryByText("No paired nodes")).toBeNull();
      const stoppedView = publish.mock.calls.at(-1)?.[0];
      await act(async () => {
        fireEvent.press(view.getByRole("button", { name: "Start node" }));
        stoppedView?.startNode();
        stoppedView?.startNode();
      });
      await waitFor(() =>
        expect(publish.mock.calls.at(-1)?.[0].snapshot?.runtime).toBe(
          Bindings.DevelopmentNodeRuntime.Running,
        ),
      );
      expect(fixture.start).toHaveBeenCalledTimes(2);
      expect(fixture.start).toHaveBeenNthCalledWith(2, { developmentTcpTarget: "127.0.0.1:4242" });
      expect(fixture.release).toHaveBeenCalledTimes(1);
      expect(fixture.stop).not.toHaveBeenCalled();
      expect(view.queryByRole("button", { name: "Start node" })).toBeNull();
      view.unmount();
      await waitFor(() => expect(fixture.release).toHaveBeenCalledTimes(2));
      expect(fixture.stop).not.toHaveBeenCalled();
      stoppedView?.startNode();
      expect(fixture.start).toHaveBeenCalledTimes(2);
    },
  );
  it("does not restart a stopped node on resume, route refresh, or Android service events", async () => {
    const fixture = androidRestartFixture();
    let resume: ((next: AppStateStatus) => void) | undefined;
    const subscription = jest.spyOn(AppState, "addEventListener");
    const originalSubscribe = subscription.getMockImplementation();
    subscription.mockImplementation((_event, callback) => {
      resume = callback;
      return { remove: jest.fn() };
    });
    const view = render(
      <DevelopmentRuntimeProvider provider={fixture.provider} refreshActive={false}>
        <NodesScreen />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("No paired nodes")).toBeTruthy());
    await act(async () => {
      fixture.emit(stoppedSnapshot());
      fixture.status("stopped");
      resume?.("background");
      resume?.("active");
    });
    view.rerender(
      <DevelopmentRuntimeProvider provider={fixture.provider} refreshActive>
        <NodesScreen />
      </DevelopmentRuntimeProvider>,
    );
    await act(async () => fixture.status("stopping"));
    expect(view.queryByRole("button", { name: "Start node" })).toBeNull();
    await act(async () => fixture.status("stopped"));
    expect(view.getByRole("button", { name: "Start node" })).toBeTruthy();
    expect(view.getByText("Paired nodes unavailable")).toBeTruthy();
    expect(view.queryByText("No paired nodes")).toBeNull();
    expect(fixture.start).toHaveBeenCalledTimes(1);
    view.unmount();
    if (originalSubscribe !== undefined) subscription.mockImplementation(originalSubscribe);
  });
  it("waits for every previous scope across intervening rerenders and ignores late observations", async () => {
    const fixture = androidRestartFixture();
    let releaseScope: (() => void) | undefined;
    const publish = jest.fn<void, [DevelopmentRuntimeView]>();
    const view = render(
      <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
        <NodesScreen />
        <RuntimeViewProbe publish={publish} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("No paired nodes")).toBeTruthy());
    await act(async () => fixture.emit(stoppedSnapshot(90n)));
    fixture.release.mockImplementationOnce(() =>
      Effect.promise(
        () =>
          new Promise<void>((resolve) => {
            releaseScope = resolve;
          }),
      ),
    );
    fireEvent.press(view.getByRole("button", { name: "Start node" }));
    await waitFor(() => expect(fixture.release).toHaveBeenCalledTimes(1));
    expect(fixture.start).toHaveBeenCalledTimes(1);
    expect(view.getByText("Getting ready")).toBeTruthy();
    for (const interval of [1000, 2000]) {
      view.rerender(
        <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={interval}>
          <NodesScreen />
          <RuntimeViewProbe publish={publish} />
        </DevelopmentRuntimeProvider>,
      );
    }
    await act(async () => {
      await Promise.resolve();
    });
    expect(fixture.start).toHaveBeenCalledTimes(1);
    await act(async () => {
      fixture.observers[0]?.onSnapshot(stoppedSnapshot(99n));
      fixture.observers[0]?.onBackgroundFailure(
        new StartError({
          stage: Bindings.DevelopmentNodeFailureStage.Node,
          detail: "stale observer",
        }),
      );
      releaseScope?.();
    });
    await waitFor(() => expect(publish.mock.calls.at(-1)?.[0].snapshot?.revision).toBe(4n));
    expect(publish.mock.calls.at(-1)?.[0].backgroundFailure).toBeNull();
    expect(fixture.start).toHaveBeenCalledTimes(2);
    expect(fixture.observers.at(-1)?.refreshIntervalMillis).toBe(2000);
    view.unmount();
  });
  it("shows an explicit retry after start failure without retrying automatically", async () => {
    const fixture = androidRestartFixture();
    fixture.start.mockResolvedValueOnce(
      Bindings.DevelopmentNodeStartOutcome.Failed.new({
        stage: Bindings.DevelopmentNodeFailureStage.Bluetooth,
        detail: "test startup failure",
      }),
    );
    const publish = jest.fn<void, [DevelopmentRuntimeView]>();
    const view = render(
      <DevelopmentRuntimeProvider provider={fixture.provider}>
        <NodesScreen />
        <RuntimeViewProbe publish={publish} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("No paired nodes")).toBeTruthy());
    await act(async () => fixture.emit(stoppedSnapshot()));
    fireEvent.press(view.getByRole("button", { name: "Start node" }));
    await waitFor(() => expect(view.getByText("This device's node failed to start")).toBeTruthy());
    expect(publish.mock.calls.at(-1)?.[0].lifecycleFailure).toBe("test startup failure");
    expect(view.queryByText("test startup failure")).toBeNull();
    expect(view.getByRole("button", { name: "Start node" })).toBeTruthy();
    expect(fixture.start).toHaveBeenCalledTimes(2);
    fireEvent.press(view.getByRole("button", { name: "Start node" }));
    await waitFor(() => expect(view.getByText("No paired nodes")).toBeTruthy());
    expect(fixture.start).toHaveBeenCalledTimes(3);
    expect(publish.mock.calls.at(-1)?.[0].lifecycleFailure).toBeNull();
    expect(fixture.stop).not.toHaveBeenCalled();
    view.unmount();
  });
  it.each(["outcome", "failure"] as const)(
    "fences a manual snapshot's late %s after an explicit restart",
    async (completion) => {
      const fixture = androidRestartFixture();
      let finishRead: ((next: DevelopmentNodeSnapshot) => void) | undefined;
      let failRead: ((failure: Error) => void) | undefined;
      fixture.runtime.readDevelopmentNodeSnapshot.mockImplementationOnce(
        () =>
          new Promise((resolve, reject) => {
            finishRead = resolve;
            failRead = reject;
          }),
      );
      const publish = jest.fn<void, [DevelopmentRuntimeView]>();
      const view = render(
        <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
          <NodesScreen />
          <RuntimeViewProbe publish={publish} />
        </DevelopmentRuntimeProvider>,
      );
      await waitFor(() => expect(view.getByText("No paired nodes")).toBeTruthy());
      const read = publish.mock.calls.at(-1)?.[0].refreshSnapshot();
      await waitFor(() =>
        expect(fixture.runtime.readDevelopmentNodeSnapshot).toHaveBeenCalledTimes(1),
      );
      await act(async () => fixture.emit(stoppedSnapshot()));
      fireEvent.press(view.getByRole("button", { name: "Start node" }));
      await waitFor(() => expect(publish.mock.calls.at(-1)?.[0].snapshot?.revision).toBe(4n));
      await act(async () => {
        if (completion === "outcome") finishRead?.(stoppedSnapshot(99n));
        else failRead?.(new Error("old snapshot failed"));
        expect(await read).toEqual({
          type: "operationFailure",
          detail: "This device's node changed while the request was running. Try again.",
        });
      });
      expect(publish.mock.calls.at(-1)?.[0].snapshot?.revision).toBe(4n);
      expect(publish.mock.calls.at(-1)?.[0].snapshot?.runtime).toBe(
        Bindings.DevelopmentNodeRuntime.Running,
      );
      expect(publish.mock.calls.at(-1)?.[0].lifecycleFailure).toBeNull();
      expect(publish.mock.calls.at(-1)?.[0].backgroundFailure).toBeNull();
      expect(fixture.start).toHaveBeenCalledTimes(2);
      view.unmount();
    },
  );
  it("releases a pending restart on unmount without publishing its late completion or stopping the process", async () => {
    const fixture = androidRestartFixture();
    let finishStart:
      | ((outcome: Awaited<ReturnType<DevelopmentRuntime["startDevelopmentNode"]>>) => void)
      | undefined;
    fixture.start.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishStart = resolve;
        }),
    );
    const publish = jest.fn<void, [DevelopmentRuntimeView]>();
    const view = render(
      <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
        <NodesScreen />
        <RuntimeViewProbe publish={publish} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("No paired nodes")).toBeTruthy());
    await act(async () => fixture.emit(stoppedSnapshot()));
    fireEvent.press(view.getByRole("button", { name: "Start node" }));
    await waitFor(() => expect(fixture.start).toHaveBeenCalledTimes(2));
    view.unmount();
    await waitFor(() => expect(fixture.release).toHaveBeenCalledTimes(2));
    const publishedBeforeCompletion = publish.mock.calls.length;
    await act(async () =>
      finishStart?.(
        Bindings.DevelopmentNodeStartOutcome.Started.new({
          snapshot: snapshot(9n),
        }),
      ),
    );
    expect(publish).toHaveBeenCalledTimes(publishedBeforeCompletion);
    expect(fixture.stop).not.toHaveBeenCalled();
    expect(fixture.runtime.readDevelopmentNodeSnapshot).not.toHaveBeenCalled();
  });
  it.each([
    [{ bluetoothPermission: "blocked" as const }, "Open app settings"],
    [{ bluetoothRadio: "unsupported" as const }, "Bluetooth is not available on this device."],
    [
      { bluetoothRadio: "off" as const },
      "Turn on Bluetooth in Android Settings, then return to prns.",
    ],
    [
      { locationServices: "off" as const },
      "Turn on Location in Android Settings to let this version of Android discover nearby Bluetooth nodes.",
    ],
  ])("keeps Android access limitations truthful: %j", async (overrides, expected) => {
    const original = fakeProvider(jest.fn());
    if (!("acquire" in original)) throw new Error("expected native fixture");
    const { accessorySetup: _accessorySetup, ...shared } = original;
    const status: AndroidRuntimeStatus = {
      revision: 1,
      bluetoothPermission: "granted",
      backgroundDiscovery: "notRequired",
      connectionNotification: "enabled",
      bluetoothRadio: "on",
      locationServices: "on",
      service: "running",
      lastError: null,
      ...overrides,
    };
    const request = jest.fn(async () => status);
    const provider: RuntimeProvider = {
      ...shared,
      availability: { type: "available", platform: "android" },
      androidRuntime: {
        readStatus: async () => status,
        requestBluetoothPermissions: request,
        requestBackgroundBluetoothPermission: request,
        requestConnectionNotificationPermission: request,
        addStatusListener: () => ({ remove: jest.fn() }),
      },
    };
    const view = render(
      <DevelopmentRuntimeProvider provider={provider}>
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText(expected)).toBeTruthy());
    expect(
      view.queryByText("Bluetooth is available. Nearby nodes will appear when discovered."),
    ).toBeNull();
    expect(request).not.toHaveBeenCalled();
  });
  it("runs Android without Bluetooth permission and exposes its own explicit access flow", async () => {
    const original = fakeProvider(jest.fn());
    if (!("acquire" in original)) throw new Error("expected native fixture");
    const { accessorySetup: _accessorySetup, ...shared } = original;
    const status: AndroidRuntimeStatus = {
      revision: 1,
      bluetoothPermission: "notRequested",
      backgroundDiscovery: "notGranted",
      connectionNotification: "enabled",
      bluetoothRadio: "on",
      locationServices: "on",
      service: "running",
      lastError: null,
    };
    const request = jest.fn(async () => ({
      ...status,
      revision: 2,
      bluetoothPermission: "granted" as const,
    }));
    const requestBackground = jest.fn(async () => ({
      ...status,
      revision: 3,
      bluetoothPermission: "granted" as const,
      backgroundDiscovery: "granted" as const,
    }));
    const acquire = jest.fn(shared.acquire);
    const provider: RuntimeProvider = {
      ...shared,
      availability: { type: "available", platform: "android" },
      acquire,
      androidRuntime: {
        readStatus: async () => status,
        requestBluetoothPermissions: request,
        requestBackgroundBluetoothPermission: requestBackground,
        requestConnectionNotificationPermission: async () => status,
        addStatusListener: () => ({ remove: jest.fn() }),
      },
    };
    const publish = jest.fn<void, [DevelopmentRuntimeView]>();
    const view = render(
      <DevelopmentRuntimeProvider provider={provider}>
        <RuntimeViewProbe publish={publish} />
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() =>
      expect(publish).toHaveBeenLastCalledWith(expect.objectContaining({ phase: "ready" })),
    );
    expect(acquire).toHaveBeenCalledTimes(1);
    expect(request).not.toHaveBeenCalled();
    expect(requestBackground).not.toHaveBeenCalled();
    expect(view.queryByRole("button", { name: "Choose a Bluetooth node" })).toBeNull();
    expect(
      view.getByText(
        "Your identity, contacts, and other connections do not need Bluetooth access.",
      ),
    ).toBeTruthy();
    fireEvent.press(view.getByRole("button", { name: "Allow Bluetooth" }));
    await waitFor(() =>
      expect(
        view.getByText("Bluetooth is available. Nearby nodes will appear when discovered."),
      ).toBeTruthy(),
    );
    expect(request).toHaveBeenCalledTimes(1);
    expect(requestBackground).not.toHaveBeenCalled();
    fireEvent.press(view.getByRole("button", { name: "Allow background discovery" }));
    await waitFor(() => expect(requestBackground).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(view.queryByRole("button", { name: "Allow background discovery" })).toBeNull(),
    );
    expect(acquire).toHaveBeenCalledTimes(1);
  });
  it("polls snapshots only for visible Nodes and Inbox routes", () => {
    expect(routeConsumesDevelopmentSnapshot("/nodes")).toBe(true);
    expect(routeConsumesDevelopmentSnapshot("/nodes/pair")).toBe(true);
    expect(routeConsumesDevelopmentSnapshot("/inbox")).toBe(true);
    expect(routeConsumesDevelopmentSnapshot("/inbox/0011")).toBe(true);
    expect(routeConsumesDevelopmentSnapshot("/")).toBe(false);
    expect(routeConsumesDevelopmentSnapshot("/settings")).toBe(false);
    expect(routeConsumesDevelopmentSnapshot("/network/interfaces")).toBe(false);
  });
  it("keeps runtime acquisition gated while showing user-initiated system setup", async () => {
    const stop = jest.fn();
    const base = fakeProvider(stop);
    if (!("acquire" in base)) {
      throw new Error("the iOS test provider must expose runtime acquisition");
    }
    const acquire = jest.fn(base.acquire);
    let publishSetup: ((status: AccessorySetupStatus) => void) | undefined;
    const showPicker = jest.fn(async () => {
      publishSetup?.({
        phase: "setupRequired",
        picker: "idle",
        authorizedAccessoryCount: 0,
        nativeStart: "notRequested",
        restorationLaunchRequested: false,
        revision: 2,
        lastError: {
          code: "userCancelled",
          detail: "The Bluetooth chooser was cancelled.",
        },
      });
      return { type: "cancelled" as const };
    });
    const provider: RuntimeProvider = {
      ...base,
      accessorySetup: {
        readStatus: async () => ({
          phase: "setupRequired",
          picker: "idle",
          authorizedAccessoryCount: 0,
          nativeStart: "notRequested",
          restorationLaunchRequested: false,
          revision: 1,
          lastError: null,
        }),
        showPicker,
        addStatusListener: (listener) => {
          publishSetup = listener;
          return { remove: jest.fn() };
        },
      },
      acquire,
    };
    const view = render(
      <DevelopmentRuntimeProvider provider={provider}>
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("Choose a nearby node")).toBeTruthy());
    expect(
      view.getByText(
        /First choose a nearby Bluetooth node\. Then open secure pairing on the node/u,
      ),
    ).toBeTruthy();
    expect(JSON.stringify(view.toJSON())).not.toMatch(/system (?:accessory|Bluetooth) setup/iu);
    expect(acquire).not.toHaveBeenCalled();
    fireEvent.press(view.getByRole("button", { name: "Choose a Bluetooth node" }));
    await waitFor(() => expect(showPicker).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(
        view.getByText("The Bluetooth chooser was cancelled. No Bluetooth access changed."),
      ).toBeTruthy(),
    );
    expect(view.queryByText("The Bluetooth chooser was cancelled.")).toBeNull();
  });
  it("waits for chooser dismissal and disables it while native startup is in flight", async () => {
    const stop = jest.fn();
    const base = fakeProvider(stop);
    if (!("acquire" in base)) {
      throw new Error("the iOS test provider must expose runtime acquisition");
    }
    const acquire = jest.fn(base.acquire);
    const showPicker = jest.fn(async () => ({ type: "completed" as const }));
    let publishSetup: ((status: AccessorySetupStatus) => void) | undefined;
    const provider: RuntimeProvider = {
      ...base,
      accessorySetup: {
        readStatus: async () => ({
          phase: "ready",
          picker: "presented",
          authorizedAccessoryCount: 1,
          nativeStart: "notRequested",
          restorationLaunchRequested: false,
          revision: 1,
          lastError: null,
        }),
        showPicker,
        addStatusListener: (listener) => {
          publishSetup = listener;
          return { remove: jest.fn() };
        },
      },
      acquire,
    };
    const view = render(
      <DevelopmentRuntimeProvider provider={provider}>
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("Bluetooth access ready")).toBeTruthy());
    expect(acquire).not.toHaveBeenCalled();
    act(() =>
      publishSetup?.({
        phase: "ready",
        picker: "idle",
        authorizedAccessoryCount: 1,
        nativeStart: "stopping",
        restorationLaunchRequested: false,
        revision: 2,
        lastError: null,
      }),
    );
    expect(acquire).not.toHaveBeenCalled();
    act(() =>
      publishSetup?.({
        phase: "ready",
        picker: "idle",
        authorizedAccessoryCount: 1,
        nativeStart: "notRequested",
        restorationLaunchRequested: false,
        revision: 3,
        lastError: null,
      }),
    );
    await waitFor(() => expect(acquire).toHaveBeenCalledTimes(1));
    act(() =>
      publishSetup?.({
        phase: "ready",
        picker: "idle",
        authorizedAccessoryCount: 1,
        nativeStart: "starting",
        restorationLaunchRequested: false,
        revision: 4,
        lastError: null,
      }),
    );
    const busyChooser = view.getByRole("button", { name: "Add another Bluetooth node" });
    expect(busyChooser.props.accessibilityState).toEqual({ disabled: true });
    fireEvent.press(busyChooser);
    expect(showPicker).not.toHaveBeenCalled();
    act(() =>
      publishSetup?.({
        phase: "ready",
        picker: "idle",
        authorizedAccessoryCount: 1,
        nativeStart: "running",
        restorationLaunchRequested: false,
        revision: 5,
        lastError: null,
      }),
    );
    await waitFor(() =>
      expect(
        view.getByRole("button", { name: "Add another Bluetooth node" }).props.accessibilityState,
      ).toEqual({ disabled: false }),
    );
  });
  it("shows only Bluetooth-access recovery when ASK status cannot be read", async () => {
    const stop = jest.fn();
    const base = fakeProvider(stop);
    if (!("acquire" in base)) {
      throw new Error("the iOS test provider must expose runtime acquisition");
    }
    const acquire = jest.fn(base.acquire);
    const provider: RuntimeProvider = {
      ...base,
      accessorySetup: {
        readStatus: async () => {
          throw new Error("hostile internal AccessorySetupKit session failure");
        },
        showPicker: async () => ({ type: "completed" }),
        addStatusListener: () => ({ remove: jest.fn() }),
      },
      acquire,
    };
    const view = render(
      <DevelopmentRuntimeProvider provider={provider}>
        <NodesScreen />
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getAllByText("Bluetooth access unavailable")).toHaveLength(2));
    expect(view.queryByText("This device's node failed to start")).toBeNull();
    expect(JSON.stringify(view.toJSON())).not.toMatch(/AccessorySetupKit|session failure/iu);
    expect(acquire).not.toHaveBeenCalled();
  });
  it("starts once when ASK reports authorization and degrades honestly after removal", async () => {
    const stop = jest.fn();
    const base = fakeProvider(stop);
    if (!("acquire" in base)) {
      throw new Error("the iOS test provider must expose runtime acquisition");
    }
    const acquire = jest.fn(base.acquire);
    let publishSetup:
      | ((status: Awaited<ReturnType<AccessorySetupRuntime["readStatus"]>>) => void)
      | undefined;
    const provider: RuntimeProvider = {
      ...base,
      accessorySetup: {
        readStatus: async () => ({
          phase: "setupRequired",
          picker: "idle",
          authorizedAccessoryCount: 0,
          nativeStart: "notRequested",
          restorationLaunchRequested: false,
          revision: 1,
          lastError: null,
        }),
        showPicker: async () => ({ type: "completed" }),
        addStatusListener: (listener) => {
          publishSetup = listener;
          return { remove: jest.fn() };
        },
      },
      acquire,
    };
    const view = render(
      <DevelopmentRuntimeProvider provider={provider}>
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("Choose a nearby node")).toBeTruthy());
    act(() =>
      publishSetup?.({
        phase: "ready",
        picker: "idle",
        authorizedAccessoryCount: 1,
        nativeStart: "notRequested",
        restorationLaunchRequested: false,
        revision: 2,
        lastError: null,
      }),
    );
    await waitFor(() => expect(view.getByText("Bluetooth access ready")).toBeTruthy());
    await waitFor(() => expect(acquire).toHaveBeenCalledTimes(1));
    act(() =>
      publishSetup?.({
        phase: "setupRequired",
        picker: "idle",
        authorizedAccessoryCount: 0,
        nativeStart: "running",
        restorationLaunchRequested: false,
        revision: 3,
        lastError: null,
      }),
    );
    await waitFor(() =>
      expect(
        view.getByText(
          "Bluetooth access was removed while this device stayed running. Authorize a node again to reconnect.",
        ),
      ).toBeTruthy(),
    );
    expect(acquire).toHaveBeenCalledTimes(1);
  });
  it("does not let a stale initial ASK read overwrite a newer authorization event", async () => {
    const stop = jest.fn();
    const base = fakeProvider(stop);
    if (!("acquire" in base)) {
      throw new Error("the iOS test provider must expose runtime acquisition");
    }
    const acquire = jest.fn(base.acquire);
    let publishSetup: ((status: AccessorySetupStatus) => void) | undefined;
    let resolveInitialRead: ((status: AccessorySetupStatus) => void) | undefined;
    const provider: RuntimeProvider = {
      ...base,
      accessorySetup: {
        readStatus: () =>
          new Promise((resolve) => {
            resolveInitialRead = resolve;
          }),
        showPicker: async () => ({ type: "completed" }),
        addStatusListener: (listener) => {
          publishSetup = listener;
          return { remove: jest.fn() };
        },
      },
      acquire,
    };
    const view = render(
      <DevelopmentRuntimeProvider provider={provider}>
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    act(() => {
      publishSetup?.({
        phase: "ready",
        picker: "idle",
        authorizedAccessoryCount: 1,
        nativeStart: "notRequested",
        restorationLaunchRequested: false,
        revision: 2,
        lastError: null,
      });
    });
    await waitFor(() => expect(acquire).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(view.getByText("Bluetooth access ready")).toBeTruthy());
    await act(async () => {
      resolveInitialRead?.({
        phase: "setupRequired",
        picker: "idle",
        authorizedAccessoryCount: 0,
        nativeStart: "notRequested",
        restorationLaunchRequested: false,
        revision: 1,
        lastError: null,
      });
      await Promise.resolve();
    });
    expect(view.getByText("Bluetooth access ready")).toBeTruthy();
    expect(view.queryByText("Choose a nearby node")).toBeNull();
    expect(acquire).toHaveBeenCalledTimes(1);
  });
  it("retains startup diagnostics without exposing them on the Nodes screen", async () => {
    const stop = jest.fn();
    const base = fakeProvider(stop);
    const failure = new StartError({
      stage: Bindings.DevelopmentNodeFailureStage.Bluetooth,
      detail: "Bluetooth is unavailable in this simulator.",
    });
    const provider: RuntimeProvider = {
      ...base,
      acquire: () => Effect.fail(failure),
    };
    const publish = jest.fn<void, [DevelopmentRuntimeView]>();
    const view = render(
      <DevelopmentRuntimeProvider provider={provider}>
        <NodesScreen />
        <RuntimeViewProbe publish={publish} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("This device's node failed to start")).toBeTruthy());
    expect(view.queryByText("Bluetooth is unavailable in this simulator.")).toBeNull();
    await waitFor(() =>
      expect(publish).toHaveBeenCalledWith(
        expect.objectContaining({
          lifecycleFailure: "Bluetooth is unavailable in this simulator.",
        }),
      ),
    );
  });
  it("keeps durable mailbox commands available after generation acquisition fails", async () => {
    const stop = jest.fn();
    const base = fakeProvider(stop);
    if (!("runtime" in base)) {
      throw new Error("the iOS test provider must expose a native runtime");
    }
    const listLxmfMessages = jest.fn(async () =>
      Bindings.LxmfMessageListOutcome.Listed.new({
        messages: [],
      }),
    );
    const retryLxmfMessage = jest.fn(async () => Bindings.RetryLxmfMessageOutcome.NotFound.new());
    const cancelLxmfMessage = jest.fn(async () => Bindings.CancelLxmfMessageOutcome.NotFound.new());
    const sendDirectText = jest.fn(async () =>
      Bindings.SendDirectTextOutcome.Accepted.new({
        localRecordId: 1n,
      }),
    );
    const provider: RuntimeProvider = {
      ...base,
      runtime: {
        ...base.runtime,
        listLxmfMessages,
        retryLxmfMessage,
        cancelLxmfMessage,
        sendDirectText,
      },
      acquire: () => Effect.die("startup failed"),
    };
    const publish = jest.fn<void, [DevelopmentRuntimeView]>();
    render(
      <DevelopmentRuntimeProvider provider={provider}>
        <RuntimeViewProbe publish={publish} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() =>
      expect(publish).toHaveBeenCalledWith(expect.objectContaining({ phase: "failed" })),
    );
    const failedView = publish.mock.calls.find(([view]) => view.phase === "failed")?.[0];
    if (failedView === undefined) {
      throw new Error("the failed runtime view was not published");
    }
    expect(
      await failedView.listLxmfMessages({ peer: undefined, before: undefined, limit: 25 }),
    ).toEqual({
      type: "outcome",
      outcome: Bindings.LxmfMessageListOutcome.Listed.new({ messages: [] }),
    });
    expect(await failedView.retryLxmfMessage(7n)).toEqual({
      type: "outcome",
      outcome: Bindings.RetryLxmfMessageOutcome.NotFound.new(),
    });
    expect(await failedView.cancelLxmfMessage(8n)).toEqual({
      type: "outcome",
      outcome: Bindings.CancelLxmfMessageOutcome.NotFound.new(),
    });
    expect(
      await failedView.sendDirectText({
        destination: observedDestination,
        title: "not admitted",
        content: "node stopped",
      }),
    ).toMatchObject({ type: "operationFailure" });
    expect(listLxmfMessages).toHaveBeenCalledTimes(1);
    expect(retryLxmfMessage).toHaveBeenCalledWith(7n);
    expect(cancelLxmfMessage).toHaveBeenCalledWith(8n);
    expect(sendDirectText).not.toHaveBeenCalled();
    const storagePreparation =
      Bindings.NativeStoragePreparationOutcome.DevelopmentResetRequired.new({
        reason: "private database detail",
      });
    listLxmfMessages.mockRejectedValueOnce(
      new Bindings.NativeStoragePreparationError(storagePreparation),
    );
    expect(await failedView.listLxmfMessages({ limit: 25 })).toEqual({
      type: "operationFailure",
      detail: "App data must be reset before it can be opened. Open Recovery in Settings.",
      storagePreparation,
    });
  });
  it("holds one scoped runtime across the Nodes surface and stops it on layout release", async () => {
    const stop = jest.fn();
    const view = render(
      <DevelopmentRuntimeProvider provider={fakeProvider(stop)} refreshIntervalMillis={50}>
        <NodesScreen />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("No paired nodes")).toBeTruthy());
    expect(view.getByText("View this device")).toBeTruthy();
    expect(view.queryByText("Host runtime")).toBeNull();
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("labels saved nodes as paired without implying current connectivity", async () => {
    const stop = jest.fn();
    const describeRemoteControlTarget = jest.fn(async () =>
      Bindings.RemoteControlDescribeOutcome.Busy.new(),
    );
    const target = {
      targetIdentityFingerprint: observedIdentity,
      destination: observedDestination,
      controllerIdentityFingerprint: identityHash(new Uint8Array(16).fill(0x55)),
      permittedRequests: [Bindings.RemoteControlRequestKind.Describe],
    };
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { describeRemoteControlTarget },
          false,
          Bindings.RemoteControlPairingState.Searching.new(),
          [target],
        )}
      >
        <NodesScreen />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("Paired")).toBeTruthy());
    expect(view.getByText("Manage node")).toBeTruthy();
    expect(view.queryByText("Ready")).toBeNull();
    expect(describeRemoteControlTarget).not.toHaveBeenCalled();
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("renders the searching state in board-neutral user language", async () => {
    const stop = jest.fn();
    const view = render(
      <DevelopmentRuntimeProvider provider={fakeProvider(stop)} refreshIntervalMillis={50}>
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("Looking for nearby nodes")).toBeTruthy());
    expect(view.getByText("Searching")).toBeTruthy();
    expect(JSON.stringify(view.toJSON())).not.toMatch(
      /E290|signed availability|upstream RemoteControl/iu,
    );
    expect(view.queryByText("Ready")).toBeNull();
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("lists nearby nodes and submits the invitation for the explicit selection", async () => {
    const stop = jest.fn();
    const initiateRemoteControlPairing = jest.fn(async () =>
      Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    );
    const candidates = [
      pairingCandidate("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Kitchen node"),
      pairingCandidate("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Workshop node", 42000n),
    ];
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { initiateRemoteControlPairing },
          false,
          Bindings.RemoteControlPairingState.Searching.new(),
          [],
          candidates,
        )}
        refreshIntervalMillis={50}
      >
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("Nearby nodes")).toBeTruthy());
    expect(view.getByText("Kitchen node")).toBeTruthy();
    expect(view.getByText("Workshop node")).toBeTruthy();
    expect(view.getByText("42 seconds remaining")).toBeTruthy();
    expect(view.queryByLabelText("Invitation code")).toBeNull();
    fireEvent.press(view.getByLabelText("Select Workshop node BBBBBBBB"));
    await waitFor(() => expect(view.getByLabelText("Invitation code")).toBeTruthy());
    fireEvent.changeText(view.getByLabelText("Invitation code"), "d8509492");
    fireEvent.press(view.getByRole("button", { name: "Submit invitation" }));
    await waitFor(() =>
      expect(initiateRemoteControlPairing).toHaveBeenCalledWith({
        candidateId: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        invitationCode: "D8509492",
      }),
    );
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("automatically selects the only nearby node", async () => {
    const stop = jest.fn();
    const onlyCandidate = pairingCandidate("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Kitchen node");
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          {},
          false,
          Bindings.RemoteControlPairingState.Searching.new(),
          [],
          [onlyCandidate],
        )}
        refreshIntervalMillis={50}
      >
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByLabelText("Selected Kitchen node AAAAAAAA")).toBeTruthy());
    expect(view.getByLabelText("Invitation code")).toBeTruthy();
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("honors a live route selection without switching to another observed node", async () => {
    const stop = jest.fn();
    const initiateRemoteControlPairing = jest.fn(async () =>
      Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    );
    const candidates = [
      pairingCandidate("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Kitchen node"),
      pairingCandidate("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Workshop node"),
    ];
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { initiateRemoteControlPairing },
          false,
          Bindings.RemoteControlPairingState.Searching.new(),
          [],
          candidates,
        )}
        refreshIntervalMillis={50}
      >
        <PairNodeScreen selectedCandidateId="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() =>
      expect(view.getByLabelText("Selected Workshop node BBBBBBBB")).toBeTruthy(),
    );
    expect(
      view.getByRole("button", { name: "Selected Workshop node BBBBBBBB", selected: true }),
    ).toBeTruthy();
    expect(
      view.getByRole("button", { name: "Select Kitchen node AAAAAAAA", selected: false }),
    ).toBeTruthy();
    fireEvent.changeText(view.getByLabelText("Invitation code"), "1234abcd");
    fireEvent.press(view.getByRole("button", { name: "Submit invitation" }));
    await waitFor(() =>
      expect(initiateRemoteControlPairing).toHaveBeenCalledWith({
        candidateId: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        invitationCode: "1234ABCD",
      }),
    );
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("keeps the invitation when the selected node observation refreshes", async () => {
    const stop = jest.fn();
    const candidateId = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const initial = pairingCandidate(candidateId, "Kitchen node", 42000n);
    const refreshed = {
      ...initial,
      observedAtMillis: 2n,
      expiresAtMillis: 3n,
      expiresInMillis: 90000n,
    };
    const readDevelopmentNodeSnapshot = jest.fn(async () =>
      snapshot(3n, false, Bindings.RemoteControlPairingState.Searching.new(), [], [refreshed]),
    );
    let runtimeView: DevelopmentRuntimeView | undefined;
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { readDevelopmentNodeSnapshot },
          false,
          Bindings.RemoteControlPairingState.Searching.new(),
          [],
          [initial],
        )}
        refreshIntervalMillis={10}
      >
        <PairNodeScreen selectedCandidateId={candidateId} />
        <RuntimeViewProbe
          publish={(next) => {
            runtimeView = next;
          }}
        />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByLabelText("Invitation code")).toBeTruthy());
    fireEvent.changeText(view.getByLabelText("Invitation code"), "d8509492");
    await act(async () => {
      await runtimeView?.refreshSnapshot();
    });
    expect(readDevelopmentNodeSnapshot).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(view.getByText("2 minutes remaining")).toBeTruthy());
    expect(view.getByLabelText("Invitation code").props.value).toBe("D8509492");
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("keeps an expired selection visible while leaving other nodes selectable", async () => {
    const stop = jest.fn();
    const candidates = [pairingCandidate("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", null)];
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          {},
          false,
          Bindings.RemoteControlPairingState.Searching.new(),
          [],
          candidates,
        )}
        refreshIntervalMillis={50}
      >
        <PairNodeScreen selectedCandidateId="cccccccccccccccccccccccccccccccc" />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("Node no longer available")).toBeTruthy());
    expect(view.getByText("Nearby node")).toBeTruthy();
    expect(view.getByText("Node ID")).toBeTruthy();
    expect(view.getByText("2 minutes remaining")).toBeTruthy();
    expect(view.queryByLabelText("Invitation code")).toBeNull();
    fireEvent.press(view.getByLabelText("Select Nearby node BBBBBBBB"));
    await waitFor(() => expect(view.getByText("Ready to pair")).toBeTruthy());
    expect(view.getByLabelText("Invitation code")).toBeTruthy();
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("leaves another observed node selectable after a terminal pairing result", async () => {
    const terminalStates = [
      Bindings.RemoteControlPairingState.Rejected.new({
        detail: "internal rejection",
      }),
      Bindings.RemoteControlPairingState.Expired.new({
        detail: "internal expiry",
      }),
      Bindings.RemoteControlPairingState.Failed.new({
        stage: Bindings.RemoteControlPairingFailureStage.Request,
        detail: "internal request failure",
      }),
      Bindings.RemoteControlPairingState.Paired.new({
        attemptId: "attempt-a",
      }),
    ] as const satisfies readonly PairingState[];
    const candidateB = pairingCandidate("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Workshop node");
    for (const pairing of terminalStates) {
      const stop = jest.fn();
      const view = render(
        <DevelopmentRuntimeProvider
          provider={fakeProvider(stop, {}, false, pairing, [], [candidateB])}
          refreshIntervalMillis={50}
        >
          <PairNodeScreen selectedCandidateId="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" />
        </DevelopmentRuntimeProvider>,
      );
      await waitFor(() => expect(view.getByText("Workshop node")).toBeTruthy());
      await waitFor(() =>
        expect(view.getByLabelText("Selected Workshop node BBBBBBBB")).toBeTruthy(),
      );
      expect(view.queryByText("Node no longer available")).toBeNull();
      expect(view.getByLabelText("Invitation code")).toBeTruthy();
      view.unmount();
      await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
    }
  });
  it("shows one failed submission after selecting another node from a terminal result", async () => {
    const stop = jest.fn();
    const initiateRemoteControlPairing = jest.fn(async () =>
      Bindings.RemoteControlPairingCommandOutcome.Busy.new(),
    );
    const candidateB = pairingCandidate("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Workshop node");
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { initiateRemoteControlPairing },
          false,
          Bindings.RemoteControlPairingState.Paired.new({
            attemptId: "attempt-a",
          }),
          [],
          [candidateB],
        )}
        refreshIntervalMillis={50}
      >
        <PairNodeScreen selectedCandidateId="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByText("Workshop node")).toBeTruthy());
    await waitFor(() =>
      expect(view.getByLabelText("Selected Workshop node BBBBBBBB")).toBeTruthy(),
    );
    fireEvent.changeText(view.getByLabelText("Invitation code"), "1234abcd");
    fireEvent.press(view.getByRole("button", { name: "Submit invitation" }));
    await waitFor(() =>
      expect(
        view.getAllByText("Another node operation is in progress. Try again shortly."),
      ).toHaveLength(1),
    );
    expect(initiateRemoteControlPairing).toHaveBeenCalledWith({
      candidateId: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      invitationCode: "1234ABCD",
    });
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("keeps the selected node fixed while its invitation is being submitted", async () => {
    const stop = jest.fn();
    let finishInitiation!: (outcome: Bindings.RemoteControlPairingCommandOutcome) => void;
    const initiateRemoteControlPairing = jest.fn(
      () =>
        new Promise<Bindings.RemoteControlPairingCommandOutcome>((resolve) => {
          finishInitiation = resolve;
        }),
    );
    const candidates = [
      pairingCandidate("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Kitchen node"),
      pairingCandidate("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Workshop node"),
    ];
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { initiateRemoteControlPairing },
          false,
          Bindings.RemoteControlPairingState.Searching.new(),
          [],
          candidates,
        )}
        refreshIntervalMillis={50}
      >
        <PairNodeScreen selectedCandidateId="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByLabelText("Invitation code")).toBeTruthy());
    fireEvent.changeText(view.getByLabelText("Invitation code"), "1234abcd");
    fireEvent.press(view.getByRole("button", { name: "Submit invitation" }));
    await waitFor(() => expect(initiateRemoteControlPairing).toHaveBeenCalledTimes(1));
    expect(view.getByLabelText("Invitation code").props.editable).toBe(false);
    fireEvent.press(view.getByLabelText("Select Workshop node BBBBBBBB"));
    expect(view.getByLabelText("Selected Kitchen node AAAAAAAA")).toBeTruthy();
    expect(view.queryByLabelText("Selected Workshop node BBBBBBBB")).toBeNull();
    expect(initiateRemoteControlPairing).toHaveBeenCalledWith({
      candidateId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      invitationCode: "1234ABCD",
    });
    finishInitiation(Bindings.RemoteControlPairingCommandOutcome.Busy.new());
    await waitFor(() =>
      expect(
        view.getByText("Another node operation is in progress. Try again shortly."),
      ).toBeTruthy(),
    );
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("keeps every pairing step board-neutral and free of protocol narration", async () => {
    const pairingViews = [
      [Bindings.RemoteControlPairingState.BluetoothUnavailable.new(), "Bluetooth unavailable"],
      [Bindings.RemoteControlPairingState.Searching.new(), "Looking for nearby nodes"],
      [
        Bindings.RemoteControlPairingState.InvitationSubmitted.new({
          candidateId: "candidate-1",
        }),
        "Invitation sent",
      ],
      [
        Bindings.RemoteControlPairingState.ConfirmationRequired.new({
          attemptId: "attempt-1",
          confirmationCode: "123456",
          targetIdentityFingerprint: observedIdentity,
          permissions: [Bindings.RemoteControlRequestKind.Describe],
        }),
        "Confirmation required",
      ],
      [
        Bindings.RemoteControlPairingState.AwaitingTargetApproval.new({
          attemptId: "attempt-1",
        }),
        "Waiting for the node",
      ],
      [
        Bindings.RemoteControlPairingState.Persisting.new({
          attemptId: "attempt-1",
        }),
        "Finishing pairing",
      ],
      [
        Bindings.RemoteControlPairingState.Paired.new({
          attemptId: "attempt-1",
        }),
        "Paired",
      ],
      [
        Bindings.RemoteControlPairingState.Rejected.new({
          detail: "upstream RemoteControl rejected",
        }),
        "Rejected",
      ],
      [
        Bindings.RemoteControlPairingState.Expired.new({
          detail: "signed availability expired",
        }),
        "Expired",
      ],
      [Bindings.RemoteControlPairingState.Cancelled.new(), "Cancelled"],
      [
        Bindings.RemoteControlPairingState.Failed.new({
          stage: Bindings.RemoteControlPairingFailureStage.Link,
          detail: "E290 link failure",
        }),
        "Pairing failed",
      ],
    ] as const satisfies readonly (readonly [PairingState, string])[];
    for (const [pairing, expectedCopy] of pairingViews) {
      const stop = jest.fn();
      const view = render(
        <DevelopmentRuntimeProvider
          provider={fakeProvider(stop, {}, false, pairing)}
          refreshIntervalMillis={50}
        >
          <PairNodeScreen selectedCandidateId={undefined} />
        </DevelopmentRuntimeProvider>,
      );
      await waitFor(() => expect(view.getByText(expectedCopy)).toBeTruthy());
      if (pairing.tag === Bindings.RemoteControlPairingState_Tags.ConfirmationRequired) {
        expect(view.getByText("44".repeat(16))).toBeTruthy();
      }
      expect(JSON.stringify(view.toJSON())).not.toMatch(
        /E290|signed availability|upstream RemoteControl/iu,
      );
      view.unmount();
      await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
    }
  });
  it("gives a distinct user remedy for every pairing failure stage", async () => {
    const failures = [
      [Bindings.RemoteControlPairingFailureStage.Input, "Check the invitation and try again."],
      [
        Bindings.RemoteControlPairingFailureStage.Candidate,
        "The node is no longer available. Reopen pairing on the node and try again.",
      ],
      [
        Bindings.RemoteControlPairingFailureStage.Route,
        "No connection path to the node is available yet. Keep its pairing screen open and try again.",
      ],
      [
        Bindings.RemoteControlPairingFailureStage.Link,
        "A secure connection to the node could not be opened. Make sure it is on and nearby, then try again.",
      ],
      [
        Bindings.RemoteControlPairingFailureStage.Identification,
        "The node could not verify this device. Reopen pairing on the node and try again.",
      ],
      [
        Bindings.RemoteControlPairingFailureStage.Timeout,
        "The node did not respond in time. Reopen pairing and try again.",
      ],
      [
        Bindings.RemoteControlPairingFailureStage.Request,
        "The node could not complete the pairing request. Keep both devices nearby and try again.",
      ],
      [
        Bindings.RemoteControlPairingFailureStage.Confirmation,
        "Confirmation could not be completed. Check both devices and try again.",
      ],
      [
        Bindings.RemoteControlPairingFailureStage.Persistence,
        "Pairing could not be saved. Try again.",
      ],
      [
        Bindings.RemoteControlPairingFailureStage.Expired,
        "The node is no longer available. Reopen pairing on the node and try again.",
      ],
      [
        Bindings.RemoteControlPairingFailureStage.Node,
        "This device went offline during pairing. Wait a moment and try again.",
      ],
    ] as const satisfies readonly (readonly [
      Extract<
        PairingState,
        {
          readonly tag: "Failed";
        }
      >["inner"]["stage"],
      string,
    ])[];
    for (const [stage, expectedCopy] of failures) {
      const stop = jest.fn();
      const view = render(
        <DevelopmentRuntimeProvider
          provider={fakeProvider(
            stop,
            {},
            false,
            Bindings.RemoteControlPairingState.Failed.new({
              stage,
              detail: "internal protocol detail",
            }),
          )}
          refreshIntervalMillis={50}
        >
          <PairNodeScreen selectedCandidateId={undefined} />
        </DevelopmentRuntimeProvider>,
      );
      await waitFor(() => expect(view.getByText(expectedCopy)).toBeTruthy());
      expect(view.queryByText("internal protocol detail")).toBeNull();
      view.unmount();
      await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
    }
  });
  it("does not expose native failure details when checking a managed node", async () => {
    const stop = jest.fn();
    const describeRemoteControlTarget = jest.fn(async () =>
      Bindings.RemoteControlDescribeOutcome.Failed.new({
        stage: Bindings.RemoteControlDescribeFailureStage.Link,
        detail: "E290 upstream RemoteControl lost signed availability on its bounded event lane",
      }),
    );
    const target = {
      targetIdentityFingerprint: observedIdentity,
      destination: observedDestination,
      controllerIdentityFingerprint: identityHash(new Uint8Array(16).fill(0x55)),
      permittedRequests: [Bindings.RemoteControlRequestKind.Describe],
    };
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { describeRemoteControlTarget },
          false,
          Bindings.RemoteControlPairingState.Searching.new(),
          [target],
        )}
        refreshIntervalMillis={50}
      >
        <ManagedNodeScreen />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() =>
      expect(view.getByRole("button", { name: "Check node connection" })).toBeTruthy(),
    );
    fireEvent.press(view.getByRole("button", { name: "Check node connection" }));
    await waitFor(() => expect(view.getByText("Could not check node")).toBeTruthy());
    expect(view.getByText("The connection check could not complete. Try again.")).toBeTruthy();
    expect(
      view.queryAllByText(/E290|signed availability|upstream RemoteControl|bounded event lane/iu),
    ).toHaveLength(0);
    expect(describeRemoteControlTarget).toHaveBeenCalledWith({
      targetIdentityFingerprint: observedIdentity,
    });
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
  it("keeps a managed route truthful when Stop clears its current paired-node inventory", async () => {
    const fixture = androidRestartFixture();
    const target = {
      targetIdentityFingerprint: observedIdentity,
      destination: observedDestination,
      controllerIdentityFingerprint: identityHash(new Uint8Array(16).fill(0x55)),
      permittedRequests: [Bindings.RemoteControlRequestKind.Describe],
    };
    const view = render(
      <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
        <ManagedNodeScreen />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(fixture.start).toHaveBeenCalledTimes(1));
    await act(async () =>
      fixture.emit(
        snapshot(3n, false, Bindings.RemoteControlPairingState.Searching.new(), [target]),
      ),
    );
    expect(view.getByRole("button", { name: "Check node connection" })).toBeTruthy();
    await act(async () =>
      fixture.emit({ ...stoppedSnapshot(4n), runtime: Bindings.DevelopmentNodeRuntime.Stopping }),
    );
    expect(view.getByText("This device's node is stopping")).toBeTruthy();
    expect(view.getByText("Back to Nodes")).toBeTruthy();
    expect(view.queryByText("Not found")).toBeNull();
    expect(view.queryByRole("button", { name: "Check node connection" })).toBeNull();
    await act(async () => fixture.emit(stoppedSnapshot(5n)));
    expect(view.getByText("This device's node is stopped")).toBeTruthy();
    expect(
      view.getByText("Return to Nodes and start this device's node to manage your paired nodes."),
    ).toBeTruthy();
    expect(view.queryByText("Not found")).toBeNull();
    expect(view.queryByText("This page is not available")).toBeNull();
    await act(async () =>
      fixture.emit(
        snapshot(6n, false, Bindings.RemoteControlPairingState.Searching.new(), [target]),
      ),
    );
    expect(view.getByRole("button", { name: "Check node connection" })).toBeTruthy();
    expect(fixture.start).toHaveBeenCalledTimes(1);
    view.unmount();
  });
  it.each(["malformed", "empty", "multiple", "missing"] as const)(
    "keeps a %s managed route not found",
    async (kind) => {
      const fixture = androidRestartFixture();
      if (kind === "malformed")
        jest.mocked(useLocalSearchParams).mockReturnValue({ nodeId: "not-a-node-id" });
      if (kind === "empty") jest.mocked(useLocalSearchParams).mockReturnValue({ nodeId: "" });
      if (kind === "multiple")
        jest
          .mocked(useLocalSearchParams)
          .mockReturnValue({ nodeId: ["44".repeat(16), "55".repeat(16)] });
      const view = render(
        <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
          <ManagedNodeScreen />
        </DevelopmentRuntimeProvider>,
      );
      await waitFor(() => expect(fixture.start).toHaveBeenCalledTimes(1));
      if (kind !== "missing") await act(async () => fixture.emit(stoppedSnapshot()));
      await waitFor(() => expect(view.getByText("Not found")).toBeTruthy());
      expect(view.queryByText("This device's node is stopped")).toBeNull();
      expect(view.getByRole("button", { name: "Return to a safe screen" })).toBeTruthy();
      view.unmount();
    },
  );
  it.each([
    [null, "Node details unavailable"],
    [Bindings.DevelopmentNodeRuntime.Starting, "This device's node is starting"],
    [Bindings.DevelopmentNodeRuntime.Failed, "This device's node is unavailable"],
  ] as const)(
    "does not infer a missing pairing when an acquired view has a %s snapshot",
    async (state, guidance) => {
      const fixture = androidRestartFixture();
      const publish = jest.fn<void, [DevelopmentRuntimeView]>();
      const providerView = render(
        <DevelopmentRuntimeProvider provider={fixture.provider} refreshIntervalMillis={60000}>
          <RuntimeViewProbe publish={publish} />
        </DevelopmentRuntimeProvider>,
      );
      await waitFor(() => expect(publish.mock.calls.at(-1)?.[0].phase).toBe("ready"));
      const ready = publish.mock.calls.at(-1)?.[0];
      if (ready === undefined) throw new Error("expected acquired view");
      const runtimeView = jest
        .spyOn(developmentRuntimeContext, "useDevelopmentRuntime")
        .mockReturnValue({
          ...ready,
          snapshot: state === null ? null : { ...stoppedSnapshot(), runtime: state },
        });
      const view = render(<ManagedNodeScreen />);
      expect(view.getByText(guidance)).toBeTruthy();
      expect(view.getByText("Back to Nodes")).toBeTruthy();
      expect(view.queryByText("Not found")).toBeNull();
      view.unmount();
      runtimeView.mockRestore();
      providerView.unmount();
    },
  );
  it("saves a live authenticated observation from device diagnostics", async () => {
    const stop = jest.fn();
    const saveObservedDestination = jest.fn(async () =>
      Bindings.ContactMutationOutcome.Saved.new({
        contact: {
          destination: observedDestination,
          identity: observedIdentity,
          alias: undefined,
          pinned: false,
        },
      }),
    );
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(stop, { saveObservedDestination }, true)}
        refreshIntervalMillis={50}
      >
        <LocalNodeScreen />
      </DevelopmentRuntimeProvider>,
    );
    await waitFor(() => expect(view.getByRole("button", { name: "Save as contact" })).toBeTruthy());
    fireEvent.press(view.getByRole("button", { name: "Save as contact" }));
    await waitFor(() => expect(saveObservedDestination).toHaveBeenCalledWith(observedDestination));
    expect(view.getByText("The verified destination was saved.")).toBeTruthy();
    expect(view.getByText("Open saved contact")).toBeTruthy();
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
});
