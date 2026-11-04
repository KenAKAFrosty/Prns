import AsyncStorage from "@react-native-async-storage/async-storage";
jest.mock("@react-native-async-storage/async-storage", () => require("@react-native-async-storage/async-storage/jest/async-storage-mock"));
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import { IdentityPanel } from "../src/IdentityPanel";
import { ContactList } from "../src/ContactList";
import { loadContacts } from "../src/contacts";
import type { SavedContact } from "../src/contacts";
import type { prns_service_runtime_uniffi as owners, prns_identities as identities } from "@prns/app-bindings";

jest.mock("../src/ContactList", () => ({ ContactList: jest.fn(() => null) }));
jest.mock("../src/messaging/Messaging", () => ({ Messaging: () => null }));
jest.mock("../src/contacts", () => ({
  identityKey: (reference: { id: { sequence: bigint } }) => reference.id.sequence.toString(),
  loadContacts: jest.fn().mockResolvedValue([]),
}));
jest.mock("expo-crypto", () => ({ getRandomBytesAsync: async () => new Uint8Array(32).fill(1) }));
jest.mock("@prns/app-bindings", () => ({
  prns_identities_core: {
    IdentityDesiredState: { Enabled: 0, Paused: 1, Retired: 2 },
    IdentityRuntimeStatus: { Starting: 0, Running: 1, Stopping: 2, Unavailable: 3 },
    IdentityOperationState_Tags: { Reserved: "Reserved", Transitioning: "Transitioning", Completed: "Completed" },
    IdentityTransitionTarget: {
      Retire: class { tag = "Retire"; }, Enable: class { tag = "Enable"; }, Pause: class { tag = "Pause"; },
      AddService: class { tag = "AddService"; readonly inner: readonly [number]; constructor(service: number) { this.inner = [service]; } },
    },
  },
  prns_identities: {
    IdentityCommand: { Transition: { instanceOf: (value: { tag: string }) => value.tag === "Transition" } },
  },
}));

function fixture(configured = true) {
  const reference = { id: { incarnation: new ArrayBuffer(16), sequence: 1n }, publicHash: new ArrayBuffer(16) };
  const record: identities.IdentityRecord = { ...reference, keyReference: new ArrayBuffer(32), lifecycleRevision: 1n, metadata: { localLabel: "Personal", publishedName: "Pat" }, metadataRevision: 1n, desired: 1, initialServices: [], services: configured ? [0x67727073] : [] };
  const snapshot = { identity: record, runtime: 3 };
  const prepared = { identity: reference, target: { tag: "Enable" }, review: {} };
  const token = { incarnation: new ArrayBuffer(16), sequence: 4n, digest: new ArrayBuffer(32) };
  const operations: unknown[] = [];
  const identity = { reference: () => reference, snapshot: jest.fn(async () => snapshot),
    prepare: jest.fn(async () => prepared), begin: jest.fn(async () => undefined),
    applyStop: jest.fn(async () => token), recover: jest.fn(async () => undefined),
    updateMetadata: jest.fn(async (guard: { id: typeof reference.id; publicHash: ArrayBuffer; metadataRevision: bigint }, _metadata: { localLabel: string; publishedName: string }) => ({ ...guard, metadataRevision: guard.metadataRevision + 1n })) };
  const catalog = { identity: () => identity, operations: jest.fn(async () => operations),
    reserve: jest.fn(async () => token), cancelReserved: jest.fn(async () => undefined) };
  const groups = jest.fn(() => ({}));
  const owner = { identities: () => catalog, groups } as unknown as owners.ServiceRuntimeLike;
  return { owner, record, identity, catalog, operations, prepared, token, snapshot, groups };
}
beforeEach(async () => { await AsyncStorage.clear(); jest.useFakeTimers(); jest.mocked(loadContacts).mockReset().mockResolvedValue([]); });
afterEach(async () => { await cleanup(); jest.useRealTimers(); });

test("a lost reservation reply is recovered from the exact native record without another reservation", async () => {
  const data = fixture();
  data.catalog.reserve.mockImplementationOnce(async () => {
    data.operations.push({ token: data.token, command: { tag: "Transition", inner: [data.prepared] }, state: { tag: "Reserved" } });
    throw new Error("Reservation reply lost");
  });
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  await act(async () => { await fireEvent.press(screen.getByText("Start messaging")); });
  expect(screen.getByText("Reservation reply lost")).toBeTruthy();
  expect(data.identity.begin).not.toHaveBeenCalled();
  expect(screen.getByText("Start messaging")).toBeDisabled();
  await act(async () => { await fireEvent.press(screen.getByText("Resume saved identity operation")); });
  expect(data.catalog.reserve).toHaveBeenCalledTimes(1);
  expect(data.identity.begin).toHaveBeenCalledWith(data.token, data.prepared);
});

test("a restarted view resumes an existing transition through native recovery, never a fresh stop", async () => {
  const data = fixture();
  data.operations.push({ token: data.token, command: { tag: "Transition", inner: [data.prepared] }, state: { tag: "Transitioning" } });
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  await act(async () => { await fireEvent.press(screen.getByText("Resume saved identity operation")); });
  expect(data.identity.recover).toHaveBeenCalledTimes(1);
  expect(data.identity.prepare).not.toHaveBeenCalled();
  expect(data.identity.applyStop).not.toHaveBeenCalled();
  expect(data.catalog.reserve).not.toHaveBeenCalled();
});

test("an identity panel does not offer recovery of another identity's saved operation", async () => {
  const data = fixture();
  const other = { ...data.prepared, identity: { ...data.prepared.identity, id: { ...data.prepared.identity.id, sequence: 2n } } };
  data.operations.push({ token: data.token, command: { tag: "Transition", inner: [other] }, state: { tag: "Reserved" } });
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  expect(screen.queryByText("Resume saved identity operation")).toBeNull();
  expect(screen.getByText("Start messaging")).toBeEnabled();
});

function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((yes) => { resolve = yes; }); return { promise, resolve }; }
async function press(title: string) { await act(async () => { await fireEvent.press(screen.getByText(title)); }); }
async function poll() { await act(async () => { jest.advanceTimersByTime(1500); }); }

test("a stale poll cannot remove a newer reserved operation after a lost mutation reply", async () => {
  const data = fixture();
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  const old = deferred<typeof data.snapshot>();
  data.identity.snapshot.mockReturnValueOnce(old.promise);
  data.catalog.operations.mockResolvedValueOnce([]);
  await poll();
  data.catalog.reserve.mockImplementationOnce(async () => {
    data.operations.push({ token: data.token, command: { tag: "Transition", inner: [data.prepared] }, state: { tag: "Reserved" } });
    throw new Error("Reservation reply lost");
  });
  await press("Start messaging");
  expect(screen.getByText("Start messaging")).toBeDisabled();
  await act(async () => { old.resolve(data.snapshot); });
  expect(screen.getByText("Start messaging")).toBeDisabled();
  expect(screen.getByText("Resume saved identity operation")).toBeEnabled();
});

test("dirty name edits retain their original guard across a newer polled revision", async () => {
  const data = fixture();
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  await fireEvent.changeText(screen.getByLabelText("Edit private identity label"), "My draft");
  data.snapshot.identity = { ...data.record, metadata: { localLabel: "Changed elsewhere", publishedName: "New name" }, metadataRevision: 2n };
  await poll();
  expect(screen.getByLabelText("Edit private identity label")).toHaveDisplayValue("My draft");
  expect(screen.getByText("Save identity names")).toBeDisabled();
  expect(data.identity.updateMetadata).not.toHaveBeenCalled();
  await press("Discard name edits and reload");
  expect(screen.getByLabelText("Edit private identity label")).toHaveDisplayValue("Changed elsewhere");
  await fireEvent.changeText(screen.getByLabelText("Edit private identity label"), "Reviewed edit");
  await press("Save identity names");
  expect(data.identity.updateMetadata).toHaveBeenCalledWith(expect.objectContaining({ metadataRevision: 2n }), { localLabel: "Reviewed edit", publishedName: "New name" });
});

test("lost metadata reply preserves the guarded draft and requires review of newer evidence", async () => {
  const data = fixture();
  data.identity.updateMetadata.mockImplementationOnce(async (_guard, metadata) => {
    data.snapshot.identity = { ...data.record, metadata, metadataRevision: 2n };
    throw new Error("Metadata reply lost");
  });
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  await fireEvent.changeText(screen.getByLabelText("Edit private identity label"), "My label");
  await press("Save identity names");
  expect(data.identity.updateMetadata).toHaveBeenCalledWith(expect.objectContaining({ metadataRevision: 1n }), expect.objectContaining({ localLabel: "My label" }));
  expect(screen.getByLabelText("Edit private identity label")).toHaveDisplayValue("My label");
  expect(screen.getByText("Save identity names")).toBeDisabled();
  expect(screen.getByText("Discard name edits and reload")).toBeEnabled();
});

test("desired paused does not claim cleanup finished while native runtime is stopping", async () => {
  const data = fixture();
  data.snapshot.runtime = 2;
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  expect(screen.getByText("Stopping messaging. Finishing accepted work; history and contacts remain available.")).toBeTruthy();
  expect(screen.queryByText("Messaging is paused. Saved history and contacts remain available.")).toBeNull();
});

test("an initial contact read failure has an explicit retry without reopening the owner", async () => {
  jest.mocked(loadContacts).mockRejectedValueOnce(new Error("Storage temporarily unavailable"));
  const data = fixture();
  await render(<IdentityPanel section="Contacts" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  expect(screen.getByText("Storage temporarily unavailable")).toBeTruthy();
  await press("Retry loading contacts");
  expect(loadContacts).toHaveBeenCalledTimes(2);
  expect(screen.queryByText("Retry loading contacts")).toBeNull();
  expect(screen.queryByText("Refresh contacts")).toBeNull();
  expect(screen.queryByText("Storage temporarily unavailable")).toBeNull();
});

test("an older contact refresh cannot overwrite a newly saved contact list", async () => {
  const data = fixture();
  const props = { owner: data.owner, record: data.record, active: true, onChanged: jest.fn(async () => undefined) };
  const view = await render(<IdentityPanel section="Contacts" {...props} contactsRevision={0} />);
  const old = deferred<readonly SavedContact[]>();
  jest.mocked(loadContacts).mockReturnValueOnce(old.promise);
  await view.rerender(<IdentityPanel section="Contacts" {...props} contactsRevision={1} />);
  expect(loadContacts).toHaveBeenCalledTimes(2);
  expect(loadContacts).toHaveBeenLastCalledWith(expect.objectContaining({ id: data.record.id, publicHash: data.record.publicHash }));
  const fresh = [{ uri: "verified", alias: "Fresh", claims: {} } as SavedContact];
  const contactProps = jest.mocked(ContactList).mock.calls.at(-1)![0];
  await act(async () => { contactProps.onChange(fresh); });
  await act(async () => { old.resolve([]); });
  expect(jest.mocked(ContactList).mock.calls.at(-1)![0].contacts).toEqual(fresh);
  expect(screen.queryByText("Retry loading contacts")).toBeNull();
  expect(screen.queryByText("Refresh contacts")).toBeNull();
});


test("fresh identity requires explicit retained setup before group access and a separate Start", async () => {
  const data = fixture(false);
  const setup = { ...data.prepared, target: { tag: "AddService", inner: [0x67727073] } };
  data.identity.prepare.mockResolvedValueOnce(setup);
  data.identity.begin.mockImplementationOnce(async () => {
    data.snapshot.identity = { ...data.record, services: [0x67727073] };
  });
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  expect(data.groups).not.toHaveBeenCalled();
  expect(screen.queryByText("Start messaging")).toBeNull();
  expect(screen.getByText("Set up messaging")).toBeEnabled();
  await press("Set up messaging");
  expect(data.identity.prepare).toHaveBeenCalledWith(expect.objectContaining({ tag: "AddService", inner: [0x67727073] }));
  expect(data.catalog.reserve).toHaveBeenCalledWith(setup, expect.any(ArrayBuffer));
  expect(data.identity.begin).toHaveBeenCalledWith(data.token, setup);
  expect(data.groups).toHaveBeenCalledTimes(1);
  expect(screen.queryByText("Set up messaging")).toBeNull();
  expect(screen.getByText("Start messaging")).toBeEnabled();
  expect(data.identity.prepare).toHaveBeenCalledTimes(1);
  await press("Start messaging");
  expect(data.identity.prepare).toHaveBeenLastCalledWith(expect.objectContaining({ tag: "Enable" }));
});

test("lost setup reservation reply resumes the exact native AddService and never recreates it", async () => {
  const data = fixture(false);
  const setup = { ...data.prepared, target: { tag: "AddService", inner: [0x67727073] } };
  data.identity.prepare.mockResolvedValueOnce(setup);
  data.catalog.reserve.mockImplementationOnce(async () => {
    data.operations.push({ token: data.token, command: { tag: "Transition", inner: [setup] }, state: { tag: "Reserved" } });
    throw new Error("Setup reservation reply lost");
  });
  const view = await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  await press("Set up messaging");
  expect(screen.getByText("Set up messaging")).toBeDisabled();
  expect(data.groups).not.toHaveBeenCalled();
  await view.unmount();
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  await press("Review saved setup");
  expect(screen.getByText("Set up messaging: waiting for confirmation")).toBeTruthy();
  await press("Resume saved identity operation");
  expect(data.identity.begin).toHaveBeenCalledWith(data.token, setup);
  expect(data.catalog.reserve).toHaveBeenCalledTimes(1);
  expect(data.identity.prepare).toHaveBeenCalledTimes(1);
  expect(data.groups).not.toHaveBeenCalled();
});


test("enabled but unavailable messaging uses canonical recovery without a new Enable reservation", async () => {
  const data = fixture();
  data.record.desired = 0;
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  await act(async () => { await fireEvent.press(screen.getByText("Resume messaging")); });
  expect(data.identity.recover).toHaveBeenCalledTimes(1);
  expect(data.identity.prepare).not.toHaveBeenCalled();
  expect(data.catalog.reserve).not.toHaveBeenCalled();
});


test("identity retirement reviews first and confirms the exact prepared native stop", async () => {
  const data = fixture();
  const prepared = { ...data.prepared, target: { tag: "Retire" } };
  data.identity.prepare.mockResolvedValue(prepared);
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  await act(async () => { await fireEvent.press(screen.getByText("Review identity retirement")); });
  expect(data.identity.applyStop).not.toHaveBeenCalled();
  expect(screen.getByText(/permanently prevents new messaging/)).toBeTruthy();
  await act(async () => { await fireEvent.press(screen.getByText("Confirm identity retirement")); });
  expect(data.identity.applyStop).toHaveBeenCalledWith(prepared, expect.any(ArrayBuffer));
  expect(data.identity.prepare).toHaveBeenCalledTimes(1);
  expect(data.catalog.reserve).not.toHaveBeenCalled();
});

test("blocked retirement cannot reach confirmation or mutate the identity", async () => {
  const data = fixture();
  data.identity.prepare.mockRejectedValue(new Error("Group ownership must be resolved"));
  await render(<IdentityPanel section="Identity" owner={data.owner} record={data.record} active onChanged={async () => undefined} />);
  await act(async () => { await fireEvent.press(screen.getByText("Review identity retirement")); });
  expect(screen.getByText("Group ownership must be resolved")).toBeTruthy();
  expect(screen.queryByText("Confirm identity retirement")).toBeNull();
  expect(data.identity.applyStop).not.toHaveBeenCalled();
});


test("contact changes notify sibling routes and a revision reloads a hidden address book", async () => {
  const data = fixture(); const changed = jest.fn();
  const view = await render(<IdentityPanel section="Contacts" owner={data.owner} record={data.record} active onChanged={async () => undefined} onContactsChanged={changed} contactsRevision={0} />);
  const current = jest.mocked(ContactList).mock.calls.at(-1)![0];
  await act(async () => current.onChange([]));
  expect(changed).toHaveBeenCalledTimes(1);
  const fresh = [{ uri: "verified", alias: "Updated elsewhere", claims: {} } as SavedContact];
  jest.mocked(loadContacts).mockResolvedValue(fresh);
  await view.rerender(<IdentityPanel section="Contacts" owner={data.owner} record={data.record} active={false} onChanged={async () => undefined} onContactsChanged={changed} contactsRevision={1} />);
  expect(jest.mocked(ContactList).mock.calls.at(-1)![0].contacts).toEqual(fresh);
});
