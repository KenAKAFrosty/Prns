import * as Bindings from "@prns-internal/expo";
import type { DevelopmentNodeSnapshot } from "@prns-internal/expo";
import { act, fireEvent, render } from "@testing-library/react-native";
import type { ReactNode } from "react";
import { StyleSheet } from "react-native";
import { destinationHash, identityHash, interfaceId } from "personal-rns/contract";

import type { DevelopmentRuntimeView } from "@/native/development-runtime-context";
import { formatBytes } from "@/features/nodes/format";
import { announceIngressName, logicalInterfaceName } from "./format";
import { NetworkScreen } from "./network-screen";

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
}));
jest.mock("@/native/development-runtime-context", () => ({
  useDevelopmentRuntime: () => mockRuntime,
}));
jest.mock("@/features/contacts/messaging-directory", () => ({
  useMessagingDirectory: () => mockDirectory,
}));
let mockDirectory: {
  contacts: Bindings.Contact[];
  peers: Bindings.LxmfPeerSummary[];
  refresh: () => Promise<void>;
};
let mockFontScale = 1;
jest.mock("react-native/Libraries/Utilities/useWindowDimensions", () => ({
  __esModule: true,
  default: () => ({ width: 320, height: 640, scale: 2, fontScale: mockFontScale }),
}));

type Network = DevelopmentNodeSnapshot["network"];
type NetworkRuntime = Pick<
  DevelopmentRuntimeView,
  | "availability"
  | "phase"
  | "snapshot"
  | "bluetoothAuthorization"
  | "bluetoothAuthorizationFailure"
  | "androidRuntime"
  | "canStartNode"
  | "startNode"
  | "refreshSnapshot"
  | "clearNetworkActivity"
  | "networkActivityClear"
>;
const id = (byte: number, length = 16) => new Uint8Array(length).fill(byte);
const route = (overrides: Partial<Network["routes"][number]> = {}): Network["routes"][number] => ({
  destination: id(0x11),
  viaIdentity: undefined,
  interfaceId: id(0x22, 8),
  hops: 1,
  learnedAgeMillis: 120_000n,
  lastActivityAgeMillis: 20_000n,
  expiresInMillis: 3_600_000n,
  expired: false,
  ...overrides,
});
const announcement = (
  overrides: Partial<Network["announces"][number]> = {},
): Network["announces"][number] => ({
  recordId: 1n,
  destination: id(0x33),
  announcedIdentity: id(0x44),
  sourceInterface: id(0x55, 8),
  hops: 1,
  ageMillis: 30_000n,
  isPathResponse: false,
  ...overrides,
});
const peer: DevelopmentNodeSnapshot["bluetooth"]["peers"][number] = {
  interfaceId: id(0x55, 8),
  name: "Nearby phone",
  connected: true,
  rxBytes: 13n,
  txBytes: 21n,
  details: undefined,
  rssiDbm: -60,
};

function snapshot(network: Partial<Network> = {}): DevelopmentNodeSnapshot {
  return {
    contractFingerprint: "test",
    revision: 1n,
    generationId: 1n,
    runtime: Bindings.DevelopmentNodeRuntime.Running,
    primaryIdentity: Bindings.PrimaryIdentityState.Missing.new(),
    localHost: Bindings.LocalHostState.Stopped.new({ lastStartFailure: undefined }),
    network: {
      state: Bindings.LocalNetworkState.Ready.new(),
      routes: [route()],
      announces: [announcement()],
      activityRevision: 1n,
      droppedAnnounceCount: 0n,
      ...network,
    },
    bluetooth: {
      desiredEnabled: true,
      state: Bindings.LocalBluetoothState.Connected.new(),
      peers: [peer],
    },
    lxmf: { state: Bindings.LxmfHealthState.Ready, inboundOverflowCount: 0n },
    controllerIdentityFingerprint: undefined,
    pairing: Bindings.RemoteControlPairingState.Searching.new(),
    pairingCandidates: [],
    pairedTargets: [],
    lastAnnouncement: undefined,
    lastRemoteChange: undefined,
    activeOperation: undefined,
    failure: undefined,
  };
}

const mockClear = jest.fn<
  ReturnType<NetworkRuntime["clearNetworkActivity"]>,
  Parameters<NetworkRuntime["clearNetworkActivity"]>
>();
const mockRefresh = jest.fn<ReturnType<NetworkRuntime["refreshSnapshot"]>, []>();
let mockRuntime: { -readonly [Key in keyof NetworkRuntime]: NetworkRuntime[Key] };
beforeEach(() => {
  jest.clearAllMocks();
  mockFontScale = 1;
  mockDirectory = { contacts: [], peers: [], refresh: jest.fn(async () => {}) };
  mockClear.mockResolvedValue({
    type: "outcome",
    outcome: Bindings.ClearNetworkActivityOutcome.Cleared.new({ activityRevision: 2n }),
  });
  mockRefresh.mockResolvedValue({ type: "outcome", outcome: snapshot() });
  mockRuntime = {
    availability: { type: "available", platform: "ios" },
    phase: "ready",
    snapshot: snapshot(),
    bluetoothAuthorization: {
      authorization: "allowedAlways",
      nativeStart: "running",
      restorationAttemptRequested: false,
      revision: 1,
    },
    bluetoothAuthorizationFailure: null,
    androidRuntime: null,
    canStartNode: false,
    startNode: jest.fn(),
    refreshSnapshot: mockRefresh,
    clearNetworkActivity: mockClear,
    networkActivityClear: null,
  };
});
afterEach(() => jest.restoreAllMocks());

test("keeps connection, route and announcement counts independent and details secondary", () => {
  mockRuntime.snapshot = snapshot({ routes: [route(), route({ destination: id(0x66) })] });
  const view = render(<NetworkScreen />);
  expect(view.getByRole("tab", { name: "Connections 1" })).toBeTruthy();
  expect(view.getByRole("tab", { name: "Routes 2" })).toBeTruthy();
  expect(view.getByRole("tab", { name: "Announcements 1" })).toBeTruthy();
  expect(view.getByText("This connection: received 13 B · sent 21 B")).toBeTruthy();
  expect(view.queryByText(formatBytes(peer.interfaceId))).toBeNull();
  fireEvent.press(view.getByRole("button", { name: "Connection details for Nearby phone" }));
  expect(view.getByText(formatBytes(peer.interfaceId))).toBeTruthy();
  expect(view.getByText("Received on this connection")).toBeTruthy();
  expect(mockRefresh).not.toHaveBeenCalled();
  expect(mockClear).not.toHaveBeenCalled();
  expect(mockRuntime.startNode).not.toHaveBeenCalled();
});

test("hides stale physical peers when the radio is off without deleting route evidence", () => {
  mockRuntime.snapshot = {
    ...snapshot(),
    bluetooth: { ...snapshot().bluetooth, state: Bindings.LocalBluetoothState.RadioOff.new() },
  };
  const view = render(<NetworkScreen />);
  expect(view.getByRole("tab", { name: "Connections 0" })).toBeTruthy();
  expect(view.getByRole("tab", { name: "Routes 1" })).toBeTruthy();
  expect(view.getByText("Bluetooth is off")).toBeTruthy();
  expect(view.queryByText("Nearby phone")).toBeNull();
});

test("zero known routes does not imply an offline node", () => {
  mockRuntime.snapshot = snapshot({ routes: [] });
  const view = render(<NetworkScreen initialTab="routes" />);
  expect(view.getByText("Running")).toBeTruthy();
  expect(view.getByText(/^No routes recorded yet/)).toBeTruthy();
  expect(view.getByRole("tab", { name: "Connections 1" })).toBeTruthy();
  expect(view.queryByText(/offline/i)).toBeNull();
});

test("route durations stay at the native inspection and absent relay is not called direct", () => {
  const view = render(<NetworkScreen initialTab="routes" />);
  fireEvent.press(view.getByRole("button", { name: /Route details for/ }));
  expect(view.getByText("2m ago")).toBeTruthy();
  expect(view.getByText("20s ago")).toBeTruthy();
  expect(view.getByText("Not recorded")).toBeTruthy();
  expect(view.queryByText("Direct")).toBeNull();
  expect(view.getByText("Times reflect the last network update.")).toBeTruthy();
  jest.spyOn(Date, "now").mockReturnValue(Number.MAX_SAFE_INTEGER);
  mockRuntime.snapshot = { ...snapshot(), revision: 99n };
  view.rerender(<NetworkScreen initialTab="routes" />);
  expect(view.getByText("2m ago")).toBeTruthy();
  expect(view.getByText(/Expires in 1h 0m/)).toBeTruthy();
});

test("route inspection failure leaves earlier announcements available and hides technical errors", () => {
  mockRuntime.snapshot = snapshot({
    state: Bindings.LocalNetworkState.Unavailable.new({ detail: "internal lock error" }),
    routes: [],
  });
  const view = render(<NetworkScreen initialTab="routes" />);
  expect(view.getByText(/Connections may still be working/)).toBeTruthy();
  expect(view.queryByText("internal lock error")).toBeNull();
  fireEvent.press(view.getByRole("tab", { name: "Announcements 1" }));
  expect(view.getByText(/Earlier announcements are still shown/)).toBeTruthy();
  expect(view.getByText("30s ago · 1 hop · Nearby phone")).toBeTruthy();
});

test("distinguishes stopped, starting and unsupported inspection", () => {
  mockRuntime.snapshot = {
    ...snapshot({ state: Bindings.LocalNetworkState.Stopped.new(), routes: [], announces: [] }),
    runtime: Bindings.DevelopmentNodeRuntime.Stopped,
  };
  mockRuntime.canStartNode = true;
  const view = render(<NetworkScreen initialTab="routes" />);
  expect(view.getByText(/Start this phone's node to inspect/)).toBeTruthy();
  fireEvent.press(view.getByRole("button", { name: "Start node" }));
  expect(mockRuntime.startNode).toHaveBeenCalledTimes(1);
  mockRuntime.snapshot = snapshot({
    state: Bindings.LocalNetworkState.Starting.new(),
    routes: [],
    announces: [],
  });
  view.rerender(<NetworkScreen initialTab="routes" />);
  expect(view.getByText("Preparing the network view…")).toBeTruthy();
  mockRuntime.availability = { type: "unavailable", platform: "web", reason: "notImplemented" };
  view.rerender(<NetworkScreen initialTab="routes" />);
  expect(view.getByText("Network inspection needs the iOS or Android app.")).toBeTruthy();
  expect(view.getByRole("button", { name: "Refresh network" })).toBeDisabled();
});

test("resolves recorded announcement ingress, not the present route", () => {
  const data = snapshot({ announces: [announcement({ sourceInterface: id(0xaa, 8) })] });
  mockRuntime.snapshot = data;
  expect(announceIngressName(data, peer.interfaceId)).toBe("Nearby phone");
  expect(
    announceIngressName(
      {
        ...data,
        bluetooth: { ...data.bluetooth, peers: [{ ...peer, name: undefined }] },
      },
      peer.interfaceId,
    ),
  ).toBe("Bluetooth connection");
  const view = render(<NetworkScreen initialTab="announcements" />);
  expect(view.getByText(/Interface details unavailable/)).toBeTruthy();
  fireEvent.press(view.getByRole("button", { name: /Announcement details/ }));
  expect(view.getByText("Recorded interface ID")).toBeTruthy();
  expect(view.getByText(formatBytes(id(0xaa, 8)))).toBeTruthy();
  expect(view.queryByText(formatBytes(route().interfaceId))).toBeNull();
});

test("distinguishes an unlisted ingress from unavailable inspection and logical interfaces from peers", () => {
  const data = snapshot();
  const inspected = {
    ...data,
    localHost: Bindings.LocalHostState.Running.new({
      host: {
        revision: 1n,
        backend: {
          backend: "Native",
          capabilities: ["Bluetooth"],
          interfaceKinds: ["AutomaticBluetoothLe"],
        },
        interfaces: [
          {
            interfaceId: interfaceId(id(0x22, 8)),
            name: "Auto",
            kind: "AutomaticBluetoothLe",
            health: "Connected",
            rxBytes: 0n,
            txBytes: 0n,
            routeCount: 1,
            linkCount: 0,
            transportedLinkCount: 0,
          },
        ],
        routes: [],
        activeLinkCount: 0,
        destinationIdentities: [],
        runtime: {
          running: true,
          uptimeMillis: 1,
          interfaceCount: 1,
          onlineInterfaceCount: 1,
          routeCount: 1,
          linkCount: 0,
          transportedLinkCount: 0,
          rxBytes: 0n,
          txBytes: 0n,
          rxBps: 0,
          txBps: 0,
        },
        persistence: { persistent: true, restored: true, lastFlushCause: "Startup" },
      },
    }),
  };
  expect(announceIngressName(inspected, id(0x99, 8))).toBe("Interface no longer listed");
  expect(announceIngressName(inspected, id(0x22, 8))).toBe("Bluetooth");
  expect(logicalInterfaceName(inspected, peer.interfaceId)).toBe("Interface not listed");
  expect(
    announceIngressName(
      { ...inspected, localHost: Bindings.LocalHostState.Unavailable.new({ detail: "busy" }) },
      id(0x99, 8),
    ),
  ).toBe("Interface details unavailable");
});

test("uses current saved and discovered names without changing recorded announce identities", () => {
  mockDirectory.contacts = [
    {
      destination: destinationHash(route().destination),
      identity: identityHash(id(0x77)),
      alias: "Alice",
      announcedName: "Old Alice",
      pinned: false,
      isMessaging: true,
    },
  ];
  const discovered: Bindings.LxmfPeerSummary = {
    destination: announcement().destination,
    identity: id(0x88),
    displayName: "Bob",
    requiredStampCost: undefined,
    sourceInterface: peer.interfaceId,
    hops: 1,
    isPathResponse: false,
    lastObservedAgeMillis: 0n,
  };
  mockDirectory.peers = [discovered];
  const view = render(<NetworkScreen initialTab="routes" />);
  expect(view.getByRole("button", { name: "Route details for Alice" })).toBeTruthy();
  fireEvent.press(view.getByRole("tab", { name: "Announcements 1" }));
  fireEvent.press(view.getByRole("button", { name: "Announcement details for Bob, entry 1" }));
  expect(view.getByText(formatBytes(announcement().announcedIdentity))).toBeTruthy();
  expect(view.queryByText(formatBytes(id(0x88)))).toBeNull();
  expect(view.getByText("Names come from your contacts and latest discovery.")).toBeTruthy();
  mockDirectory.peers = [{ ...discovered, displayName: "Robert" }];
  view.rerender(<NetworkScreen initialTab="routes" />);
  expect(
    view.getByRole("button", { name: "Announcement details for Robert, entry 1" }),
  ).toBeTruthy();
});

test("pages the bounded ring and identifies retention evictions, not network loss", () => {
  mockRuntime.snapshot = snapshot({
    announces: Array.from({ length: 200 }, (_, index) =>
      announcement({ recordId: BigInt(200 - index) }),
    ),
    droppedAnnounceCount: 15n,
  });
  const view = render(<NetworkScreen initialTab="announcements" />);
  expect(view.getAllByRole("button", { name: /Announcement details/ })).toHaveLength(20);
  expect(view.getByText("15 older entries removed. This list keeps the latest 200.")).toBeTruthy();
  expect(view.queryByText(/packet loss|dropped packets/i)).toBeNull();
  fireEvent.press(view.getByRole("button", { name: "Show more (180 remaining)" }));
  expect(view.getAllByRole("button", { name: /Announcement details/ })).toHaveLength(40);
});

test("describes an empty cleared history without claiming no announcements since startup", () => {
  mockRuntime.snapshot = snapshot({ announces: [], activityRevision: 2n });
  mockRuntime.networkActivityClear = { generationId: 1n, activityRevision: 2n };
  const view = render(<NetworkScreen initialTab="announcements" />);
  expect(
    view.getByText(
      /Recent announcements received by this phone\. Keeps up to 200 entries until cleared or the node restarts\./,
    ),
  ).toBeTruthy();
  expect(
    view.getByText("No announcements in this history. Ask a nearby node to announce."),
  ).toBeTruthy();
  expect(view.queryByText(/since the node started|No announcements heard yet/)).toBeNull();
});

test("waits for clear acknowledgement, suppresses only older snapshots, and preserves new entries", async () => {
  let finish:
    | ((value: Awaited<ReturnType<NetworkRuntime["clearNetworkActivity"]>>) => void)
    | undefined;
  mockClear.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<NetworkScreen initialTab="announcements" />);
  const clearButton = view.getByRole("button", { name: "Clear announcement history" });
  fireEvent.press(clearButton);
  fireEvent.press(clearButton);
  expect(mockClear).toHaveBeenCalledTimes(1);
  expect(mockClear).toHaveBeenCalledWith({ generationId: 1n });
  expect(view.getAllByRole("button", { name: /Announcement details/ })).toHaveLength(1);
  await act(async () => {
    finish?.({
      type: "outcome",
      outcome: Bindings.ClearNetworkActivityOutcome.Cleared.new({ activityRevision: 2n }),
    });
  });
  expect(view.queryByRole("button", { name: /Announcement details/ })).toBeNull();
  expect(view.getByText(/History cleared. Refresh/)).toBeTruthy();
  // Even a failed subsequent refresh cannot resurrect a pre-clear projection.
  mockRefresh.mockResolvedValueOnce({ type: "operationFailure", detail: "unavailable" });
  await act(async () => {
    fireEvent.press(view.getByRole("button", { name: "Refresh network" }));
  });
  expect(view.queryByRole("button", { name: /Announcement details/ })).toBeNull();
  mockRuntime.snapshot = snapshot({
    activityRevision: 3n,
    announces: [announcement({ recordId: 2n })],
  });
  view.rerender(<NetworkScreen initialTab="announcements" />);
  expect(view.getByRole("button", { name: /entry 2$/ })).toBeTruthy();
  expect(view.queryByText(/History cleared. Refresh/)).toBeNull();
});

test("drops clear feedback, watermark and expanded state on a new node generation", async () => {
  let finish:
    | ((value: Awaited<ReturnType<NetworkRuntime["clearNetworkActivity"]>>) => void)
    | undefined;
  mockClear.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(<NetworkScreen initialTab="announcements" />);
  fireEvent.press(view.getByRole("button", { name: /Announcement details/ }));
  fireEvent.press(view.getByRole("button", { name: "Clear announcement history" }));
  mockRuntime.snapshot = { ...snapshot(), generationId: 2n };
  view.rerender(<NetworkScreen initialTab="announcements" />);
  await act(async () => {
    finish?.({
      type: "outcome",
      outcome: Bindings.ClearNetworkActivityOutcome.Cleared.new({ activityRevision: 99n }),
    });
  });
  expect(view.queryByText(/history cleared/i)).toBeNull();
  expect(view.queryByText("Announced identity")).toBeNull();
  expect(view.getByRole("button", { name: /Announcement details/ })).toBeTruthy();
  expect(view.getByRole("button", { name: "Clear announcement history" })).toBeEnabled();
});

test("retains the provider clear barrier across navigation without masking a new generation", () => {
  mockRuntime.networkActivityClear = { generationId: 1n, activityRevision: 2n };
  const firstVisit = render(<NetworkScreen initialTab="announcements" />);
  expect(firstVisit.queryByRole("button", { name: /Announcement details/ })).toBeNull();
  firstVisit.unmount();
  const nextVisit = render(<NetworkScreen initialTab="announcements" />);
  expect(nextVisit.getByText(/History cleared. Refresh/)).toBeTruthy();
  expect(nextVisit.queryByRole("button", { name: /Announcement details/ })).toBeNull();
  mockRuntime.snapshot = snapshot({
    activityRevision: 3n,
    announces: [announcement({ recordId: 2n })],
  });
  nextVisit.rerender(<NetworkScreen initialTab="announcements" />);
  expect(nextVisit.getByRole("button", { name: /entry 2$/ })).toBeTruthy();
  mockRuntime.snapshot = { ...snapshot(), generationId: 2n };
  nextVisit.rerender(<NetworkScreen initialTab="announcements" />);
  expect(nextVisit.getByRole("button", { name: /entry 1$/ })).toBeTruthy();
  expect(nextVisit.queryByText(/History cleared. Refresh/)).toBeNull();
});

test("a previous clear acknowledgement does not conceal the stopped-node state", () => {
  mockRuntime.networkActivityClear = { generationId: 1n, activityRevision: 2n };
  mockRuntime.snapshot = {
    ...snapshot({
      state: Bindings.LocalNetworkState.Stopped.new(),
      activityRevision: 0n,
      announces: [],
      routes: [],
    }),
    runtime: Bindings.DevelopmentNodeRuntime.Stopped,
  };
  const view = render(<NetworkScreen initialTab="announcements" />);
  expect(view.getByText(/Start this phone's node to inspect/)).toBeTruthy();
  expect(view.queryByText(/History cleared. Refresh/)).toBeNull();
});

test.each([
  [
    Bindings.ClearNetworkActivityOutcome.Busy.new(),
    "The node is busy. Wait a moment, then try again.",
  ],
  [
    Bindings.ClearNetworkActivityOutcome.GenerationChanged.new(),
    "The node restarted. Refresh before clearing its new history.",
  ],
  [
    Bindings.ClearNetworkActivityOutcome.LocalNodeStopped.new(),
    "The node has stopped. Its announcement history is no longer active.",
  ],
])("reports clear outcome %s without removing displayed rows", async (outcome, expected) => {
  mockClear.mockResolvedValueOnce({ type: "outcome", outcome });
  const view = render(<NetworkScreen initialTab="announcements" />);
  await act(async () => {
    fireEvent.press(view.getByRole("button", { name: "Clear announcement history" }));
  });
  expect(view.getByText(expected)).toBeTruthy();
  expect(view.getByRole("button", { name: /Announcement details/ })).toBeTruthy();
});

test("clear failure leaves history unchanged and presents a user-facing error", async () => {
  mockClear.mockRejectedValueOnce(new Error("native detail"));
  const view = render(<NetworkScreen initialTab="announcements" />);
  await act(async () => {
    fireEvent.press(view.getByRole("button", { name: "Clear announcement history" }));
  });
  expect(view.getByText("Announcement history could not be cleared. Try again.")).toBeTruthy();
  expect(view.queryByText("native detail")).toBeNull();
  expect(view.getByRole("button", { name: /Announcement details/ })).toBeTruthy();
});

test("large-text tabs wrap and retain accessible touch targets", () => {
  mockFontScale = 1.6;
  const view = render(<NetworkScreen />);
  for (const tab of view.getAllByRole("tab")) {
    expect(StyleSheet.flatten(tab.props.style)).toMatchObject({ minHeight: 48, flexBasis: "100%" });
  }
  expect(view.getByRole("tab", { name: "Connections 1" }).props.accessibilityState).toEqual({
    selected: true,
  });
});
