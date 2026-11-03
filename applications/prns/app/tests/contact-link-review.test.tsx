import { act, fireEvent, render, screen } from "@testing-library/react-native";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { prns_contact_card as cards } from "@prns/app-bindings";
import type { prns_identities as identities, prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { ContactLinkReview } from "../src/ContactLinkReview";
import { loadContacts, saveContact } from "../src/contacts";

jest.mock("@react-native-async-storage/async-storage", () => ({ getItem: jest.fn(), setItem: jest.fn() }));
jest.mock("@prns/app-bindings", () => ({ prns_contact_card: {
  decodeContactCardUri: jest.fn(), encodeContactCardUri: jest.fn(), verifyContactCard: jest.fn(), serviceNameHash: jest.fn(),
} }));
const bytes = (n: number) => new Uint8Array([n]).buffer;
const record = (sequence: bigint, label: string) => ({ id: { incarnation: bytes(1), sequence }, publicHash: bytes(Number(sequence)), metadata: { localLabel: label, publishedName: label } }) as identities.IdentityRecord;
const personal = record(2n, "Personal"); const work = record(3n, "Work");
const records = jest.fn<Promise<identities.IdentityRecord[]>, unknown[]>();
const owner = { identities: () => ({ identities: records }) } as unknown as owners.ServiceRuntimeLike;
const store = new Map<string, string>(); const storage = jest.mocked(AsyncStorage);
const onDone = jest.fn(); const onSaved = jest.fn();
const props = { link: { id: 1, url: "rns-contact://v1/4" }, owner, active: true, onDone, onSaved };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((yes) => { resolve = yes; }); return { promise, resolve }; }
async function choose(label = "Personal") {
  await fireEvent.press(await screen.findByText(`Save contact under ${label}`)); await act(async () => {});
}
beforeEach(() => {
  jest.resetAllMocks(); store.clear(); records.mockResolvedValue([personal, work]);
  storage.getItem.mockImplementation(async (key) => store.get(key) ?? null);
  storage.setItem.mockImplementation(async (key, value) => { store.set(key, value); });
  jest.mocked(cards.decodeContactCardUri).mockImplementation((uri) => {
    if (!/^rns-contact:\/\/v1\/[0-9]+$/.test(uri)) throw new Error("Invalid signed card");
    return bytes(Number(uri.split("/").at(-1)));
  });
  jest.mocked(cards.encodeContactCardUri).mockImplementation((wire) => `rns-contact://v1/${new Uint8Array(wire)[0]}`);
  jest.mocked(cards.serviceNameHash).mockReturnValue(bytes(7));
  jest.mocked(cards.verifyContactCard).mockImplementation((wire) => {
    const n = new Uint8Array(wire)[0]!; if (n === 255) throw new Error("Invalid signature");
    return { identityHash: wire, publicIdentity: wire, destinationHash: bytes(n + 1), serviceNameHash: bytes(n === 254 ? 8 : 7), displayName: "Remote person" };
  });
});

test("a link arriving before readiness waits for an explicitly chosen immutable destination and explicit save", async () => {
  const view = await render(<ContactLinkReview {...props} owner={undefined} />);
  expect(screen.getByText(/Open or set up this phone/)).toBeTruthy(); expect(records).not.toHaveBeenCalled();
  expect(storage.setItem).not.toHaveBeenCalled();
  await view.rerender(<ContactLinkReview {...props} />); await screen.findByText("Save contact under Personal");
  expect(screen.queryByText("Save reviewed incoming contact")).toBeNull();
  await choose("Work"); expect(storage.setItem).not.toHaveBeenCalled();
  await fireEvent.changeText(screen.getByLabelText("Incoming contact private label"), "Friend at work");
  await fireEvent.press(screen.getByText("Save reviewed incoming contact")); await act(async () => {});
  expect((await loadContacts(work))[0]?.alias).toBe("Friend at work"); expect(await loadContacts(personal)).toEqual([]);
  expect(onSaved).toHaveBeenCalledWith({ id: work.id, publicHash: work.publicHash }, expect.any(Array)); expect(onDone).toHaveBeenCalledWith(1);
});

test("an existing private alias is preserved instead of replaced by the card's shared name or blank input", async () => {
  await saveContact(personal, props.link.url, "Private friend label"); storage.setItem.mockClear();
  await render(<ContactLinkReview {...props} />); await choose();
  expect(screen.getByLabelText("Incoming contact private label").props.value).toBe("Private friend label");
  await fireEvent.press(screen.getByText("Save reviewed incoming contact")); await act(async () => {});
  expect((await loadContacts(personal))[0]?.alias).toBe("Private friend label");
});

test.each([255, 254])("invalid signature or service %s never offers a destination or writes contacts", async (n) => {
  await render(<ContactLinkReview {...props} link={{ id: 1, url: `rns-contact://v1/${n}` }} />); await act(async () => {});
  expect(screen.getByRole("alert")).toBeTruthy(); expect(records).not.toHaveBeenCalled(); expect(storage.setItem).not.toHaveBeenCalled();
});

test("catalog and address-book read failures preserve the same review and allow explicit retry", async () => {
  records.mockRejectedValueOnce(new Error("Catalog unavailable")); await render(<ContactLinkReview {...props} />);
  await screen.findByText("Catalog unavailable"); await fireEvent.press(screen.getByText("Retry loading destination identities"));
  storage.getItem.mockRejectedValueOnce(new Error("Address book unavailable")); await choose();
  expect(screen.getByText("Address book unavailable")).toBeTruthy(); expect(storage.setItem).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByText("Retry loading destination contacts")); await act(async () => {});
  expect(screen.getByText("Save reviewed incoming contact")).toBeEnabled();
});

test("a lost save reply retains exact destination and alias; retry writes the same contact without duplication", async () => {
  storage.setItem.mockImplementationOnce(async (key, value) => { store.set(key, value); throw new Error("Lost save reply"); });
  await render(<ContactLinkReview {...props} />); await choose();
  await fireEvent.changeText(screen.getByLabelText("Incoming contact private label"), "Retained alias");
  await fireEvent.press(screen.getByText("Save reviewed incoming contact")); await screen.findByText("Lost save reply");
  expect(screen.getByText("Save contact under Work")).toBeDisabled(); expect(screen.getByLabelText("Incoming contact private label")).toBeDisabled(); expect(onDone).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByText("Retry saving contact")); await act(async () => {});
  const [first, second] = storage.setItem.mock.calls;
  expect(second).toEqual(first); expect((await loadContacts(personal)).length).toBe(1); expect(onDone).toHaveBeenCalledWith(1);
});

test("stale destination contacts cannot relabel a newly selected identity and owner replacement cannot retarget a pending save", async () => {
  const read = deferred<string | null>(); storage.getItem.mockImplementationOnce(() => read.promise);
  const view = await render(<ContactLinkReview {...props} />); await choose();
  await choose("Work"); await fireEvent.changeText(screen.getByLabelText("Incoming contact private label"), "Work label");
  await act(async () => read.resolve(JSON.stringify({ version: 1, contacts: [{ uri: props.link.url, alias: "Old personal label" }] })));
  expect(screen.getByLabelText("Incoming contact private label").props.value).toBe("Work label");
  const replacement = { identities: () => ({ identities: async () => [personal] }) } as unknown as owners.ServiceRuntimeLike;
  await view.rerender(<ContactLinkReview {...props} owner={replacement} />); await act(async () => {});
  expect(screen.getByText(/identity you chose is no longer available/)).toBeTruthy();
  expect(screen.getByText("Save reviewed incoming contact")).toBeDisabled(); expect(storage.setItem).not.toHaveBeenCalled();
});

test("backgrounded review cannot start saving; an already-started save retains its original destination", async () => {
  const write = deferred<void>(); storage.setItem.mockImplementationOnce(async (key, value) => { await write.promise; store.set(key, value); });
  const view = await render(<ContactLinkReview {...props} />); await choose();
  await view.rerender(<ContactLinkReview {...props} active={false} />);
  await fireEvent.press(screen.getByText("Save reviewed incoming contact")); expect(storage.setItem).not.toHaveBeenCalled();
  await view.rerender(<ContactLinkReview {...props} />); await fireEvent.press(screen.getByText("Save reviewed incoming contact")); await act(async () => {});
  await view.rerender(<ContactLinkReview {...props} active={false} owner={undefined} />);
  await act(async () => write.resolve());
  expect(onSaved).toHaveBeenCalledWith({ id: personal.id, publicHash: personal.publicHash }, expect.any(Array)); expect(onDone).toHaveBeenCalledWith(1);
});


test("new destination identities arrive through catalog updates without a refresh or automatic save", async () => {
  records.mockResolvedValue([]);
  const view = await render(<ContactLinkReview {...props} catalogRevision="empty" />);
  await screen.findByText(/It will appear here automatically/);
  expect(screen.queryByText("Refresh destination identities")).toBeNull();
  records.mockResolvedValue([personal]);
  await view.rerender(<ContactLinkReview {...props} catalogRevision="personal" />);
  await screen.findByText("Save contact under Personal");
  expect(storage.setItem).not.toHaveBeenCalled(); expect(onSaved).not.toHaveBeenCalled();
});
