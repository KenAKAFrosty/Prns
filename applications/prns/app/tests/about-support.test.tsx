import { Share } from "react-native";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import { AboutScreen } from "../src/screens/AboutScreen";
jest.mock("../src/navigation", () => ({ useRouteActive: () => mockActive }));
jest.mock("expo-constants", () => ({ __esModule: true, default: { nativeAppVersion: "0.1", nativeBuildVersion: "1", expoConfig: { extra: { prnsBuild: { label: "parity-test", variant: "dev" } } } } }));
jest.mock("@prns/app-platform", () => ({ status: () => mockPlatform() }));
jest.mock("../src/runtime/InstallationProvider", () => ({ useInstallation: () => mockInstallation }));
jest.mock("@prns/app-bindings", () => ({
  prns_chat_app: { appApiVersion: () => 3 }, prns_host_uniffi: { HostPhysicalRole: { Member: 2 } },
  prns_service_runtime: { ServiceRuntimePhase: { 1: "Ready" }, ServiceRuntimeNetworkPhase: { Ready: 2, 2: "Ready", 0: "Offline" } },
}));
let mockActive = true;
const mockPlatform = jest.fn();
const mockInspection = jest.fn();
const mockOwner = { host: () => ({ inspection: mockInspection }) };
let mockInstallation: { owner: typeof mockOwner; status: { phase: number }; network: { phase: number }; networkKey: string | undefined };
let share: jest.SpyInstance;
const inspected = (routes: number) => ({ interfaces: [], host: { runtime: { onlineInterfaceCount: 1 }, routes: Array.from({ length: routes }, () => ({ destination: "PRIVATE-ADDRESS" })), activeLinkCount: 0 } });
beforeEach(() => {
  jest.useFakeTimers();
  mockActive = true;
  jest.clearAllMocks();
  mockPlatform.mockResolvedValue({ bluetooth: { selected: true, attached: false, authorization: "allowed", availability: "off" }, notifications: "enabled" });
  mockInspection.mockResolvedValue(inspected(2));
  mockInstallation = { owner: mockOwner, status: { phase: 1 }, network: { phase: 2 }, networkKey: "1" };
  share = jest.spyOn(Share, "share").mockResolvedValue({ action: Share.sharedAction, activityType: undefined });
});
afterEach(async () => { await cleanup(); share.mockRestore(); jest.useRealTimers(); });
async function press(label: string) { await act(async () => { await fireEvent.press(screen.getByRole("button", { name: label })); }); }
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (cause: Error) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }

test("Share uses the displayed aggregate report and preserves Bluetooth status without addresses", async () => {
  await render(<AboutScreen />);
  expect(screen.queryByText(/^PRNS Chat diagnostics/)).toBeNull();
  await press("Diagnostics");
  const report = screen.getByText(/^PRNS Chat diagnostics/).props.children;
  expect(report).toContain("Known routes: 2"); expect(report).toContain("Bluetooth selected: yes");
  expect(report).not.toContain("PRIVATE-ADDRESS");
  await press("Diagnostics");
  await press("Share diagnostics");
  expect(share).toHaveBeenCalledWith({ title: "PRNS Chat diagnostics", message: report });
});

test("a replacement host blocks sharing stale counts until its own observation finishes", async () => {
  const view = await render(<AboutScreen />);
  await press("Diagnostics");
  let resolve!: (value: unknown) => void;
  mockInspection.mockImplementation(() => new Promise((done) => { resolve = done; }));
  mockInstallation = { ...mockInstallation, networkKey: "2" };
  await view.rerender(<AboutScreen />);
  expect(screen.getByRole("button", { name: "Share diagnostics" }).props.accessibilityState.disabled).toBe(true);
  expect(screen.getByText(/^PRNS Chat diagnostics/).props.children).toContain("Known routes: unknown");
  await act(async () => resolve(inspected(4)));
  await press("Diagnostics");
  await press("Share diagnostics");
  expect(share.mock.calls[0]![0].message).toContain("Known routes: 4");
});


test("automatic health refresh retains the displayed report and sharing until a new aggregate is ready", async () => {
  await render(<AboutScreen />);
  await press("Diagnostics");
  const displayed = screen.getByText(/^PRNS Chat diagnostics/);
  const oldReport = displayed.props.children;
  const next = deferred<ReturnType<typeof inspected>>();
  mockInspection.mockReturnValueOnce(next.promise);
  await act(async () => { jest.advanceTimersByTime(5000); });
  expect(mockInspection).toHaveBeenCalledTimes(2);
  expect(screen.getByText(/^PRNS Chat diagnostics/)).toBe(displayed);
  expect(displayed.props.children).toBe(oldReport);
  expect(screen.queryByText("Checking connection health…")).toBeNull();
  expect(screen.getByRole("button", { name: "Share diagnostics" })).toBeEnabled();
  await act(async () => { next.resolve(inspected(4)); });
  const freshReport = screen.getByText(/^PRNS Chat diagnostics/).props.children;
  expect(freshReport).toContain("Known routes: 4");
  expect(freshReport).toContain("Bluetooth selected: yes");
  expect(freshReport).not.toContain("PRIVATE-ADDRESS");
  await press("Diagnostics");
  await press("Share diagnostics");
  expect(share).toHaveBeenCalledWith({ title: "PRNS Chat diagnostics", message: freshReport });
});

test.each(["foreground", "owner"])("%s replacement permits a fresh Share and ignores the retired OS share completion", async (replacement) => {
  const view = await render(<AboutScreen />);
  const old = deferred<Awaited<ReturnType<typeof Share.share>>>();
  const fresh = deferred<Awaited<ReturnType<typeof Share.share>>>();
  share.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
  await press("Share diagnostics");
  expect(screen.getByRole("button", { name: "Opening share…" })).toBeDisabled();
  if (replacement === "foreground") {
    mockActive = false;
    await view.rerender(<AboutScreen />);
    mockActive = true;
  } else {
    mockInstallation = { ...mockInstallation, owner: { ...mockOwner } };
  }
  await view.rerender(<AboutScreen />);
  expect(screen.getByRole("button", { name: "Share diagnostics" })).toBeEnabled();
  await press("Share diagnostics");
  expect(share).toHaveBeenCalledTimes(2);
  await act(async () => { old.reject(new Error("Retired share failure")); });
  expect(screen.queryByText(/Retired share failure/)).toBeNull();
  expect(screen.getByRole("button", { name: "Opening share…" })).toBeDisabled();
  await act(async () => { fresh.resolve({ action: Share.sharedAction, activityType: undefined }); });
  expect(screen.getByRole("button", { name: "Share diagnostics" })).toBeEnabled();
});
