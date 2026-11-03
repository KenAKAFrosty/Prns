import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { InstallationScreen } from "../src/screens/InstallationScreen";
jest.mock("expo-router", () => ({ Link: ({ href, children }: { href: unknown; children: React.ReactElement }) => {
  const React = require("react"); return React.cloneElement(children, { onPress: () => mockNavigate(href) });
} }));
jest.mock("../src/runtime/InstallationProvider", () => ({ useInstallation: () => mockInstallation }));
jest.mock("../src/navigation", () => ({ useRouteActive: () => true }));
jest.mock("../src/identity-reference", () => ({ identityKey: (record: { key: string }) => record.key }));
jest.mock("@prns/app-bindings", () => ({
  prns_identities_core: { IdentityDesiredState: { Enabled: 1, Paused: 0 }, IdentityRuntimeStatus: { Running: 1, Starting: 0, Unavailable: 3 } },
  prns_service_runtime: { ServiceRuntimeNetworkPhase: { Ready: 2, Offline: 0 } },
}));
const mockNavigate = jest.fn();
const mockRecord = { key: "selected-owner", id: { sequence: 1n }, publicHash: new ArrayBuffer(0), services: [] as number[], desired: 0,
  metadata: { localLabel: "Personal", publishedName: "Alex" } };
const mockSnapshot = jest.fn();
const mockClient = { snapshot: mockSnapshot };
const mockIdentity = jest.fn(() => mockClient);
const mockOwner = { identities: () => ({ identity: mockIdentity }) };
let mockInstallation: Record<string, unknown>;
beforeEach(() => {
  jest.clearAllMocks(); mockRecord.services = []; mockRecord.desired = 0;
  mockInstallation = { owner: mockOwner, mode: "open", records: [mockRecord], selected: "selected-owner",
    status: { cleanupComplete: true }, network: { phase: 0 }, stop: jest.fn(), start: jest.fn() };
  mockSnapshot.mockImplementation(async () => ({ identity: mockRecord, runtime: 3 }));
});

test("local readiness guides setup before activation and does not imply network/message readiness", async () => {
  await render(<InstallationScreen />);
  expect(screen.getByText("Saved data is open on this phone.")).toBeTruthy();
  expect(screen.getByText("2. Set up group messaging for this identity.")).toBeTruthy();
  await fireEvent.press(screen.getByText("Set up messaging"));
  expect(mockNavigate).toHaveBeenCalledWith({ pathname: "/more/identities/[reference]", params: { reference: "selected-owner" } });
  expect(screen.queryByText("Open conversations")).toBeNull();
  expect(screen.getByText(/Networking is stopped or unavailable/)).toBeTruthy();
});

test("configured but paused identity points to activation; only actual Running unlocks next chat steps", async () => {
  mockRecord.services = [0x67727073];
  const view = await render(<InstallationScreen />);
  expect(screen.getByText("Start messaging")).toBeTruthy();
  await view.unmount();
  mockRecord.desired = 1;
  mockSnapshot.mockImplementation(async () => ({ identity: mockRecord, runtime: 0 }));
  const starting = await render(<InstallationScreen />);
  expect(screen.getByText("Check messaging status")).toBeTruthy(); expect(screen.queryByText("Open conversations")).toBeNull();
  await starting.unmount();
  mockSnapshot.mockImplementation(async () => ({ identity: mockRecord, runtime: 1 }));
  await render(<InstallationScreen />);
  expect(screen.getByText("Open conversations")).toBeTruthy();
  expect(screen.getByText("Messages can be sent when networking is available again.")).toBeTruthy();
  await fireEvent.press(screen.getByText("Open contacts"));
  expect(mockNavigate).toHaveBeenCalledWith({ pathname: "/contacts", params: { owner: "selected-owner" } });
});

test("no selected identity offers creation, and late snapshot from another identity cannot enable chat", async () => {
  let resolve!: (value: unknown) => void;
  mockSnapshot.mockImplementation(() => new Promise((done) => { resolve = done; }));
  const view = await render(<InstallationScreen />);
  mockInstallation = { ...mockInstallation, records: [], selected: undefined };
  await view.rerender(<InstallationScreen />);
  await act(async () => resolve({ identity: { ...mockRecord, services: [0x67727073], desired: 1 }, runtime: 1 }));
  expect(screen.getByText("Choose a messaging identity")).toBeTruthy();
  expect(screen.queryByText("Open conversations")).toBeNull();
});
