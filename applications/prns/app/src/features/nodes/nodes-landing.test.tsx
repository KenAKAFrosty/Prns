import * as Bindings from "@prns-internal/expo";
import type { DevelopmentNodeSnapshot } from "@prns-internal/expo";
import { act, fireEvent, render } from "@testing-library/react-native";
import { Link } from "expo-router";
import type { ReactNode } from "react";
import { destinationHash, identityHash } from "personal-rns/contract";

import type { DevelopmentRuntimeView } from "@/native/development-runtime-context";
import { NodesScreen } from "./nodes-screen";

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
}));

jest.mock("@/native/development-runtime-context", () => ({
  useDevelopmentRuntime: () => mockRuntime,
}));

type LandingRuntime = Pick<
  DevelopmentRuntimeView,
  | "availability"
  | "phase"
  | "snapshot"
  | "accessorySetup"
  | "accessorySetupFailure"
  | "backgroundFailure"
  | "androidRuntime"
  | "canStartNode"
  | "startNode"
  | "canStopNode"
  | "stoppingNode"
  | "stopFailure"
  | "stopNode"
  | "refreshSnapshot"
>;

function target(byte: number): DevelopmentNodeSnapshot["pairedTargets"][number] {
  return {
    targetIdentityFingerprint: identityHash(new Uint8Array(16).fill(byte)),
    destination: destinationHash(new Uint8Array(16).fill(byte + 1)),
    controllerIdentityFingerprint: identityHash(new Uint8Array(16).fill(0x55)),
    permittedRequests: [Bindings.RemoteControlRequestKind.Describe],
  };
}

function snapshot(
  runtime = Bindings.DevelopmentNodeRuntime.Running,
  pairedTargets = [target(0x11)],
): DevelopmentNodeSnapshot {
  return {
    contractFingerprint: "test-contract",
    revision: 1n,
    runtime,
    primaryIdentity: Bindings.PrimaryIdentityState.Missing.new(),
    localHost: Bindings.LocalHostState.Stopped.new({ lastStartFailure: undefined }),
    lxmf: { state: Bindings.LxmfHealthState.Ready, inboundOverflowCount: 0n },
    controllerIdentityFingerprint: undefined,
    pairing: Bindings.RemoteControlPairingState.Searching.new(),
    pairingCandidates: [],
    pairedTargets,
    lastAnnouncement: undefined,
    lastRemoteChange: undefined,
    generationId: 1n,
    activeOperation: undefined,
    failure: undefined,
  };
}

const mockRefresh = jest.fn<ReturnType<LandingRuntime["refreshSnapshot"]>, []>();
let mockRuntime: LandingRuntime;

beforeEach(() => {
  jest.clearAllMocks();
  mockRefresh.mockResolvedValue({ type: "outcome", outcome: snapshot() });
  mockRuntime = {
    availability: { type: "available", platform: "android" },
    phase: "ready",
    snapshot: snapshot(),
    accessorySetup: null,
    accessorySetupFailure: null,
    backgroundFailure: null,
    androidRuntime: {
      status: {
        revision: 1,
        bluetoothPermission: "granted",
        backgroundDiscovery: "granted",
        connectionNotification: "enabled",
        bluetoothRadio: "on",
        locationServices: "on",
        service: "running",
        lastError: null,
      },
      failure: null,
      refresh: jest.fn(async () => {}),
      requestBluetoothPermissions: jest.fn(async () => {}),
      requestBackgroundBluetoothPermission: jest.fn(async () => {}),
      requestConnectionNotificationPermission: jest.fn(async () => {}),
    },
    canStartNode: false,
    startNode: jest.fn(),
    canStopNode: true,
    stoppingNode: false,
    stopFailure: null,
    stopNode: jest.fn(async () => {}),
    refreshSnapshot: mockRefresh,
  };
});

test("puts paired nodes and their actions before local status and controls", () => {
  const view = render(<NodesScreen />);
  const headings = view.getAllByRole("header").map((heading) => heading.props.children);
  expect(headings.indexOf("Paired nodes")).toBeLessThan(headings.indexOf("This device"));
  expect(headings.indexOf("Paired nodes")).toBeLessThan(headings.indexOf("Bluetooth access"));
  expect(headings.indexOf("Paired nodes")).toBeLessThan(headings.indexOf("Node controls"));
  expect(view.getByText("Paired")).toBeTruthy();
  expect(view.queryByText("Connected")).toBeNull();
  expect(view.getByRole("link", { name: "Manage node" })).toBeTruthy();
  expect(view.getByRole("link", { name: "Pair a node" })).toBeTruthy();
  expect(view.getByRole("link", { name: "Remote access" })).toBeTruthy();
  expect(view.getByRole("link", { name: "View this device" })).toBeTruthy();
  expect(view.getByText("View device")).toBeTruthy();
  expect(view.queryByText("View this device")).toBeNull();
  expect(view.queryByText("Available controls")).toBeNull();
  expect(view.queryByText("View node information.")).toBeNull();
  expect(view.getByRole("button", { name: "Stop node" })).toBeEnabled();
});

test("keeps each paired node distinguishable and routes its action to that node", () => {
  mockRuntime = { ...mockRuntime, snapshot: snapshot(undefined, [target(0x11), target(0x22)]) };
  const view = render(<NodesScreen />);
  expect(view.getByText("11111111111111111111111111111111")).toBeTruthy();
  expect(view.getByText("22222222222222222222222222222222")).toBeTruthy();
  expect(view.getAllByRole("link", { name: "Manage node" })).toHaveLength(2);
  const managedLinks = view
    .UNSAFE_getAllByType(Link)
    .map((link) => link.props.href)
    .filter((href) => typeof href === "object");
  expect(managedLinks).toEqual([
    { pathname: "/nodes/managed/[nodeId]", params: { nodeId: "11111111111111111111111111111111" } },
    { pathname: "/nodes/managed/[nodeId]", params: { nodeId: "22222222222222222222222222222222" } },
  ]);
});

test("keeps stopped nodes unavailable rather than describing them as an empty pairing list", () => {
  mockRuntime = {
    ...mockRuntime,
    snapshot: snapshot(Bindings.DevelopmentNodeRuntime.Stopped),
    canStartNode: true,
    canStopNode: false,
  };
  const view = render(<NodesScreen />);
  expect(view.getByText("Paired nodes unavailable")).toBeTruthy();
  expect(view.queryByText("No paired nodes")).toBeNull();
  expect(view.queryByRole("link", { name: "Manage node" })).toBeNull();
  expect(view.getByRole("button", { name: "Start node" })).toBeEnabled();
  expect(view.getByRole("button", { name: "Stop node" })).toBeDisabled();
  const headings = view.getAllByRole("header").map((heading) => heading.props.children);
  expect(headings.indexOf("This device's node is stopped")).toBeLessThan(
    headings.indexOf("This device"),
  );
  fireEvent.press(view.getByRole("button", { name: "Start node" }));
  expect(mockRuntime.startNode).toHaveBeenCalledTimes(1);
});

test("keeps the pair action with an empty list and preserves Bluetooth access recovery", () => {
  const androidRuntime = mockRuntime.androidRuntime;
  if (androidRuntime?.status === null || androidRuntime === null)
    throw new Error("Android fixture");
  mockRuntime = {
    ...mockRuntime,
    snapshot: snapshot(undefined, []),
    androidRuntime: {
      ...androidRuntime,
      status: { ...androidRuntime.status, bluetoothPermission: "blocked", bluetoothRadio: "off" },
    },
  };
  const view = render(<NodesScreen />);
  expect(view.getByText("No paired nodes")).toBeTruthy();
  expect(view.getByRole("link", { name: "Pair a node" })).toBeTruthy();
  expect(view.getByRole("button", { name: "Open app settings" })).toBeEnabled();
  expect(
    view.getByText("Turn on Bluetooth in Android Settings, then return to prns."),
  ).toBeTruthy();
});

test("preserves refresh pending/failure feedback without hiding device navigation", async () => {
  let complete:
    | ((result: Awaited<ReturnType<LandingRuntime["refreshSnapshot"]>>) => void)
    | undefined;
  mockRefresh.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  );
  mockRuntime = { ...mockRuntime, backgroundFailure: "test refresh failure" };
  const view = render(<NodesScreen />);
  expect(view.getByText("Automatic refresh failed. Try refreshing again.")).toBeTruthy();
  fireEvent.press(view.getByRole("button", { name: "Refresh now" }));
  expect(view.getByRole("button", { name: "Refreshing…" })).toBeDisabled();
  expect(view.getByRole("link", { name: "View this device" })).toBeTruthy();
  await act(async () => complete?.({ type: "operationFailure", detail: "test failure" }));
  expect(view.getByText("Refresh failed. Try again.")).toBeTruthy();
  expect(view.getByRole("button", { name: "Refresh now" })).toBeEnabled();
});

test("retains startup diagnostics and recovery when no snapshot is available", () => {
  mockRuntime = { ...mockRuntime, phase: "failed", snapshot: null, canStartNode: true };
  const view = render(<NodesScreen />);
  expect(view.getByText("This device's node failed to start")).toBeTruthy();
  expect(view.getByRole("button", { name: "Start node" })).toBeEnabled();
  expect(view.getByRole("link", { name: "View diagnostics" })).toBeTruthy();
  expect(view.getByRole("link", { name: "Pair a node" })).toBeTruthy();
  expect(view.getByRole("link", { name: "Remote access" })).toBeTruthy();
  expect(view.queryByRole("button", { name: "Refresh now" })).toBeNull();
});
