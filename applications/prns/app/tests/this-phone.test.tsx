import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { ThisPhoneScreen } from "../src/screens/ThisPhoneScreen";

jest.mock("expo-router", () => ({ Link: ({ href, children }: { href: string; children: React.ReactElement }) => {
  const React = require("react"); return React.cloneElement(children, { onPress: () => mockNavigate(href) });
} }));
jest.mock("../src/navigation", () => ({ useRouteActive: () => mockActive }));
jest.mock("../src/runtime/InstallationProvider", () => ({ useInstallation: () => mockInstallation }));
jest.mock("@prns/app-platform", () => ({ status: () => mockPlatform() }));
jest.mock("@prns/app-bindings", () => ({
  prns_service_runtime: { ServiceRuntimePhase: { 1: "Ready" }, ServiceRuntimeNetworkPhase: { Ready: 2, 2: "Ready", 0: "Offline" } },
  prns_host_uniffi: { RemoteControlControllerIdentitySettlement_Tags: { Completed: "Completed" },
    remoteControlControllerIdentityHash: () => ({ value: new Uint8Array([0x22]) }),
    InterfaceKind: { 0: "TcpClient" }, InterfaceHealth: { 0: "Online" }, Capability: { 0: "Inspection" } },
}));
const mockNavigate = jest.fn();
const mockPlatform = jest.fn();
const mockInspection = jest.fn();
const mockController = jest.fn();
const mockHost = jest.fn();
const mockOwner = { host: mockHost };
let mockInstallation: { owner: typeof mockOwner; status: { phase: number }; network: { phase: number }; networkKey: string };
let mockActive = true;
function inspected(connections = 1) { return { host: {
  runtime: { running: true, onlineInterfaceCount: connections, rxBytes: 2048n, txBytes: 100n },
  activeLinkCount: 3, routes: [{}], backend: { interfaceKinds: [0], capabilities: [0] },
  interfaces: [{ interfaceId: new Uint8Array([1]), name: "Test TCP", health: 0, kind: 0 }],
} }; }
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }
beforeEach(() => {
  jest.useFakeTimers(); jest.clearAllMocks(); mockActive = true;
  mockPlatform.mockResolvedValue({ bluetooth: { selected: true, attached: false, authorization: "allowed", availability: "ready" } });
  mockInspection.mockResolvedValue(inspected());
  mockController.mockResolvedValue({ tag: "Completed", inner: { value: {} } });
  mockHost.mockReturnValue({ inspection: mockInspection, identityHash: () => new Uint8Array([0x11]), remoteControlControllerIdentity: mockController });
  mockInstallation = { owner: mockOwner, status: { phase: 1 }, network: { phase: 2 }, networkKey: "1" };
});
afterEach(() => jest.useRealTimers());

test("groups observed phone facts, folds public IDs and links to the canonical connection screens", async () => {
  await render(<ThisPhoneScreen />);
  expect(screen.getByText("Saved data: Ready")).toBeTruthy();
  expect(screen.getByText("Network host: Running")).toBeTruthy();
  expect(screen.getByText("Bluetooth: Selected, not attached")).toBeTruthy();
  expect(screen.getByText("1 connections online · 3 active links")).toBeTruthy();
  expect(screen.getByText("Test TCP · Online")).toBeTruthy();
  expect(screen.queryByText("Primary network identity")).toBeNull();
  await fireEvent.press(screen.getByRole("button", { name: "Show advanced details" }));
  expect(screen.getByText("11")).toBeTruthy(); expect(screen.getByText("22")).toBeTruthy();
  expect(screen.getByText("Received 2 KiB · sent 100 B")).toBeTruthy();
  expect(screen.getByText("Supported connections: TcpClient")).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "Connections" }));
  expect(mockNavigate).toHaveBeenLastCalledWith("/more/interfaces");
  await fireEvent.press(screen.getByRole("button", { name: "Network activity" }));
  expect(mockNavigate).toHaveBeenLastCalledWith("/more/network");
  expect(screen.queryByText(/Refresh/)).toBeNull();
});

test("offline local readiness does not borrow a host or fabricate network facts", async () => {
  mockInstallation.network.phase = 0;
  await render(<ThisPhoneScreen />);
  expect(mockHost).not.toHaveBeenCalled();
  expect(screen.getByText("Saved data: Ready")).toBeTruthy();
  expect(screen.getByText("Networking: Offline")).toBeTruthy();
  expect(screen.queryByText(/connections online/)).toBeNull();
  await fireEvent.press(screen.getByRole("button", { name: "Show advanced details" }));
  expect(screen.getAllByText("Unavailable")).toHaveLength(2);
});

test("a replaced network generation immediately hides old facts and rejects late replies", async () => {
  const view = await render(<ThisPhoneScreen />);
  const old = deferred<ReturnType<typeof inspected>>();
  mockInspection.mockReturnValueOnce(old.promise);
  await act(async () => { jest.advanceTimersByTime(2000); });
  const next = deferred<ReturnType<typeof inspected>>();
  mockInspection.mockReturnValueOnce(next.promise);
  mockInstallation = { ...mockInstallation, networkKey: "2" };
  await view.rerender(<ThisPhoneScreen />);
  expect(screen.queryByText(/connections online/)).toBeNull();
  await act(async () => old.resolve(inspected(99)));
  expect(screen.queryByText(/99 connections/)).toBeNull();
  await act(async () => next.resolve(inspected(4)));
  expect(screen.getByText("4 connections online · 3 active links")).toBeTruthy();
});

test("background reads retain known values without loading churn and failures remain visible", async () => {
  const view = await render(<ThisPhoneScreen />);
  const pending = deferred<ReturnType<typeof inspected>>();
  mockInspection.mockReturnValueOnce(pending.promise);
  await act(async () => { jest.advanceTimersByTime(2000); });
  expect(screen.getByText("1 connections online · 3 active links")).toBeTruthy();
  expect(screen.queryByText("Checking this phone…")).toBeNull();
  await act(async () => pending.resolve(inspected(2)));
  mockInspection.mockRejectedValueOnce(new Error("temporary"));
  await act(async () => { jest.advanceTimersByTime(2000); });
  expect(screen.getByText("2 connections online · 3 active links")).toBeTruthy();
  expect(screen.getByRole("alert")).toBeTruthy();
  await act(async () => { jest.advanceTimersByTime(2000); });
  expect(screen.queryByRole("alert")).toBeNull();
  await view.unmount();
});

test("inactive routes reject pending observations and stop polling", async () => {
  const pending = deferred<ReturnType<typeof inspected>>();
  mockInspection.mockReturnValue(pending.promise);
  const view = await render(<ThisPhoneScreen />);
  mockActive = false;
  await view.rerender(<ThisPhoneScreen />);
  await act(async () => { pending.resolve(inspected(9)); jest.advanceTimersByTime(10_000); });
  expect(screen.queryByText(/9 connections/)).toBeNull();
  expect(mockInspection).toHaveBeenCalledTimes(1);
});

test("a controller read failure does not hide available host observations or invent its ID", async () => {
  mockController.mockResolvedValue({ tag: "Failed", inner: { failure: {} } });
  await render(<ThisPhoneScreen />);
  expect(screen.getByText("1 connections online · 3 active links")).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "Show advanced details" }));
  expect(screen.getByText("11")).toBeTruthy();
  expect(screen.getByText("Unavailable")).toBeTruthy();
  expect(screen.queryByText("22")).toBeNull();
});

test("owner replacement fences old data even when both owners use the same network generation", async () => {
  const view = await render(<ThisPhoneScreen />);
  const pending = deferred<ReturnType<typeof inspected>>();
  mockInspection.mockReturnValueOnce(pending.promise);
  mockInstallation = { ...mockInstallation, owner: { host: mockHost } };
  await view.rerender(<ThisPhoneScreen />);
  expect(screen.queryByText(/connections online/)).toBeNull();
  await act(async () => pending.resolve(inspected(5)));
  expect(screen.getByText("5 connections online · 3 active links")).toBeTruthy();
  mockHost.mockImplementationOnce(() => { throw new Error("host unavailable"); });
  await act(async () => { jest.advanceTimersByTime(2000); });
  expect(screen.getByText("5 connections online · 3 active links")).toBeTruthy();
  expect(screen.getByRole("alert")).toBeTruthy();
});
