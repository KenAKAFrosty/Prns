import * as Bindings from "@prns-internal/expo";
import type { DevelopmentNodeSnapshot } from "@prns-internal/expo";
import { act, fireEvent, render } from "@testing-library/react-native";
import type { ReactNode } from "react";
import { Linking, StyleSheet, View } from "react-native";
import { interfaceId } from "personal-rns/contract";

import type { DevelopmentRuntimeView } from "@/native/development-runtime-context";
import { bluetoothStatus } from "./bluetooth-status";
import { ConnectionsScreen } from "./connections-screen";

jest.mock("expo-router", () => ({
  Link: ({ children }: { readonly children: ReactNode }) => children,
}));

jest.mock("@/native/development-runtime-context", () => ({
  useDevelopmentRuntime: () => mockRuntime,
}));

let mockWidth = 390;
let mockFontScale = 1;

jest.mock("react-native/Libraries/Utilities/useWindowDimensions", () => ({
  __esModule: true,
  default: () => ({ width: mockWidth, height: 844, scale: 3, fontScale: mockFontScale }),
}));

type ConnectionsRuntime = Pick<
  DevelopmentRuntimeView,
  | "availability"
  | "phase"
  | "snapshot"
  | "bluetoothAuthorization"
  | "bluetoothAuthorizationFailure"
  | "androidRuntime"
  | "canStartNode"
  | "startNode"
  | "stoppingNode"
  | "refreshSnapshot"
  | "setBluetoothEnabled"
>;
type BluetoothSnapshot = DevelopmentNodeSnapshot["bluetooth"];
type AndroidRuntime = NonNullable<ConnectionsRuntime["androidRuntime"]>;

function snapshot(
  bluetooth: Partial<BluetoothSnapshot> = {},
  runtime = Bindings.DevelopmentNodeRuntime.Running,
): DevelopmentNodeSnapshot {
  return {
    contractFingerprint: "test-contract",
    revision: 1n,
    runtime,
    primaryIdentity: Bindings.PrimaryIdentityState.Missing.new(),
    localHost: Bindings.LocalHostState.Stopped.new({ lastStartFailure: undefined }),
    lxmf: { state: Bindings.LxmfHealthState.Ready, inboundOverflowCount: 0n },
    bluetooth: {
      desiredEnabled: true,
      state: Bindings.LocalBluetoothState.WaitingForPeers.new(),
      peers: [],
      ...bluetooth,
    },
    controllerIdentityFingerprint: undefined,
    pairing: Bindings.RemoteControlPairingState.Searching.new(),
    pairingCandidates: [],
    pairedTargets: [],
    lastAnnouncement: undefined,
    lastRemoteChange: undefined,
    generationId: 1n,
    activeOperation: undefined,
    failure: undefined,
  };
}

function peer(
  byte: number,
  overrides: Partial<BluetoothSnapshot["peers"][number]> = {},
): BluetoothSnapshot["peers"][number] {
  return {
    interfaceId: new Uint8Array(8).fill(byte),
    name: "Nearby phone",
    connected: true,
    rxBytes: 13n,
    txBytes: 21n,
    details: undefined,
    rssiDbm: undefined,
    ...overrides,
  };
}

function androidRuntime(
  status: Partial<NonNullable<AndroidRuntime["status"]>> = {},
): AndroidRuntime {
  return {
    status: {
      revision: 1,
      bluetoothPermission: "granted",
      backgroundDiscovery: "granted",
      connectionNotification: "enabled",
      bluetoothRadio: "on",
      locationServices: "on",
      service: "running",
      lastError: null,
      ...status,
    },
    failure: null,
    refresh: jest.fn(async () => {}),
    requestBluetoothPermissions: jest.fn(async () => {}),
    requestBackgroundBluetoothPermission: jest.fn(async () => {}),
    requestConnectionNotificationPermission: jest.fn(async () => {}),
  };
}

const mockSetBluetooth = jest.fn<
  ReturnType<ConnectionsRuntime["setBluetoothEnabled"]>,
  [boolean]
>();
const mockRefresh = jest.fn<ReturnType<ConnectionsRuntime["refreshSnapshot"]>, []>();
let mockRuntime: { -readonly [Key in keyof ConnectionsRuntime]: ConnectionsRuntime[Key] };

beforeEach(() => {
  jest.clearAllMocks();
  mockWidth = 390;
  mockFontScale = 1;
  mockSetBluetooth.mockResolvedValue({
    type: "outcome",
    outcome: Bindings.LocalBluetoothSettingsOutcome.Ready.new({ enabled: false }),
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
    stoppingNode: false,
    refreshSnapshot: mockRefresh,
    setBluetoothEnabled: mockSetBluetooth,
  };
});

afterEach(() => jest.restoreAllMocks());

describe("Bluetooth status", () => {
  test("reports ready with zero physical peers even when host route and link counts remain", () => {
    mockRuntime.snapshot = {
      ...snapshot(),
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
              interfaceId: interfaceId(new Uint8Array(8).fill(0x22)),
              name: "Bluetooth Auto",
              kind: "AutomaticBluetoothLe",
              health: "Connected",
              rxBytes: 3n,
              txBytes: 4n,
              routeCount: 42,
              linkCount: 9,
              transportedLinkCount: 17,
            },
          ],
          routes: [],
          activeLinkCount: 9,
          destinationIdentities: [],
          runtime: {
            running: true,
            uptimeMillis: 5,
            interfaceCount: 1,
            onlineInterfaceCount: 1,
            routeCount: 42,
            linkCount: 9,
            transportedLinkCount: 17,
            rxBytes: 3n,
            txBytes: 4n,
            rxBps: 0,
            txBps: 0,
          },
          persistence: { persistent: true, restored: true, lastFlushCause: "Startup" },
        },
      }),
    };
    expect(bluetoothStatus(mockRuntime).label).toBe("Ready");
    const view = render(<ConnectionsScreen />);
    expect(view.getByText("Running")).toBeTruthy();
    expect(view.getByText(/^No devices connected\./)).toBeTruthy();
    expect(view.queryByText(/^Connected to/)).toBeNull();
    expect(view.queryByText("Connection ID")).toBeNull();
  });

  test("uses the iOS radio state even when authorization and node operation are healthy", () => {
    mockRuntime.snapshot = snapshot({ state: Bindings.LocalBluetoothState.RadioOff.new() });
    expect(bluetoothStatus(mockRuntime)).toMatchObject({
      label: "Bluetooth is off",
      warning: true,
    });
    const view = render(<ConnectionsScreen />);
    expect(view.getByText("Running")).toBeTruthy();
    expect(view.getByText("Bluetooth is off")).toBeTruthy();
    expect(view.getByText(/Turn on Bluetooth in your phone's settings/)).toBeTruthy();
    expect(view.queryByText("Ready")).toBeNull();
  });

  test("keeps an app-disabled Bluetooth setting distinct from the running node", () => {
    mockRuntime.snapshot = snapshot({
      desiredEnabled: false,
      state: Bindings.LocalBluetoothState.Disabled.new(),
    });
    const view = render(<ConnectionsScreen />);
    expect(view.getByText("Running")).toBeTruthy();
    expect(view.getByText("Off")).toBeTruthy();
    expect(view.getByText(/Your node keeps running\./)).toBeTruthy();
    expect(view.getByRole("button", { name: "Enable Bluetooth connections" })).toBeEnabled();
    expect(view.queryByText("Node stopped")).toBeNull();
  });

  test("keeps a stopped node distinct from healthy Bluetooth permission", () => {
    mockRuntime.snapshot = snapshot(
      { state: Bindings.LocalBluetoothState.Stopped.new() },
      Bindings.DevelopmentNodeRuntime.Stopped,
    );
    mockRuntime.canStartNode = true;
    const view = render(<ConnectionsScreen />);
    expect(view.getByText("Node stopped")).toBeTruthy();
    expect(view.queryByText("Ready")).toBeNull();
    expect(view.queryByText(/Your node keeps running\./)).toBeNull();
    fireEvent.press(view.getByRole("button", { name: "Start node" }));
    expect(mockRuntime.startNode).toHaveBeenCalledTimes(1);
  });

  test("counts and displays only connected physical peers", () => {
    mockRuntime.snapshot = snapshot({
      state: Bindings.LocalBluetoothState.Connected.new(),
      peers: [
        peer(0x11),
        peer(0x22, { name: "Disconnected peer", connected: false }),
        peer(0x33, { name: undefined }),
      ],
    });
    const view = render(<ConnectionsScreen />);
    expect(view.getByText("Connected to 2 devices")).toBeTruthy();
    expect(view.getAllByText("Connection ID")).toHaveLength(2);
    expect(view.getByRole("header", { name: "Nearby device" })).toBeTruthy();
    expect(view.queryByText("Disconnected peer")).toBeNull();
    expect(view.queryByText("2222222222222222")).toBeNull();
    mockRuntime.snapshot = snapshot({
      state: Bindings.LocalBluetoothState.Connected.new(),
      peers: [peer(0x11)],
    });
    view.rerender(<ConnectionsScreen />);
    expect(view.getByText("Connected to 1 device")).toBeTruthy();
  });

  test("does not claim a connection from a connected aggregate state without connected peers", () => {
    mockRuntime.snapshot = snapshot({
      state: Bindings.LocalBluetoothState.Connected.new(),
      peers: [peer(0x11, { connected: false })],
    });
    expect(bluetoothStatus(mockRuntime).label).toBe("Updating");
    const view = render(<ConnectionsScreen />);
    expect(view.queryByText(/^Connected to/)).toBeNull();
    expect(view.queryByText("Connection ID")).toBeNull();
  });

  test.each([
    ["notRequested", "Permission needed", "Allow Bluetooth"],
    ["denied", "Permission needed", "Allow Bluetooth"],
    ["blocked", "Permission needed", "Open app settings"],
    ["granted", "Checking access", null],
  ] as const)(
    "resolves Android %s permission before an unknown radio state",
    (bluetoothPermission, expectedStatus, recoveryAction) => {
      // Android 12+ cannot inspect the radio until Bluetooth permission is granted.
      mockRuntime = {
        ...mockRuntime,
        availability: { type: "available", platform: "android" },
        bluetoothAuthorization: null,
        androidRuntime: androidRuntime({
          bluetoothPermission,
          bluetoothRadio: "unknown",
          locationServices: "notRequired",
        }),
        snapshot: snapshot({
          state: Bindings.LocalBluetoothState.Connected.new(),
          peers: [peer(0x11)],
        }),
      };
      expect(bluetoothStatus(mockRuntime).label).toBe(expectedStatus);
      const view = render(<ConnectionsScreen />);
      expect(view.getByText(expectedStatus)).toBeTruthy();
      expect(view.queryByText("Ready")).toBeNull();
      expect(view.queryByText("Connected to 1 device")).toBeNull();
      expect(view.queryByText("Connection ID")).toBeNull();
      if (recoveryAction !== null) {
        expect(view.getByRole("button", { name: recoveryAction })).toBeEnabled();
        expect(view.queryByText("Checking access")).toBeNull();
      } else {
        expect(view.queryByRole("button", { name: "Allow Bluetooth" })).toBeNull();
        expect(view.queryByRole("button", { name: "Open app settings" })).toBeNull();
      }
    },
  );

  test.each(["ios", "android"])(
    "shows a failed initial %s access read instead of waiting indefinitely",
    (platform) => {
      mockRuntime = {
        ...mockRuntime,
        availability: { type: "available", platform: platform as "ios" | "android" },
        bluetoothAuthorization: null,
        bluetoothAuthorizationFailure: platform === "ios" ? "test access failure" : null,
        androidRuntime:
          platform === "android"
            ? { ...androidRuntime(), status: null, failure: "test access failure" }
            : null,
      };
      expect(bluetoothStatus(mockRuntime)).toMatchObject({
        label: "Status unavailable",
        warning: true,
      });
    },
  );
});

describe("connection controls and recovery", () => {
  test.each([
    "missing snapshot",
    "unread setting",
    "starting node",
    "stopping node",
    "disconnecting",
  ])("disables changes while %s", (condition) => {
    if (condition === "missing snapshot") mockRuntime.snapshot = null;
    if (condition === "unread setting")
      mockRuntime.snapshot = snapshot({ desiredEnabled: undefined });
    if (condition === "starting node") mockRuntime.phase = "starting";
    if (condition === "stopping node") mockRuntime.stoppingNode = true;
    if (condition === "disconnecting")
      mockRuntime.snapshot = snapshot({
        desiredEnabled: false,
        state: Bindings.LocalBluetoothState.Disabling.new(),
      });
    const view = render(<ConnectionsScreen />);
    const toggle = view.getByRole("button", { name: /(?:Enable|Disable) Bluetooth connections/ });
    expect(toggle).toBeDisabled();
    fireEvent.press(toggle);
    expect(mockSetBluetooth).not.toHaveBeenCalled();
  });

  test("waits for the native setting command and a published snapshot without a duplicate change", async () => {
    let complete:
      | ((result: Awaited<ReturnType<ConnectionsRuntime["setBluetoothEnabled"]>>) => void)
      | undefined;
    mockSetBluetooth.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    const view = render(<ConnectionsScreen />);
    fireEvent.press(view.getByRole("button", { name: "Disable Bluetooth connections" }));
    expect(mockSetBluetooth).toHaveBeenCalledWith(false);
    expect(view.getByRole("button", { name: "Please wait…" })).toBeDisabled();
    expect(view.getByRole("button", { name: "Refresh connections" })).toBeDisabled();
    fireEvent.press(view.getByRole("button", { name: "Please wait…" }));
    expect(mockSetBluetooth).toHaveBeenCalledTimes(1);
    await act(async () =>
      complete?.({
        type: "outcome",
        outcome: Bindings.LocalBluetoothSettingsOutcome.Ready.new({ enabled: false }),
      }),
    );
    expect(view.queryByText("Off")).toBeNull();
    mockRuntime.snapshot = snapshot({
      desiredEnabled: false,
      state: Bindings.LocalBluetoothState.Disabled.new(),
    });
    view.rerender(<ConnectionsScreen />);
    expect(view.getByText("Off")).toBeTruthy();
    await act(async () =>
      fireEvent.press(view.getByRole("button", { name: "Enable Bluetooth connections" })),
    );
    expect(mockSetBluetooth).toHaveBeenLastCalledWith(true);
  });

  test.each(["operation", "unavailable", "busy"])(
    "keeps the last snapshot visible and explains a %s failure",
    async (failure) => {
      const detail = "Bluetooth could not be changed. Try again.";
      mockSetBluetooth.mockResolvedValueOnce(
        failure === "operation"
          ? { type: "operationFailure", detail }
          : {
              type: "outcome",
              outcome:
                failure === "busy"
                  ? Bindings.LocalBluetoothSettingsOutcome.Busy.new()
                  : Bindings.LocalBluetoothSettingsOutcome.Unavailable.new({
                      detail: "private native diagnostic",
                    }),
            },
      );
      const view = render(<ConnectionsScreen />);
      await act(async () =>
        fireEvent.press(view.getByRole("button", { name: "Disable Bluetooth connections" })),
      );
      expect(view.getByRole("button", { name: "Disable Bluetooth connections" })).toBeEnabled();
      expect(view.queryByText("Off")).toBeNull();
      expect(view.queryByText("private native diagnostic")).toBeNull();
      expect(
        view.getByText(
          failure === "operation"
            ? detail
            : failure === "busy"
              ? "This phone is changing connections. Wait a moment and try again."
              : "The Bluetooth setting could not be confirmed. Refresh to check it, then try again.",
        ),
      ).toBeTruthy();
    },
  );

  test("refreshes Android access and the snapshot, and exposes refresh failure", async () => {
    const android = androidRuntime();
    mockRuntime = {
      ...mockRuntime,
      availability: { type: "available", platform: "android" },
      bluetoothAuthorization: null,
      androidRuntime: android,
    };
    mockRefresh.mockResolvedValueOnce({
      type: "operationFailure",
      detail: "Connection refresh failed. Try again.",
    });
    const view = render(<ConnectionsScreen />);
    await act(async () =>
      fireEvent.press(view.getByRole("button", { name: "Refresh connections" })),
    );
    expect(android.refresh).toHaveBeenCalledTimes(1);
    expect(mockRefresh).toHaveBeenCalledTimes(1);
    expect(view.getByText("Connection refresh failed. Try again.")).toBeTruthy();
    expect(view.getByRole("button", { name: "Refresh connections" })).toBeEnabled();
  });

  test("offers Android permission recovery without presenting a ready connection", async () => {
    const android = androidRuntime({ bluetoothPermission: "notRequested" });
    mockRuntime = {
      ...mockRuntime,
      availability: { type: "available", platform: "android" },
      bluetoothAuthorization: null,
      androidRuntime: android,
    };
    const view = render(<ConnectionsScreen />);
    expect(view.getByText("Permission needed")).toBeTruthy();
    expect(view.queryByText("Ready")).toBeNull();
    await act(async () => fireEvent.press(view.getByRole("button", { name: "Allow Bluetooth" })));
    expect(android.requestBluetoothPermissions).toHaveBeenCalledTimes(1);
    expect(mockSetBluetooth).not.toHaveBeenCalled();
  });

  test("opens iOS permission settings without exposing stale connected peers", async () => {
    const openSettings = jest.spyOn(Linking, "openSettings").mockResolvedValue();
    mockRuntime.bluetoothAuthorization = {
      authorization: "denied",
      nativeStart: "running",
      restorationAttemptRequested: false,
      revision: 2,
    };
    mockRuntime.snapshot = snapshot({
      state: Bindings.LocalBluetoothState.Connected.new(),
      peers: [peer(0x11)],
    });
    const view = render(<ConnectionsScreen />);
    expect(view.getByText("Permission needed")).toBeTruthy();
    expect(view.queryByText("Connection ID")).toBeNull();
    expect(view.queryByText("Connected to 1 device")).toBeNull();
    await act(async () => fireEvent.press(view.getByRole("button", { name: "Open Settings" })));
    expect(openSettings).toHaveBeenCalledTimes(1);
    expect(mockSetBluetooth).not.toHaveBeenCalled();
  });

  test.each([
    ["radio", "Bluetooth is off", "Open Bluetooth settings", "android.settings.BLUETOOTH_SETTINGS"],
    [
      "location",
      "Location needed",
      "Open Location settings",
      "android.settings.LOCATION_SOURCE_SETTINGS",
    ],
  ])(
    "opens Android %s recovery and refreshes the displayed state",
    async (setting, label, button, intent) => {
      const openSettings = jest.spyOn(Linking, "sendIntent").mockResolvedValue();
      mockRuntime = {
        ...mockRuntime,
        availability: { type: "available", platform: "android" },
        bluetoothAuthorization: null,
        androidRuntime: androidRuntime(
          setting === "radio" ? { bluetoothRadio: "off" } : { locationServices: "off" },
        ),
      };
      const view = render(<ConnectionsScreen />);
      expect(view.getByText(label)).toBeTruthy();
      expect(view.queryByText("Ready")).toBeNull();
      await act(async () => fireEvent.press(view.getByRole("button", { name: button })));
      expect(openSettings).toHaveBeenCalledWith(intent);
      mockRuntime.androidRuntime = androidRuntime();
      view.rerender(<ConnectionsScreen />);
      expect(view.queryByText(label)).toBeNull();
      expect(view.getByText(/^No devices connected\./)).toBeTruthy();
    },
  );

  test.each([1.5, 2])(
    "keeps settings recovery and peer details usable at %sx text",
    async (fontScale) => {
      mockWidth = 320;
      mockFontScale = fontScale;
      const openSettings = jest
        .spyOn(Linking, "openSettings")
        .mockRejectedValue(new Error("test settings failure"));
      mockRuntime = {
        ...mockRuntime,
        availability: { type: "available", platform: "android" },
        bluetoothAuthorization: null,
        androidRuntime: androidRuntime({ bluetoothPermission: "blocked" }),
        snapshot: snapshot({
          state: Bindings.LocalBluetoothState.Connected.new(),
          peers: [
            peer(0x11, {
              name: "Nearby phone with a long descriptive name",
              details:
                "Bluetooth connection details that wrap across several lines on a narrow display.",
              rssiDbm: -67,
              rxBytes: 9007199254740993n,
              txBytes: 18446744073709551615n,
            }),
          ],
        }),
      };
      const view = render(<ConnectionsScreen />);
      await act(async () =>
        fireEvent.press(view.getByRole("button", { name: "Open app settings" })),
      );
      expect(openSettings).toHaveBeenCalledTimes(1);
      expect(
        view.getByText(
          "Settings could not open. Open Android Settings to check Bluetooth and app permissions.",
        ),
      ).toBeTruthy();
      expect(view.queryByText("Connection ID")).toBeNull();
      mockRuntime.androidRuntime = androidRuntime();
      view.rerender(<ConnectionsScreen />);
      expect(view.getByText("Connected to 1 device")).toBeTruthy();
      for (const text of [
        "Nearby phone with a long descriptive name",
        "Bluetooth connection details that wrap across several lines on a narrow display.",
        "9007199254740993 bytes",
        "18446744073709551615 bytes",
        "-67 dBm",
      ]) {
        const element = view.getByText(text);
        expect(element.props.numberOfLines).toBeUndefined();
        expect(element.props.allowFontScaling).not.toBe(false);
      }
      const received = view.getByText("9007199254740993 bytes");
      expect(received.props.selectable).toBe(true);
      expect(StyleSheet.flatten(received.props.style).textAlign).toBe("left");
      expect(
        view
          .UNSAFE_getAllByType(View)
          .filter((node) => StyleSheet.flatten(node.props.style)?.flexBasis === "100%"),
      ).toHaveLength(2);
      for (const button of view.getAllByRole("button")) {
        expect(StyleSheet.flatten(button.props.style).minHeight).toBeGreaterThanOrEqual(48);
      }
    },
  );
});
