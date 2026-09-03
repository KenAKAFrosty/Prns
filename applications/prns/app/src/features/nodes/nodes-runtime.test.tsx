import type {
  DevelopmentNodeSnapshot,
  DevelopmentRuntime,
  EffectDevelopmentRuntime,
} from "@prns-internal/expo";
import { render, waitFor } from "@testing-library/react-native";
import { Effect } from "effect";
import { identityHash, interfaceId } from "personal-rns/contract";
import type { ReactNode } from "react";

import { DevelopmentRuntimeProvider } from "@/native/development-runtime-context";
import type { RuntimeProvider } from "@/native/runtime-provider.types";
import { NodesScreen } from "./nodes-screen";
import { PairNodeScreen } from "./pair-node-screen";

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
}));

function snapshot(revision: bigint): DevelopmentNodeSnapshot {
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
        destinationIdentities: [],
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
    controllerIdentityFingerprint: null,
    pairing: { type: "searching" },
    pairedTargets: [],
    activeOperation: null,
    failure: null,
  };
}

function fakeProvider(stop: jest.Mock): RuntimeProvider {
  const initial = snapshot(2n);
  const runtime: DevelopmentRuntime = {
    inspectDevelopmentIdentity: async () => initial.primaryIdentity,
    previewIdentityImport: async () => ({ type: "invalidLength" }),
    createGeneratedIdentity: async () => ({ type: "alreadyExists" }),
    createImportedIdentity: async () => ({ type: "alreadyExists" }),
    startDevelopmentNode: async () => ({ type: "started", snapshot: initial }),
    readDevelopmentNodeSnapshot: async () => snapshot(1n),
    initiateRemoteControlPairing: async () => ({ type: "busy" }),
    approveRemoteControlPairing: async () => ({ type: "busy" }),
    rejectRemoteControlPairing: async () => ({ type: "busy" }),
    describeRemoteControlTarget: async () => ({ type: "busy" }),
    stopDevelopmentNode: async () => {
      stop();
      return { type: "stopped" };
    },
    resetDevelopmentData: async () => ({ type: "alreadyStopped" }),
  };
  const effectRuntime: EffectDevelopmentRuntime = {
    startDevelopmentNode: Effect.promise(runtime.startDevelopmentNode),
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
          options.onSnapshot(snapshot(1n));
          return { runtime: effectRuntime, initialSnapshot: initial };
        }),
        () => Effect.promise(runtime.stopDevelopmentNode).pipe(Effect.asVoid),
      ),
  };
}

describe("Foundation 1 Nodes runtime binding", () => {
  it("holds one scoped runtime across the Nodes surface and stops it on layout release", async () => {
    const stop = jest.fn();
    const view = render(
      <DevelopmentRuntimeProvider provider={fakeProvider(stop)} refreshIntervalMillis={50}>
        <NodesScreen />
      </DevelopmentRuntimeProvider>,
    );

    await waitFor(() => expect(view.getAllByText("Running")).toHaveLength(2));
    expect(view.getByText("No persisted targets")).toBeTruthy();
    expect(view.getAllByText("2")).toHaveLength(2);
    expect(view.getByText("Native")).toBeTruthy();

    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });

  it("renders the observed searching state without claiming a pairing result", async () => {
    const stop = jest.fn();
    const view = render(
      <DevelopmentRuntimeProvider provider={fakeProvider(stop)} refreshIntervalMillis={50}>
        <PairNodeScreen selectedCandidateId={undefined} />
      </DevelopmentRuntimeProvider>,
    );

    await waitFor(() => expect(view.getByText("Searching")).toBeTruthy());
    expect(view.getByText("Waiting for signed availability")).toBeTruthy();
    expect(view.getByText("Connected")).toBeTruthy();
    expect(view.queryByText("Authorization persisted")).toBeNull();
    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
});
