import type {
  DevelopmentNodeSnapshot,
  DevelopmentRuntime,
  DevelopmentRuntimeStartError,
  EffectDevelopmentRuntime,
} from "@prns-internal/expo";
import { fireEvent, render, waitFor } from "@testing-library/react-native";
import { Effect } from "effect";
import { destinationHash, identityHash, interfaceId } from "personal-rns/contract";
import { type ReactNode, useEffect } from "react";

import {
  DevelopmentRuntimeProvider,
  routeConsumesDevelopmentSnapshot,
  type DevelopmentRuntimeView,
  useDevelopmentRuntime,
} from "@/native/development-runtime-context";
import type { RuntimeProvider } from "@/native/runtime-provider.types";
import { LocalNodeScreen, NodesScreen } from "./nodes-screen";
import { PairNodeScreen } from "./pair-node-screen";

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
}));

const observedDestination = destinationHash(new Uint8Array(16).fill(0x33));
const observedIdentity = identityHash(new Uint8Array(16).fill(0x44));
type PairingState = DevelopmentNodeSnapshot["pairing"];

function snapshot(
  revision: bigint,
  includeObservation = false,
  pairing: PairingState = { type: "searching" },
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
    pairedTargets: [],
    activeOperation: null,
    failure: null,
  };
}

function fakeProvider(
  stop: jest.Mock,
  overrides: Partial<DevelopmentRuntime> = {},
  includeObservation = false,
  pairing: PairingState = { type: "searching" },
): RuntimeProvider {
  const initial = snapshot(2n, includeObservation, pairing);
  const runtime: DevelopmentRuntime = {
    inspectDevelopmentIdentity: async () => initial.primaryIdentity,
    previewIdentityImport: async () => ({ type: "invalidLength" }),
    createGeneratedIdentity: async () => ({ type: "alreadyExists" }),
    createImportedIdentity: async () => ({ type: "alreadyExists" }),
    startDevelopmentNode: async () => ({ type: "started", snapshot: initial }),
    readDevelopmentNodeSnapshot: async () => snapshot(1n, false, pairing),
    initiateRemoteControlPairing: async () => ({ type: "busy" }),
    approveRemoteControlPairing: async () => ({ type: "busy" }),
    rejectRemoteControlPairing: async () => ({ type: "busy" }),
    describeRemoteControlTarget: async () => ({ type: "busy" }),
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
    stopDevelopmentNode: Effect.promise(runtime.stopDevelopmentNode),
    resetDevelopmentData: Effect.promise(runtime.resetDevelopmentData),
  };
  return {
    availability: { type: "available", platform: "ios" },
    runtime,
    acquire: (options) =>
      Effect.acquireRelease(
        Effect.sync(() => {
          options.onSnapshot(initial);
          options.onSnapshot(snapshot(1n, includeObservation, pairing));
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
  it("polls snapshots only for visible Nodes and Inbox routes", () => {
    expect(routeConsumesDevelopmentSnapshot("/nodes")).toBe(true);
    expect(routeConsumesDevelopmentSnapshot("/nodes/pair")).toBe(true);
    expect(routeConsumesDevelopmentSnapshot("/inbox")).toBe(true);
    expect(routeConsumesDevelopmentSnapshot("/inbox/0011")).toBe(true);
    expect(routeConsumesDevelopmentSnapshot("/")).toBe(false);
    expect(routeConsumesDevelopmentSnapshot("/settings")).toBe(false);
    expect(routeConsumesDevelopmentSnapshot("/network/interfaces")).toBe(false);
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

  it("keeps every pairing step board-neutral and free of protocol narration", async () => {
    const pairingViews = [
      [{ type: "bluetoothUnavailable" }, "Bluetooth unavailable"],
      [{ type: "searching" }, "Looking for nearby nodes"],
      [
        {
          type: "candidateObserved",
          candidate: {
            candidateId: "candidate-1",
            endpoint: observedDestination,
            observedAtMillis: 1n,
            expiresAtMillis: 2n,
            publicAppData: new Uint8Array(),
          },
        },
        "Node found",
      ],
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
      expect(JSON.stringify(view.toJSON())).not.toMatch(
        /E290|signed availability|upstream RemoteControl/iu,
      );
      view.unmount();
      await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
    }
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
