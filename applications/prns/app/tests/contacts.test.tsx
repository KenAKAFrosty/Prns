import AsyncStorage from "@react-native-async-storage/async-storage";
import { prns_contact_card as cards } from "@prns/app-bindings";
import { loadContacts, removeContact, saveContact, setContactPinned, verifyMessagingContact } from "../src/contacts";

jest.mock("@react-native-async-storage/async-storage", () => ({
  getItem: jest.fn(), setItem: jest.fn(),
}));
jest.mock("../src/identity-creation", () => ({
  hex: (bytes: ArrayBuffer) => Array.from(new Uint8Array(bytes), (byte) => byte.toString(16).padStart(2, "0")).join(""),
}));
jest.mock("@prns/app-bindings", () => ({ prns_contact_card: {
  decodeContactCardUri: jest.fn(), encodeContactCardUri: jest.fn(),
  verifyContactCard: jest.fn(), serviceNameHash: jest.fn(),
} }));
const store = new Map<string, string>();
const storage = jest.mocked(AsyncStorage);
const bytes = (number: number) => new Uint8Array([number]).buffer;
const reference = (sequence = 1n) => ({ id: { incarnation: bytes(1), sequence }, publicHash: bytes(2) });

beforeEach(() => {
  jest.resetAllMocks(); store.clear();
  storage.getItem.mockImplementation(async (key) => store.get(key) ?? null);
  storage.setItem.mockImplementation(async (key, value) => { store.set(key, value); });
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

test("saved contacts are scoped by complete immutable identity, and claims are reverified on load", async () => {
  await saveContact(reference(1n), " card:1 ", " Alice ");
  await saveContact(reference(2n), "card:2", "Bob");
  const alice = await loadContacts(reference(1n));
  expect(alice.map((contact) => contact.alias)).toEqual(["Alice"]);
  expect((await loadContacts(reference(2n)))[0]?.alias).toBe("Bob");
  expect(cards.verifyContactCard).toHaveBeenCalledTimes(4);
  for (const encoded of store.values()) {
    expect(encoded).not.toContain("claims"); // Only user choices and canonical card are persisted.
  }
});

test("invalid signatures and another service are rejected before any address-book write", async () => {
  expect(() => verifyMessagingContact("card:255")).toThrow("Invalid signature");
  expect(() => verifyMessagingContact("card:254")).toThrow("does not offer group messaging");
  expect(storage.setItem).not.toHaveBeenCalled();
});

test("concurrent saves retain both contacts instead of overwriting a stale snapshot", async () => {
  await Promise.all([saveContact(reference(), "card:1", "Alice"), saveContact(reference(), "card:2", "Bob")]);
  expect((await loadContacts(reference())).map((contact) => contact.alias)).toEqual(["Alice", "Bob"]);
});

test("a stale remove cannot erase a contact edited after it was displayed", async () => {
  const [old] = await saveContact(reference(), "card:1", "Alice");
  await saveContact(reference(), "card:1", "Alice at work");
  await expect(removeContact(reference(), old!)).rejects.toThrow("changed");
  expect((await loadContacts(reference()))[0]?.alias).toBe("Alice at work");
});

test("unknown storage outcome can be re-read and explicitly retried without duplicate contacts", async () => {
  storage.setItem.mockImplementationOnce(async (key, value) => { store.set(key, value); throw new Error("Lost write reply"); });
  await expect(saveContact(reference(), "card:1", "Alice")).rejects.toThrow("Lost write reply");
  expect((await loadContacts(reference())).length).toBe(1);
  await saveContact(reference(), "card:1", "Alice");
  expect((await loadContacts(reference())).length).toBe(1);
});

test("corrupt or future storage is preserved, never overwritten with an empty address book", async () => {
  await saveContact(reference(), "card:1", "Alice");
  const key = [...store.keys()][0]!;
  for (const corrupted of ['{"version":2,"contacts":[]}', "not json"]) {
    store.set(key, corrupted);
    await expect(loadContacts(reference())).rejects.toThrow();
    await expect(saveContact(reference(), "card:2", "Bob")).rejects.toThrow();
    expect(store.get(key)).toBe(corrupted);
  }
});


test("pinning is identity-private, sorts saved contacts first, and survives card/name updates", async () => {
  await saveContact(reference(), "card:1", "Alice");
  const [, bob] = await saveContact(reference(), "card:2", "Bob");
  await setContactPinned(reference(), bob!, true);
  const updated = await saveContact(reference(), "card:2", "Robert");
  expect(updated.map((entry) => [entry.alias, entry.pinned === true])).toEqual([["Robert", true], ["Alice", false]]);
  expect((await loadContacts(reference()))[0]?.pinned).toBe(true);
  await saveContact(reference(2n), "card:2", "Other label");
  expect((await loadContacts(reference(2n)))[0]?.pinned).toBe(false);
  expect(JSON.parse([...store.values()][0]!).contacts[0]).toEqual({ uri: "card:2", alias: "Robert", pinned: true });
});

test("pin retries reread an unknown committed write and cannot restore a removed contact", async () => {
  const [contact] = await saveContact(reference(), "card:1", "Alice");
  storage.setItem.mockImplementationOnce(async (key, value) => { store.set(key, value); throw new Error("Lost reply"); });
  await expect(setContactPinned(reference(), contact!, true)).rejects.toThrow("Lost reply");
  expect((await setContactPinned(reference(), contact!, true))[0]?.pinned).toBe(true);
  await removeContact(reference(), contact!);
  await expect(setContactPinned(reference(), contact!, true)).rejects.toThrow("changed");
  expect(await loadContacts(reference())).toEqual([]);
});

test("a concurrent rename keeps the pin and a stale pin cannot overwrite new metadata", async () => {
  const [contact] = await saveContact(reference(), "card:1", "Alice");
  await Promise.all([setContactPinned(reference(), contact!, true), saveContact(reference(), "card:1", "Alice at work")]);
  expect((await loadContacts(reference()))[0]).toMatchObject({ alias: "Alice at work", pinned: true });
  await expect(setContactPinned(reference(), contact!, false)).rejects.toThrow("changed");
  expect((await loadContacts(reference()))[0]?.pinned).toBe(true);
});

test("old contacts load unpinned; malformed pin preferences preserve the address book", async () => {
  await saveContact(reference(), "card:1", "Alice");
  const key = [...store.keys()][0]!;
  store.set(key, JSON.stringify({ version: 1, contacts: [{ uri: "card:1", alias: "Alice" }] }));
  expect((await loadContacts(reference()))[0]?.pinned).toBe(false);
  const malformed = JSON.stringify({ version: 1, contacts: [{ uri: "card:1", alias: "Alice", pinned: "yes" }] });
  store.set(key, malformed);
  await expect(saveContact(reference(), "card:2", "Bob")).rejects.toThrow("preference");
  expect(store.get(key)).toBe(malformed);
});
