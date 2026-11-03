import { Text } from "react-native";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import type { prns_identities as identities } from "@prns/app-bindings";
import { ContactList } from "../src/ContactList";
import { ContactWorkflowScreen } from "../src/screens/ContactWorkflowScreen";
import { ContactExchange } from "../src/ContactExchange";
import { identityKey } from "../src/identity-reference";

jest.mock("expo-router", () => ({
  useLocalSearchParams: () => ({ owner: mockReference, card: mockCard }), useRouter: () => ({ replace: mockReplace }),
  Link: ({ href, children }: { href: unknown; children: React.ReactElement }) => {
    const React = require("react"); return React.cloneElement(children, { onPress: () => mockLink(href) });
  },
}));
jest.mock("../src/navigation", () => ({ scalar: (value: unknown) => typeof value === "string" ? value : undefined,
  useRouteLease: () => ({ active: mockActive, isCurrent: () => mockActive }) }));
jest.mock("../src/runtime/InstallationProvider", () => ({ useInstallation: () => ({
  records: mockRecords, owner: mockOwner, selected: "02:2:02", contactSaved: mockSaved,
}) }));
jest.mock("../src/ContactExchange", () => ({ ContactExchange: jest.fn(({ mode }) => { const { Text } = require("react-native"); return <Text>Exchange: {mode}</Text>; }) }));
jest.mock("@prns/app-bindings", () => ({ prns_contact_card: {} }));
jest.mock("@react-native-async-storage/async-storage", () => ({ getItem: jest.fn(), setItem: jest.fn() }));
const bytes = (value: number) => new Uint8Array([value]).buffer;
const mockRecords = [1, 2].map((value) => ({ id: { incarnation: bytes(value), sequence: BigInt(value) }, publicHash: bytes(value),
  metadata: { publishedName: `Person ${value}` }, services: [0x67727073] }));
let mockCard: string | undefined; let mockReference: string | undefined; let mockActive = true;
const mockIdentity = jest.fn((reference) => ({ reference: () => reference }));
const mockOwner = { identities: () => ({ identity: mockIdentity }) };
const mockSaved = jest.fn(); const mockReplace = jest.fn(); const mockLink = jest.fn();
beforeEach(() => { jest.clearAllMocks(); mockCard = undefined; mockActive = true; mockReference = identityKey(mockRecords[0]!); });

test("saved contacts lead with pinned rows and route add/card to the full owning identity", async () => {
  const make = (value: number, pinned = false) => ({ uri: `card:${value}`, alias: `Person ${value}`, pinned,
    claims: { identityHash: bytes(value), serviceNameHash: bytes(7) } });
  const rows = [make(3), make(4, true)] as never;
  const identity = { reference: () => mockRecords[0]! } as unknown as identities.IdentityClientLike;
  const open = jest.fn();
  await render(<ContactList identity={identity} displayName="Owner" contacts={rows} active onChange={jest.fn()} onOpenContact={open} />);
  const contacts = screen.getAllByRole("button", { name: /^Contact details:/ });
  expect(contacts.map((item) => item.props.accessibilityLabel)).toEqual(["Contact details: Person 4", "Contact details: Person 3"]);
  await fireEvent.press(contacts[0]!); expect(open).toHaveBeenCalledWith(rows[1]);
  await fireEvent.press(screen.getByText("Add contact"));
  expect(mockLink).toHaveBeenLastCalledWith({ pathname: "/contacts/add", params: { owner: identityKey(mockRecords[0]!) } });
  await fireEvent.press(screen.getByRole("button", { name: "My contact card" }));
  expect(mockLink).toHaveBeenLastCalledWith({ pathname: "/contacts/card", params: { owner: identityKey(mockRecords[0]!) } });
  await fireEvent.press(screen.getByRole("button", { name: "Find people" }));
  expect(mockLink).toHaveBeenLastCalledWith({ pathname: "/contacts/discover", params: { owner: identityKey(mockRecords[0]!) } });
  expect(screen.queryByText("Review contact")).toBeNull();
});

test("card/add workflow captures route owner instead of the current identity selection", async () => {
  const view = await render(<ContactWorkflowScreen mode="add" />);
  expect(mockIdentity).toHaveBeenCalledWith({ id: mockRecords[0]!.id, publicHash: mockRecords[0]!.publicHash });
  const captured = jest.mocked(ContactExchange).mock.calls.at(-1)![0];
  expect(captured.displayName).toBe("Person 1");
  mockReference = identityKey(mockRecords[1]!); await view.rerender(<ContactWorkflowScreen mode="card" />);
  await act(async () => captured.onChange([]));
  expect(mockSaved).toHaveBeenCalledTimes(1); expect(mockReplace).not.toHaveBeenCalled();
  expect(screen.getByText("My contact card")).toBeTruthy();
  expect(mockIdentity).toHaveBeenLastCalledWith({ id: mockRecords[1]!.id, publicHash: mockRecords[1]!.publicHash });
  expect(screen.queryByText("Back to contacts")).toBeNull(); // The shared shell owns Back.
});

test("missing explicit owner never silently imports into a selected identity", async () => {
  mockReference = undefined;
  await render(<ContactWorkflowScreen mode="add" />);
  expect(mockIdentity).not.toHaveBeenCalled(); expect(ContactExchange).not.toHaveBeenCalled();
  expect(screen.getByText(/Open this screen from the contacts/)).toBeTruthy();
});


test("a completed save for an earlier discovery card cannot close a newer card review", async () => {
  mockCard = "card:3";
  const view = await render(<ContactWorkflowScreen mode="add" />);
  const captured = jest.mocked(ContactExchange).mock.calls.at(-1)![0];
  expect(captured.initialCard).toBe("card:3");
  mockCard = "card:4"; await view.rerender(<ContactWorkflowScreen mode="add" />);
  await act(async () => captured.onChange([]));
  expect(mockSaved).toHaveBeenCalledTimes(1); expect(mockReplace).not.toHaveBeenCalled();
});
