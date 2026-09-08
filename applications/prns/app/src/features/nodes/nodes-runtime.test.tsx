import type {
  AccessorySetupRuntime,
  AccessorySetupStatus,
  AndroidRuntimeStatus,
  DevelopmentNodeSnapshot,
  DevelopmentRuntime,
  DevelopmentRuntimeStartError,
  EffectDevelopmentRuntime,
} from "@prns-internal/expo";
import { act, fireEvent, render, waitFor } from "@testing-library/react-native";
import { Effect } from "effect";
import { destinationHash, identityHash, interfaceId } from "personal-rns/contract";
import { type ReactNode, useEffect } from "react";

import {
  DevelopmentRuntimeProvider,
  type DevelopmentRuntimeView,
  routeConsumesDevelopmentSnapshot,
  useDevelopmentRuntime,
} from "@/native/development-runtime-context";
import type { RuntimeProvider } from "@/native/runtime-provider.types";
import { ManagedNodeScreen } from "./managed-node-screen";
import { LocalNodeScreen, NodesScreen } from "./nodes-screen";
import { PairNodeScreen } from "./pair-node-screen";

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
  useLocalSearchParams: () => ({ nodeId: "44444444444444444444444444444444" }),
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
  expiresInMillis = 90_000n,
): PairingCandidates[number] {
  return {
    candidateId,
    displayName,
    observedAtMillis: 1n,
    expiresAtMillis: 2n,
    expiresInMillis,
  };
}

function snapshot(
  revision: bigint,
  includeObservation = false,
  pairing: PairingState = { type: "searching" },
  pairedTargets: DevelopmentNodeSnapshot["pairedTargets"] = [],
  pairingCandidates: PairingCandidates = [],
): DevelopmentNodeSnapshot {
  return {
    contractFingerprint: "test-contract",
    revision,
    runtime: "running",
    primaryIdentity: { type: "present", identityHash: identityHash(new Uint8Array(16).fill(0x11)) },
    localHost: {
      type: "running",
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
    },
    lxmf: { state: "ready", inboundOverflowCount: 0n },
    controllerIdentityFingerprint: null,
    pairing,
    pairingCandidates,
    pairedTargets,
    lastAnnouncement: null,
    generationId: 0n,
    activeOperation: null,
    failure: null,
  };
}

function fakeProvider(
  stop: jest.Mock,
  overrides: Partial<DevelopmentRuntime> = {},
  includeObservation = false,
  pairing: PairingState = { type: "searching" },
  pairedTargets: DevelopmentNodeSnapshot["pairedTargets"] = [],
  pairingCandidates: PairingCandidates = [],
): RuntimeProvider {
  const initial = snapshot(2n, includeObservation, pairing, pairedTargets, pairingCandidates);
  const runtime: DevelopmentRuntime = {
    inspectDevelopmentIdentity: async () => initial.primaryIdentity,
    previewIdentityImport: async () => ({ type: "invalidLength" }),
    createGeneratedIdentity: async () => ({ type: "alreadyExists" }),
    createImportedIdentity: async () => ({ type: "alreadyExists" }),
    startDevelopmentNode: async () => ({ type: "started", snapshot: initial }),
    readDevelopmentNodeSnapshot: async () =>
      snapshot(1n, false, pairing, pairedTargets, pairingCandidates),
    initiateRemoteControlPairing: async () => ({ type: "busy" }),
    approveRemoteControlPairing: async () => ({ type: "busy" }),
    rejectRemoteControlPairing: async () => ({ type: "busy" }),
    describeRemoteControlTarget: async () => ({ type: "busy" }),
    announceRemoteControlTarget: async () => ({ type: "busy" }),
    saveObservedDestination: async () => ({ type: "notObserved" }),
    createManualContact: async () => ({ type: "notFound" }),
    setContactAlias: async () => ({ type: "notFound" }),
    setContactPinned: async () => ({ type: "notFound" }),
    deleteContact: async () => ({ type: "notFound" }),
    getContact: async () => ({ type: "notFound" }),
    listContacts: async () => ({ type: "listed", contacts: [] }),
    listLxmfPeers: async () => ({ type: "listed", peers: [] }),
    listLxmfMessages: async () => ({ type: "listed", messages: [] }),
    retryLxmfMessage: async () => ({ type: "notFound" }),
    cancelLxmfMessage: async () => ({ type: "notFound" }),
    announceLxmf: async () => ({ type: "announced" }),
    measureLxmfText: async () => ({
      type: "measured",
      wireBytes: 113,
      remainingBytes: 318,
    }),
    sendDirectText: async () => ({ type: "accepted", localRecordId: 1n }),
    stopDevelopmentNode: async () => {
      stop();
      return { type: "stopped" };
    },
    resetDevelopmentData: async () => ({ type: "alreadyStopped" }),
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

describe("Foundation 1 Nodes runtime binding", () => {
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
    const failure = Object.assign(new Error(), {
      _tag: "DevelopmentRuntimeStartError",
      stage: "bluetooth",
      detail: "Bluetooth is unavailable in this simulator.",
    }) as unknown as DevelopmentRuntimeStartError;
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
    const listLxmfMessages = jest.fn(async () => ({
      type: "listed" as const,
      messages: [],
    }));
    const retryLxmfMessage = jest.fn(async () => ({ type: "notFound" as const }));
    const cancelLxmfMessage = jest.fn(async () => ({ type: "notFound" as const }));
    const sendDirectText = jest.fn(async () => ({
      type: "accepted" as const,
      localRecordId: 1n,
    }));
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
    expect(await failedView.listLxmfMessages({ peer: null, before: null, limit: 25 })).toEqual({
      type: "outcome",
      outcome: { type: "listed", messages: [] },
    });
    expect(await failedView.retryLxmfMessage(7n)).toEqual({
      type: "outcome",
      outcome: { type: "notFound" },
    });
    expect(await failedView.cancelLxmfMessage(8n)).toEqual({
      type: "outcome",
      outcome: { type: "notFound" },
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
    const describeRemoteControlTarget = jest.fn(async () => ({ type: "busy" as const }));
    const target = {
      targetIdentityFingerprint: observedIdentity,
      destination: observedDestination,
      controllerIdentityFingerprint: identityHash(new Uint8Array(16).fill(0x55)),
      permittedRequests: ["describe" as const],
    };
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { describeRemoteControlTarget },
          false,
          { type: "searching" },
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
    const initiateRemoteControlPairing = jest.fn(async () => ({ type: "busy" as const }));
    const candidates = [
      pairingCandidate("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Kitchen node"),
      pairingCandidate("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Workshop node", 42_000n),
    ];
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { initiateRemoteControlPairing },
          false,
          { type: "searching" },
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
        provider={fakeProvider(stop, {}, false, { type: "searching" }, [], [onlyCandidate])}
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
    const initiateRemoteControlPairing = jest.fn(async () => ({ type: "busy" as const }));
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
          { type: "searching" },
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
    const initial = pairingCandidate(candidateId, "Kitchen node", 42_000n);
    const refreshed = {
      ...initial,
      observedAtMillis: 2n,
      expiresAtMillis: 3n,
      expiresInMillis: 90_000n,
    };
    const readDevelopmentNodeSnapshot = jest.fn(async () =>
      snapshot(3n, false, { type: "searching" }, [], [refreshed]),
    );
    let runtimeView: DevelopmentRuntimeView | undefined;
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { readDevelopmentNodeSnapshot },
          false,
          { type: "searching" },
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
        provider={fakeProvider(stop, {}, false, { type: "searching" }, [], candidates)}
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
      { type: "rejected", detail: "internal rejection" },
      { type: "expired", detail: "internal expiry" },
      { type: "failed", stage: "request", detail: "internal request failure" },
      { type: "paired", attemptId: "attempt-a" },
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
    const initiateRemoteControlPairing = jest.fn(async () => ({ type: "busy" as const }));
    const candidateB = pairingCandidate("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Workshop node");
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { initiateRemoteControlPairing },
          false,
          { type: "paired", attemptId: "attempt-a" },
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
    let finishInitiation!: (outcome: { readonly type: "busy" }) => void;
    const initiateRemoteControlPairing = jest.fn(
      () =>
        new Promise<{ readonly type: "busy" }>((resolve) => {
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
          { type: "searching" },
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

    finishInitiation({ type: "busy" });
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
      [{ type: "bluetoothUnavailable" }, "Bluetooth unavailable"],
      [{ type: "searching" }, "Looking for nearby nodes"],
      [{ type: "invitationSubmitted", candidateId: "candidate-1" }, "Invitation sent"],
      [
        {
          type: "confirmationRequired",
          attemptId: "attempt-1",
          confirmationCode: "123456",
          targetIdentityFingerprint: observedIdentity,
          permissions: ["describe"],
        },
        "Confirmation required",
      ],
      [{ type: "awaitingTargetApproval", attemptId: "attempt-1" }, "Waiting for the node"],
      [{ type: "persisting", attemptId: "attempt-1" }, "Finishing pairing"],
      [{ type: "paired", attemptId: "attempt-1" }, "Paired"],
      [{ type: "rejected", detail: "upstream RemoteControl rejected" }, "Rejected"],
      [{ type: "expired", detail: "signed availability expired" }, "Expired"],
      [{ type: "cancelled" }, "Cancelled"],
      [{ type: "failed", stage: "link", detail: "E290 link failure" }, "Pairing failed"],
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
      if (pairing.type === "confirmationRequired") {
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
      ["input", "Check the invitation and try again."],
      ["candidate", "The node is no longer available. Reopen pairing on the node and try again."],
      [
        "route",
        "No connection path to the node is available yet. Keep its pairing screen open and try again.",
      ],
      [
        "link",
        "A secure connection to the node could not be opened. Make sure it is on and nearby, then try again.",
      ],
      [
        "identification",
        "The node could not verify this device. Reopen pairing on the node and try again.",
      ],
      ["timeout", "The node did not respond in time. Reopen pairing and try again."],
      [
        "request",
        "The node could not complete the pairing request. Keep both devices nearby and try again.",
      ],
      ["confirmation", "Confirmation could not be completed. Check both devices and try again."],
      ["persistence", "Pairing could not be saved. Try again."],
      ["expired", "The node is no longer available. Reopen pairing on the node and try again."],
      ["node", "This device went offline during pairing. Wait a moment and try again."],
    ] as const satisfies readonly (readonly [
      Extract<PairingState, { readonly type: "failed" }>["stage"],
      string,
    ])[];

    for (const [stage, expectedCopy] of failures) {
      const stop = jest.fn();
      const view = render(
        <DevelopmentRuntimeProvider
          provider={fakeProvider(stop, {}, false, {
            type: "failed",
            stage,
            detail: "internal protocol detail",
          })}
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
    const describeRemoteControlTarget = jest.fn(async () => ({
      type: "failed" as const,
      stage: "link" as const,
      detail: "E290 upstream RemoteControl lost signed availability on its bounded event lane",
    }));
    const target = {
      targetIdentityFingerprint: observedIdentity,
      destination: observedDestination,
      controllerIdentityFingerprint: identityHash(new Uint8Array(16).fill(0x55)),
      permittedRequests: ["describe" as const],
    };
    const view = render(
      <DevelopmentRuntimeProvider
        provider={fakeProvider(
          stop,
          { describeRemoteControlTarget },
          false,
          { type: "searching" },
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
    expect(
      view.getByText("A secure connection to this node could not be opened. Try again."),
    ).toBeTruthy();
    expect(
      view.queryAllByText(/E290|signed availability|upstream RemoteControl|bounded event lane/iu),
    ).toHaveLength(0);
    expect(describeRemoteControlTarget).toHaveBeenCalledWith({
      targetIdentityFingerprint: observedIdentity,
    });

    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });

  it("saves a live authenticated observation from device diagnostics", async () => {
    const stop = jest.fn();
    const saveObservedDestination = jest.fn(async () => ({
      type: "saved" as const,
      contact: {
        destination: observedDestination,
        identity: observedIdentity,
        alias: null,
        pinned: false,
      },
    }));
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
