import { AppState, Share, View } from "react-native";
import type { AppStateStatus } from "react-native";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { prns_contact_card as cards } from "@prns/app-bindings";
import type { prns_identities as identities } from "@prns/app-bindings";
import { ContactExchange } from "../src/ContactExchange";
import { ContactScanner } from "../src/ContactScanner";
import { ContactQr } from "../src/ContactQr";
import { contactInputBudget, reviewContactInput } from "../src/contact-input";

jest.mock("@react-native-async-storage/async-storage", () => ({ getItem: jest.fn(), setItem: jest.fn() }));
jest.mock("../src/ContactScanner", () => ({ ContactScanner: jest.fn() }));
jest.mock("../src/ContactQr", () => ({ ContactQr: jest.fn() }));
jest.mock("@prns/app-bindings", () => ({ prns_contact_card: {
  decodeContactCardUri: jest.fn(), encodeContactCardUri: jest.fn(), verifyContactCard: jest.fn(), serviceNameHash: jest.fn(),
} }));
const bytes = (n: number) => new Uint8Array([n]).buffer;
const reference = { id: { incarnation: bytes(1), sequence: 1n }, publicHash: bytes(2) };
const contactCard = jest.fn<Promise<ArrayBuffer>, [ArrayBuffer, string]>();
const identity = { reference: () => reference, contactCard } as unknown as identities.IdentityClientLike;
const store = new Map<string, string>();
const storage = jest.mocked(AsyncStorage);
let appState: (state: AppStateStatus) => void;
const onChange = jest.fn();
const props = { identity, displayName: "Alice", mode: "add" as const, onChange, active: true };
const addressBookWrites = () => storage.setItem.mock.calls.filter(([key]) => key.startsWith("prns.app.v1.contacts."));
const scan = () => jest.mocked(ContactScanner).mock.calls.at(-1)![0];
const scanner = () => screen.queryByLabelText("Scanner fixture");
let stateSubscription: jest.SpyInstance;
let share: jest.SpyInstance;

beforeEach(() => {
  jest.resetAllMocks(); store.clear();
  Object.defineProperty(AppState, "currentState", { value: "active", configurable: true });
  stateSubscription = jest.spyOn(AppState, "addEventListener").mockImplementation((_event, handler) => {
    appState = handler; return { remove: jest.fn() };
  });
  share = jest.spyOn(Share, "share").mockResolvedValue({ action: Share.sharedAction, activityType: undefined });
  storage.getItem.mockImplementation(async (key) => store.get(key) ?? null);
  storage.setItem.mockImplementation(async (key, value) => { store.set(key, value); });
  contactCard.mockResolvedValue(bytes(2));
  jest.mocked(ContactScanner).mockImplementation(() => <View accessibilityLabel="Scanner fixture" />);
  jest.mocked(ContactQr).mockImplementation(() => <View accessibilityLabel="QR fixture" />);
  jest.mocked(cards.decodeContactCardUri).mockImplementation((uri) => {
    if (!/^card:[0-9]+$/.test(uri)) throw new Error("Invalid signed card");
    return bytes(Number(uri.slice(5)));
  });
  jest.mocked(cards.encodeContactCardUri).mockImplementation((wire) => `card:${new Uint8Array(wire)[0]}`);
  jest.mocked(cards.serviceNameHash).mockReturnValue(bytes(7));
  jest.mocked(cards.verifyContactCard).mockImplementation((wire) => {
    const id = new Uint8Array(wire)[0]!;
    if (id === 255) throw new Error("Invalid signature");
    return { publicIdentity: wire, identityHash: wire, destinationHash: bytes(id + 1),
      serviceNameHash: bytes(id === 254 ? 8 : 7), displayName: `Contact ${id}` };
  });
});
afterEach(() => { stateSubscription.mockRestore(); share.mockRestore(); });

test("scanning only reviews a canonical verified card; saving is a separate explicit identity-scoped action", async () => {
  await render(<ContactExchange {...props} />);
  await fireEvent.press(screen.getByText("Scan a contact QR code"));
  await act(() => scan().onScan(" card:3 "));
  expect(scanner()).toBeNull(); expect(screen.getByText("Verified signed card · group messaging")).toBeTruthy();
  expect(cards.verifyContactCard).toHaveBeenCalledTimes(1); expect(addressBookWrites()).toHaveLength(0);
  expect(screen.getByText("Identity address")).toBeTruthy();
  expect(screen.getByLabelText("Contact identity address")).toHaveTextContent("03");
  await fireEvent.press(screen.getByText("Save this contact"));
  await screen.findByText("Scan a contact QR code");
  await act(async () => {});
  expect(addressBookWrites()).toHaveLength(1); expect(onChange).toHaveBeenCalledWith(expect.arrayContaining([expect.objectContaining({ uri: "card:3" })]));
  expect([...store.keys()][0]).toContain("01:1:02");
});

test.each([["card:255", "Invalid signature"], ["card:254", "does not offer group messaging"], ["https://example.org", "Invalid signed card"]])(
  "invalid or unrelated scanned input %s never reaches saving", async (input, error) => {
    await render(<ContactExchange {...props} />); await fireEvent.press(screen.getByText("Scan a contact QR code"));
    await act(() => scan().onScan(input));
    expect(screen.getByText(new RegExp(error!))).toBeTruthy(); expect(screen.queryByText("Save this contact")).toBeNull();
    expect(addressBookWrites()).toHaveLength(0); expect(scanner()).toBeNull();
    await fireEvent.press(screen.getByText("Scan a contact QR code")); expect(scanner()).toBeTruthy();
  },
);

test("oversized QR input is rejected before native parsing and exact signed URI is otherwise preserved", () => {
  expect(() => reviewContactInput("a".repeat(contactInputBudget + 1))).toThrow("too large");
  expect(cards.decodeContactCardUri).not.toHaveBeenCalled();
  expect(reviewContactInput("card:3").uri).toBe("card:3");
});

test("camera failure leaves paste review usable and does not silently save", async () => {
  await render(<ContactExchange {...props} />); await fireEvent.press(screen.getByText("Scan a contact QR code"));
  await act(() => scan().onFailure("The camera is unavailable. Paste a contact link instead."));
  expect(scanner()).toBeNull();
  await fireEvent.changeText(screen.getByLabelText("Contact link"), "card:3"); await fireEvent.press(screen.getByText("Review contact"));
  expect(screen.getByText("Save this contact")).toBeTruthy(); expect(addressBookWrites()).toHaveLength(0);
});

test("identity/tab switches and background stop the camera and reject its late results", async () => {
  const view = await render(<ContactExchange {...props} />); await fireEvent.press(screen.getByText("Scan a contact QR code"));
  const first = scan(); await view.rerender(<ContactExchange {...props} active={false} />);
  await act(() => first.onScan("card:3")); expect(scanner()).toBeNull(); expect(cards.verifyContactCard).not.toHaveBeenCalled();
  await view.rerender(<ContactExchange {...props} />); expect(scanner()).toBeNull();
  await fireEvent.press(screen.getByText("Scan a contact QR code")); const second = scan();
  await act(() => { appState("background"); second.onScan("card:3"); });
  expect(scanner()).toBeNull(); expect(cards.verifyContactCard).not.toHaveBeenCalled();
  await act(() => appState("active")); expect(scanner()).toBeNull();
});

test("the displayed QR and shared fallback use the same verified signed URI", async () => {
  await render(<ContactExchange {...props} mode="card" />); await fireEvent.press(screen.getByText("Show my QR code"));
  await screen.findByLabelText("QR fixture");
  expect(jest.mocked(ContactQr).mock.calls.at(-1)![0].uri).toBe("card:2");
  expect(screen.queryByText("Scan a contact QR code")).toBeNull();
  expect(screen.queryByText("Review contact")).toBeNull();
  expect(screen.queryByLabelText("Your contact link")).toBeNull();
  expect(screen.queryByLabelText("Your identity address")).toBeNull();
  expect(screen.getByRole("button", { name: "Share this exact contact link" })).toBeEnabled();
  await fireEvent.press(screen.getByRole("button", { name: "Contact link and address" }));
  expect(screen.getByText("Identity address")).toBeTruthy();
  expect(screen.getByLabelText("Your identity address")).toHaveTextContent("02");
  expect(screen.getByLabelText("Your contact link").props.children).toBe("card:2"); expect(share).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByRole("button", { name: "Contact link and address" }));
  expect(screen.queryByLabelText("Your contact link")).toBeNull();
  expect(screen.getByLabelText("QR fixture")).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "Share this exact contact link" })); await act(async () => {});
  expect(share).toHaveBeenCalledWith({ message: "card:2" }); expect(contactCard).toHaveBeenCalledTimes(1);
});

test("delayed signing after hiding the identity never opens the system share sheet", async () => {
  let resolve!: (wire: ArrayBuffer) => void;
  contactCard.mockImplementation(() => new Promise((done) => { resolve = done; }));
  const view = await render(<ContactExchange {...props} mode="card" />); await fireEvent.press(screen.getByText("Share my contact card"));
  await view.rerender(<ContactExchange {...props} mode="card" active={false} />); await act(async () => resolve(bytes(2)));
  expect(share).not.toHaveBeenCalled();
});

test("a signed card for another identity is neither displayed nor shared", async () => {
  contactCard.mockResolvedValue(bytes(3)); await render(<ContactExchange {...props} mode="card" />);
  await fireEvent.press(screen.getByText("Share my contact card"));
  await screen.findByText(/does not belong to the selected identity/);
  expect(screen.queryByLabelText("QR fixture")).toBeNull(); expect(share).not.toHaveBeenCalled();
});

test("contact import drafts survive navigation and stay with their original identity", async () => {
  const first = await render(<ContactExchange {...props} />);
  await fireEvent.changeText(screen.getByLabelText("Contact link"), "card:3");
  await fireEvent.changeText(screen.getByLabelText("Private contact label"), "Alex");
  await act(async () => {});
  await first.unmount();
  const reopened = await render(<ContactExchange {...props} />);
  expect(screen.getByDisplayValue("card:3")).toBeTruthy();
  expect(screen.getByDisplayValue("Alex")).toBeTruthy();
  const other = { ...identity, reference: () => ({ ...reference, publicHash: bytes(4) }) } as unknown as identities.IdentityClientLike;
  await reopened.rerender(<ContactExchange {...props} identity={other} />);
  expect(screen.queryByDisplayValue("card:3")).toBeNull();
  expect(screen.queryByDisplayValue("Alex")).toBeNull();
  expect(addressBookWrites()).toHaveLength(0);
});


test("a contact save completing after navigation still notifies its captured identity's address book", async () => {
  let settle!: () => void;
  storage.setItem.mockImplementation(async (key, value) => {
    if (key.startsWith("prns.app.v1.contacts.")) await new Promise<void>((done) => { settle = done; });
    store.set(key, value);
  });
  const view = await render(<ContactExchange {...props} />);
  await fireEvent.changeText(screen.getByLabelText("Contact link"), "card:3");
  await fireEvent.press(screen.getByText("Review contact"));
  await act(async () => await fireEvent.press(screen.getByText("Save this contact")));
  await view.rerender(<ContactExchange {...props} active={false} />);
  await act(async () => settle());
  expect(onChange).toHaveBeenCalledTimes(1);
  expect(onChange).toHaveBeenCalledWith(expect.arrayContaining([expect.objectContaining({ uri: "card:3" })]));
});


test("contact review is a focused step and editing returns to the same unsaved input", async () => {
  await render(<ContactExchange {...props} />);
  await fireEvent.changeText(screen.getByLabelText("Contact link"), "card:3");
  await fireEvent.changeText(screen.getByLabelText("Private contact label"), "Alex");
  await fireEvent.press(screen.getByRole("button", { name: "Review contact" }));
  expect(screen.queryByLabelText("Contact link")).toBeNull();
  expect(screen.queryByText("Scan a contact QR code")).toBeNull();
  expect(screen.getByRole("button", { name: "Save this contact" })).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "Edit contact" }));
  expect(screen.getByDisplayValue("card:3")).toBeTruthy(); expect(screen.getByDisplayValue("Alex")).toBeTruthy();
  expect(addressBookWrites()).toHaveLength(0); expect(onChange).not.toHaveBeenCalled();
});


test("discovery opens an exact signed-card review without saving or carrying a different draft alias", async () => {
  store.set("prns.ui.contact-import.01:1:02.v1", JSON.stringify({ version: 1, uri: "card:4", alias: "Different person" }));
  await render(<ContactExchange {...props} initialCard="card:3" />);
  expect(screen.getByText("Contact 3")).toBeTruthy();
  expect(screen.queryByText("Different person")).toBeNull(); expect(addressBookWrites()).toHaveLength(0);
  expect(JSON.parse(store.get("prns.ui.contact-import.01:1:02.v1")!).uri).toBe("card:4");
  await fireEvent.press(screen.getByText("Save this contact")); await act(async () => {});
  expect(onChange).toHaveBeenCalledWith(expect.arrayContaining([expect.objectContaining({ uri: "card:3", alias: "" })]));
});
