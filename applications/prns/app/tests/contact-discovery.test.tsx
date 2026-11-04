import { Platform } from "react-native";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { contactKey, loadContacts, saveContact } from "../src/contacts";
import type { SavedContact } from "../src/contacts";
import { prns_contact_card as cards } from "@prns/app-bindings";
import { discoverContact, discoveryLifetimeMillis, retainDiscoveries } from "../src/runtime/contact-discovery";
import type { DiscoveredContact } from "../src/runtime/contact-discovery";
import type { HeardAnnouncement } from "../src/runtime/network-activity";
import { ContactDiscoveryScreen } from "../src/screens/ContactDiscoveryScreen";
import { identityKey } from "../src/identity-reference";

jest.mock("@react-native-async-storage/async-storage", () => require("@react-native-async-storage/async-storage/jest/async-storage-mock"));
jest.mock("../src/contacts", () => ({ ...jest.requireActual("../src/contacts"), loadContacts: jest.fn(), saveContact: jest.fn() }));
jest.mock("@prns/app-bindings", () => ({ prns_contact_card: {
  verifyContactCard: jest.fn(), encodeContactCardUri: jest.fn(), serviceNameHash: jest.fn(),
}, prns_identities_runtime: { IdentityManualAnnounce_Tags: { Queued: "Queued", Throttled: "Throttled", Disabled: "Disabled" } } }));
jest.mock("expo-router", () => ({ useLocalSearchParams: () => ({ owner: mockOwnerKey }), useRouter: () => ({ push: mockPush }) }));
jest.mock("../src/navigation", () => ({ scalar: (value: unknown) => typeof value === "string" ? value : undefined,
  useRouteLease: () => ({ active: mockActive, isCurrent: () => mockActive }) }));
jest.mock("../src/runtime/InstallationProvider", () => ({ useInstallation: () => mockInstallation }));
const bytes = (n: number) => new Uint8Array([n]).buffer;
const record = { id: { incarnation: bytes(1), sequence: 1n }, publicHash: bytes(1) };
const mockSettings = jest.fn(), mockAnnounce = jest.fn(), mockSetSettings = jest.fn();
const mockClient = { settings: mockSettings, announce: mockAnnounce, setSettings: mockSetSettings };
const mockOwner = { announcements: jest.fn(() => mockClient) };
const mockClearDiscoveries = jest.fn();
const mockPush = jest.fn(); let mockActive = true; let mockOwnerKey: string | undefined = identityKey(record);
let mockInstallation: any;
const claims = (n: number) => ({ publicIdentity: bytes(n), identityHash: bytes(n), destinationHash: bytes(n + 1), serviceNameHash: bytes(7), displayName: `Person ${n}` });
const observation = (n: number, now = 100): HeardAnnouncement => ({ id: BigInt(n), observedAt: now,
  event: { tag: "AnnounceHeard", inner: { destination: new Uint8Array([n + 1]), appData: new Uint8Array(148).fill(n), hops: 2, sourceInterface: new Uint8Array([1]) } } } as unknown as HeardAnnouncement);
const item = (n: number, now = 100): DiscoveredContact => ({ contact: { claims: claims(n), alias: "", uri: `card:${n}` }, observation: observation(n, now) });
beforeEach(() => {
  jest.resetAllMocks(); mockActive = true; mockOwnerKey = identityKey(record);
  jest.mocked(cards.serviceNameHash).mockReturnValue(bytes(7));
  jest.mocked(cards.verifyContactCard).mockImplementation((wire) => claims(new Uint8Array(wire)[0]!));
  jest.mocked(cards.encodeContactCardUri).mockImplementation((wire) => `card:${new Uint8Array(wire)[0]}`);
  jest.mocked(loadContacts).mockResolvedValue([]);
  mockSettings.mockResolvedValue({ intervalMillis: undefined }); mockAnnounce.mockResolvedValue({ tag: "Queued" });
  mockOwner.announcements.mockReturnValue(mockClient);
  mockInstallation = { owner: mockOwner, records: [record], contactsRevision: 0, clearDiscoveredPeople: mockClearDiscoveries, networkKey: "4", networkActivity: { generation: "4", discovered: [item(1), item(3)], dropped: 0n } };
});

test("discovery requires signed enrollment claim, matching destination and the native accepted full public key", async () => {
  const accepted = jest.fn().mockResolvedValue(new Uint8Array([3]));
  expect((await discoverContact(observation(3), accepted))?.contact.uri).toBe("card:3");
  expect(accepted).toHaveBeenCalledWith(new Uint8Array([4]));
  accepted.mockResolvedValue(undefined); expect(await discoverContact(observation(3), accepted)).toBeUndefined();
  accepted.mockResolvedValue(new Uint8Array([9])); expect(await discoverContact(observation(3), accepted)).toBeUndefined();
  for (const value of [{ ...claims(3), serviceNameHash: bytes(9) }, { ...claims(3), destinationHash: bytes(9) }]) {
    accepted.mockClear(); jest.mocked(cards.verifyContactCard).mockReturnValue(value);
    expect(await discoverContact(observation(3), accepted)).toBeUndefined(); expect(accepted).not.toHaveBeenCalled();
  }
  jest.mocked(cards.verifyContactCard).mockImplementation(() => { throw new Error("bad signature"); });
  expect(await discoverContact(observation(3), accepted)).toBeUndefined();
});

test("unrelated payloads are rejected cheaply and retained discoveries replace by identity within the day and 200-row bound", async () => {
  const prior = observation(3);
  const raw = { ...prior, event: { ...prior.event, inner: { ...prior.event.inner, appData: new Uint8Array(277) } } };
  expect(await discoverContact(raw, jest.fn())).toBeUndefined(); expect(cards.verifyContactCard).not.toHaveBeenCalled();
  const before = Array.from({ length: 200 }, (_, n) => item(n, 100));
  const next = retainDiscoveries(before, [item(3, 200), item(201, 200)], 200);
  expect(next).toHaveLength(200); expect(next[0]?.observation.observedAt).toBe(200);
  expect(next.filter((entry) => entry.contact.uri === "card:3")).toHaveLength(1);
  expect(retainDiscoveries(next, [], discoveryLifetimeMillis + 200)).toEqual([]);
});

test("retained discoveries exclude local identities and open exact route owner review without saving", async () => {
  const view = await render(<ContactDiscoveryScreen />);
  expect(screen.queryByText("Person 1")).toBeNull(); expect(screen.getByText("Person 3")).toBeTruthy();
  await act(async () => await fireEvent.press(screen.getByRole("button", { name: "Review contact: Person 3" })));
  expect(mockPush).toHaveBeenCalledWith({ pathname: "/contacts/add", params: { owner: identityKey(record), card: "card:3" } });
  mockActive = false;
  await fireEvent.press(screen.getByRole("button", { name: "Review contact: Person 3" })); expect(mockPush).toHaveBeenCalledTimes(1);
  mockInstallation.networkKey = "5"; await view.rerender(<ContactDiscoveryScreen />);
  expect(screen.queryByText("Person 3")).toBeNull();
  mockOwnerKey = undefined; await view.rerender(<ContactDiscoveryScreen />);
  expect(screen.getByText(/Choose an available messaging identity/)).toBeTruthy();
});


function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }

test("Find people exposes Announce yourself directly and keeps schedule changes separate", async () => {
  await render(<ContactDiscoveryScreen />);
  expect(screen.getByRole("button", { name: "Announce yourself" })).toBeEnabled();
  expect(screen.queryByText("Manual only")).toBeNull();
  expect(mockAnnounce).not.toHaveBeenCalled();
  await act(async () => { await fireEvent.press(screen.getByRole("button", { name: "Announce yourself" })); });
  expect(mockAnnounce).toHaveBeenCalledTimes(1);
  expect(mockSetSettings).not.toHaveBeenCalled();
  expect(mockOwner.announcements).toHaveBeenCalledWith({ id: record.id, publicHash: record.publicHash });
  expect(screen.getByText("Announcement queued.")).toBeTruthy();
});

test("saved discoveries open the exact contact using its private alias without resaving the announced card", async () => {
  const saved = { ...item(3).contact, uri: "older-verified-card", alias: "Private Alex", pinned: true };
  jest.mocked(loadContacts).mockResolvedValue([saved]);
  await render(<ContactDiscoveryScreen />);
  expect(screen.getByText("Saved")).toBeTruthy();
  expect(screen.getByText("Private Alex")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Review contact: Person 3" })).toBeNull();
  await act(async () => { await fireEvent.press(screen.getByRole("button", { name: "Open contact: Private Alex" })); });
  expect(mockPush).toHaveBeenCalledWith({ pathname: "/contacts/[contact]", params: { owner: identityKey(record), contact: contactKey(saved) } });
  expect(saveContact).not.toHaveBeenCalled();
  expect(saved).toMatchObject({ uri: "older-verified-card", alias: "Private Alex", pinned: true });
});

test("new observations appear automatically and a contact revision changes Review into Open without polling", async () => {
  const buttons = jest.spyOn(require("../src/ui/primitives") as typeof import("../src/ui/primitives"), "Button");
  const view = await render(<ContactDiscoveryScreen />);
  const review = (buttons.mock.calls.findLast(([props]) => props.title === "Review contact: Person 3")![0].onPress as () => void);
  mockInstallation = { ...mockInstallation, networkActivity: { ...mockInstallation.networkActivity, discovered: [item(3), item(4)] } };
  await view.rerender(<ContactDiscoveryScreen />);
  expect(screen.getByRole("button", { name: "Review contact: Person 4" })).toBeEnabled();
  expect(loadContacts).toHaveBeenCalledTimes(1);
  const updated = deferred<readonly SavedContact[]>();
  jest.mocked(loadContacts).mockReturnValueOnce(updated.promise);
  mockInstallation = { ...mockInstallation, contactsRevision: 1 };
  await view.rerender(<ContactDiscoveryScreen />);
  expect(screen.getByRole("button", { name: "Review contact: Person 3" })).toBeDisabled();
  await act(async () => { review(); });
  expect(mockPush).not.toHaveBeenCalled();
  await act(async () => { updated.resolve([{ ...item(3).contact, alias: "Private Alex" }]); });
  expect(screen.getByRole("button", { name: "Open contact: Private Alex" })).toBeEnabled();
  expect(screen.queryByRole("button", { name: "Review contact: Person 3" })).toBeNull();
  expect(screen.queryByText("Refresh contacts")).toBeNull();
  expect(loadContacts).toHaveBeenCalledTimes(2);
  expect(saveContact).not.toHaveBeenCalled();
});

test("a failed saved-contact read fences review until explicit read recovery", async () => {
  jest.mocked(loadContacts).mockRejectedValueOnce(new Error("Saved contacts unavailable"));
  await render(<ContactDiscoveryScreen />);
  expect(screen.getByRole("button", { name: "Review contact: Person 3" })).toBeDisabled();
  expect(mockPush).not.toHaveBeenCalled();
  await act(async () => { await fireEvent.press(screen.getByText("Retry loading saved contacts")); });
  expect(screen.getByRole("button", { name: "Review contact: Person 3" })).toBeEnabled();
  expect(screen.queryByText("Retry loading saved contacts")).toBeNull();
  expect(saveContact).not.toHaveBeenCalled();
});

test("a late saved-contact read from another owner cannot label or redirect the current discovery", async () => {
  const pending = deferred<readonly SavedContact[]>();
  jest.mocked(loadContacts).mockReturnValueOnce(pending.promise);
  const view = await render(<ContactDiscoveryScreen />);
  const second = { id: { incarnation: bytes(2), sequence: 2n }, publicHash: bytes(2) };
  mockOwnerKey = identityKey(second);
  mockInstallation = { ...mockInstallation, owner: { ...mockOwner }, records: [record, second] };
  await view.rerender(<ContactDiscoveryScreen />);
  await act(async () => { pending.resolve([{ ...item(3).contact, alias: "Other identity private name" }]); });
  expect(screen.queryByText("Other identity private name")).toBeNull();
  await act(async () => { await fireEvent.press(screen.getByRole("button", { name: "Review contact: Person 3" })); });
  expect(mockPush).toHaveBeenCalledWith({ pathname: "/contacts/add", params: { owner: identityKey(second), card: "card:3" } });
  expect(saveContact).not.toHaveBeenCalled();
});


async function chooseClear() {
  await act(async () => { await fireEvent.press(screen.getByRole("button", { name: "Discovery options" })); });
  const dismiss = screen.container.queryAll((value) => typeof value.props.onDismiss === "function")[0]!.props.onDismiss;
  await act(async () => { await fireEvent.press(screen.getByRole("button", { name: "Clear recent discoveries" })); });
  if (Platform.OS === "ios") await act(async () => { dismiss(); });
}

test("clearing recent discoveries leaves saved contacts unchanged and later announcements can reappear as saved", async () => {
  const saved = { ...item(3).contact, alias: "Private Alex", pinned: true };
  jest.mocked(loadContacts).mockResolvedValue([saved]);
  const view = await render(<ContactDiscoveryScreen />);
  expect(screen.queryByRole("button", { name: "Clear recent discoveries" })).toBeNull();
  await chooseClear();
  expect(mockClearDiscoveries).toHaveBeenCalledTimes(1);
  expect(saveContact).not.toHaveBeenCalled();
  mockInstallation = { ...mockInstallation, networkActivity: { ...mockInstallation.networkActivity, discovered: [] } };
  await view.rerender(<ContactDiscoveryScreen />);
  expect(screen.queryByText("Private Alex")).toBeNull();
  mockInstallation = { ...mockInstallation, networkActivity: { ...mockInstallation.networkActivity, discovered: [item(3, 200)] } };
  await view.rerender(<ContactDiscoveryScreen />);
  expect(screen.getByRole("button", { name: "Open contact: Private Alex" })).toBeEnabled();
  expect(loadContacts).toHaveBeenCalledTimes(1);
  expect(saveContact).not.toHaveBeenCalled();
  expect(saved.pinned).toBe(true);
});

test("an iOS clear action waiting for menu dismissal cannot clear a replacement network generation", async () => {
  jest.replaceProperty(Platform, "OS", "ios");
  try {
    const view = await render(<ContactDiscoveryScreen />);
    await act(async () => { await fireEvent.press(screen.getByRole("button", { name: "Discovery options" })); });
    const dismiss = screen.container.queryAll((value) => typeof value.props.onDismiss === "function")[0]!.props.onDismiss;
    await act(async () => { await fireEvent.press(screen.getByRole("button", { name: "Clear recent discoveries" })); });
    expect(mockClearDiscoveries).not.toHaveBeenCalled();
    mockInstallation = { ...mockInstallation, networkKey: "5", networkActivity: { ...mockInstallation.networkActivity, generation: "5" } };
    await view.rerender(<ContactDiscoveryScreen />);
    await act(async () => { dismiss(); });
    expect(mockClearDiscoveries).not.toHaveBeenCalled();
    await chooseClear();
    expect(mockClearDiscoveries).toHaveBeenCalledTimes(1);
  } finally { jest.restoreAllMocks(); }
});

test("discovery options cannot offer an unavailable clear capability", async () => {
  mockInstallation = { ...mockInstallation, clearDiscoveredPeople: undefined };
  await render(<ContactDiscoveryScreen />);
  expect(screen.getByRole("button", { name: "Discovery options" })).toBeDisabled();
  expect(mockClearDiscoveries).not.toHaveBeenCalled();
});

afterEach(() => { jest.restoreAllMocks(); });
