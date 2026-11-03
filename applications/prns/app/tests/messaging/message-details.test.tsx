import "./native-mock";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react-native";
import { prns_groups as groups, prns_groups_core as protocol } from "@prns/app-bindings";
import type { SavedContact } from "../../src/contacts";
import { hex } from "../../src/bytes";
import { MessageDetails } from "../../src/messaging/MessageDetails";

const bytes = (size: number, fill: number) => new Uint8Array(size).fill(fill).buffer;
const group = bytes(32, 1), incarnation = bytes(24, 2), id = bytes(32, 3), author = bytes(16, 4);
const endpoint = (fill: number): protocol.GroupEndpoint => ({ id: bytes(16, fill), incarnation: 1n });
const sender = endpoint(5), recipient = endpoint(6), another = endpoint(7);
const contact: SavedContact = { uri: "already-verified-card", alias: "Garden tablet", claims: {
  identityHash: author, displayName: "Published name", publicIdentity: bytes(64, 4), serviceNameHash: bytes(10, 8), destinationHash: bytes(16, 9),
} };
const props = { group: hex(group), incarnation: hex(incarnation), messageId: hex(id), contacts: [contact], active: true, revision: 1, conversationTitle: "Garden" };
function snapshot(selected = incarnation, outgoing = true) {
  const position = { group, incarnation: selected, position: 10n };
  const record: groups.GroupStoredRecord = {
    record: { metadata: { group, record: id, author: sender, namespace: 1, schema: 1,
      validity: { createdAt: 1_700_000_000n, deadline: 1_700_086_400n }, originalControl: { era: bytes(32, 10), head: bytes(32, 11) } },
      nonce: bytes(32, 12), targets: [recipient], body: new ArrayBuffer(0) },
    position, authorIdentity: author, outgoing, clientRequestId: outgoing ? bytes(32, 12) : undefined,
    progress: { membershipChanged: false, summary: { originalTargets: 1n, received: 0n, removed: 0n, pending: 1n, expired: 0n, timeUncertain: 0n } },
  };
  const message: groups.GroupTextMessage = { id, position, author, outgoing, text: "A saved garden message", sentAtMillis: 1_700_000_000_000n,
    receivedCount: 0n, recipientCount: 1n, membershipChanged: false };
  return { record, message };
}
function target(endpointValue = recipient, receipt = protocol.GroupReceiptStatus.Awaiting, removal = protocol.GroupRemovalStatus.NotKnownRemoved): groups.GroupDeliveryTarget {
  return { endpoint: endpointValue, identity: author, receipt, removal };
}
function fixture(value = snapshot()) {
  const native = {
    message: jest.fn<Promise<groups.GroupTextMessage | undefined>, unknown[]>(async () => value.message),
    record: jest.fn<Promise<groups.GroupStoredRecord | undefined>, unknown[]>(async () => value.record),
    deliveryTargets: jest.fn<Promise<groups.GroupDeliveryTargetPage | undefined>, unknown[]>(async () => ({ targets: [target()], next: undefined })),
    changes: jest.fn(async (_group: ArrayBuffer, after: groups.GroupChangeToken | undefined): Promise<groups.GroupRecordChanges> => {
      const token = { group, incarnation: value.record.position.incarnation, position: after?.position ?? 0n };
      return { changes: [], next: token, head: token, historyPruned: false };
    }),
    messages: jest.fn(), records: jest.fn(), execute: jest.fn(), publishRequest: jest.fn(),
  };
  return { native, client: native as unknown as groups.ClientLike, ...value };
}
function deferred<T>() { let resolve!: (value: T) => void, reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
async function press(name: string) { await act(async () => { await fireEvent.press(screen.getByRole("button", { name })); }); }
afterEach(cleanup);

test("reads the exact retained message and labels the sender clock without claiming delivery time", async () => {
  const { client, native } = fixture();
  await render(<MessageDetails {...props} client={client} />);
  expect(native.message.mock.calls[0]?.slice(0, 3)).toEqual([group, incarnation, id]);
  expect(native.record.mock.calls[0]?.slice(0, 4)).toEqual(native.message.mock.calls[0]?.slice(0, 4));
  expect(native.message.mock.calls[0]?.[3]).toEqual(expect.objectContaining({ tag: protocol.GroupClockReading_Tags.Trusted }));
  expect(screen.getByText("You")).toBeTruthy();
  expect(screen.getByText("A saved garden message")).toBeTruthy();
  expect(screen.getByText("Sender’s timestamp")).toBeTruthy();
  expect(screen.getByText(new Date(1_700_000_000_000).toLocaleString())).toBeTruthy();
  expect(screen.queryByText(/Delivered at|Read at/)).toBeNull();
  await press("Technical details");
  expect(screen.getByText("Local membership reference")).toBeTruthy();
  expect(screen.getByText(hex(incarnation))).toBeTruthy();
  expect(screen.getByText("Delivery deadline")).toBeTruthy();
  expect(screen.getByText(/deadline does not delete/)).toBeTruthy();
  expect(native.messages).not.toHaveBeenCalled();
  expect(native.records).not.toHaveBeenCalled();
});

test("incoming detail uses authenticated author and local-save wording", async () => {
  const { client } = fixture(snapshot(incarnation, false));
  await render(<MessageDetails {...props} client={client} localIdentity={author} />);
  expect(screen.getByText("Garden tablet")).toBeTruthy();
  expect(screen.getByText("Saved on this phone")).toBeTruthy();
  expect(screen.getByText(/Your identity · device/)).toBeTruthy();
  expect(screen.queryByText("You")).toBeNull();
  await press("About message details");
  expect(screen.getByText(/do not mean someone read it/)).toBeTruthy();
});

test.each([
  ["pending", "Waiting for confirmation: 1", "Awaiting confirmation"],
  ["expired", "Delivery window ended without confirmation: 1", "Delivery window ended"],
  ["timeUncertain", "Delivery window cannot be checked because of the device clock: 1", "Delivery window cannot be checked"],
] as const)("%s is separate from overlapping received and removed evidence", async (window, summaryLabel, peerLabel) => {
  const value = snapshot();
  value.record.record.targets = [recipient, another];
  value.record.progress = { membershipChanged: true, summary: { originalTargets: 2n, received: 1n, removed: 1n, pending: 0n, expired: 0n, timeUncertain: 0n, [window]: 1n } };
  // The point text projection was read separately; the generic summary is the authority.
  value.message.receivedCount = 0n;
  const { client, native } = fixture(value);
  native.deliveryTargets.mockResolvedValue({ targets: [target(recipient, protocol.GroupReceiptStatus.Received, protocol.GroupRemovalStatus.Removed), target(another)], next: undefined });
  await render(<MessageDetails {...props} client={client} />);
  expect(screen.getByText("1 of 2 recipient devices confirmed saving this message.")).toBeTruthy();
  expect(screen.getByText("Saved by 1 of 2 devices · membership changed")).toBeTruthy();
  expect(screen.getByText("No longer in the conversation: 1")).toBeTruthy();
  expect(screen.getByText(summaryLabel)).toBeTruthy();
  expect(screen.getByText("Save confirmed")).toBeTruthy();
  expect(screen.getByText("No longer in the conversation")).toBeTruthy();
  expect(screen.getByText(peerLabel)).toBeTruthy();
  expect(screen.getByText(/Membership changed before every original recipient/)).toBeTruthy();
  expect(screen.queryByText(/Read by/)).toBeNull();
});

test("an unconfirmed removed recipient is not presented as received or still awaiting delivery", async () => {
  const value = snapshot();
  value.record.progress.summary = { originalTargets: 1n, received: 0n, removed: 1n, pending: 0n, expired: 0n, timeUncertain: 0n };
  const { client, native } = fixture(value);
  native.deliveryTargets.mockResolvedValue({ targets: [target(recipient, protocol.GroupReceiptStatus.Awaiting, protocol.GroupRemovalStatus.Removed)], next: undefined });
  await render(<MessageDetails {...props} client={client} />);
  expect(screen.getByText("No confirmation recorded")).toBeTruthy();
  expect(screen.getByText("No longer in the conversation")).toBeTruthy();
  expect(screen.queryByText("Save confirmed")).toBeNull();
  expect(screen.queryByText("Awaiting confirmation")).toBeNull();
});

test("recipient paging uses the selected incarnation and endpoint and can return to the first page", async () => {
  const value = snapshot(); value.record.record.targets.push(another);
  const { client, native } = fixture(value);
  native.deliveryTargets.mockResolvedValueOnce({ targets: [target()], next: recipient })
    .mockResolvedValueOnce({ targets: [{ ...target(another), identity: undefined }], next: undefined })
    .mockResolvedValueOnce({ targets: [target()], next: recipient });
  await render(<MessageDetails {...props} client={client} />);
  expect(native.deliveryTargets.mock.calls[0]?.slice(0, 5)).toEqual([group, incarnation, id, undefined, { maxRows: 32, maxBytes: 16n * 1024n }]);
  await press("Next recipient devices");
  expect(native.deliveryTargets.mock.calls[1]?.[3]).toEqual(recipient);
  expect(screen.getByText("Device 07070707")).toBeTruthy();
  expect(screen.queryByText(/device 06060606/)).toBeNull();
  await press("First recipient devices page");
  expect(native.deliveryTargets.mock.calls[2]?.[3]).toBeUndefined();
  expect(screen.getByText(/Garden tablet · device 06060606/)).toBeTruthy();
});

test("a recipient outside the message's original endpoint set fails closed without hiding the message", async () => {
  const { client, native } = fixture();
  native.deliveryTargets.mockResolvedValue({ targets: [target(another)], next: undefined });
  await render(<MessageDetails {...props} client={client} />);
  expect(screen.getByText("Recipient details did not match this message.")).toBeTruthy();
  expect(screen.queryByText(/device 07070707/)).toBeNull();
  expect(screen.getByText("A saved garden message")).toBeTruthy();
});

test.each(["missing text", "missing record"])("%s does not fall back to a history scan or request an unrelated target page", async (missing) => {
  const { client, native } = fixture();
  if (missing === "missing text") native.message.mockResolvedValue(undefined);
  else native.record.mockResolvedValue(undefined);
  await render(<MessageDetails {...props} client={client} />);
  expect(screen.getByText(/This message is no longer available/)).toBeTruthy();
  expect(native.deliveryTargets).not.toHaveBeenCalled();
  expect(native.changes).not.toHaveBeenCalled();
  expect(native.messages).not.toHaveBeenCalled();
  expect(native.records).not.toHaveBeenCalled();
});

test.each(["message id", "record id", "message group", "record group", "message incarnation", "record incarnation", "author", "direction", "position", "metadata group"])(
  "a mismatched native %s never renders content or recipient evidence", async (fault) => {
    const value = snapshot();
    // Each response owns its provenance, even when the native values coincide.
    value.message.position = { ...value.message.position };
    value.record.position = { ...value.record.position };
    if (fault === "message id") value.message.id = bytes(32, 99);
    if (fault === "record id") value.record.record.metadata.record = bytes(32, 99);
    if (fault === "message group") value.message.position.group = bytes(32, 99);
    if (fault === "record group") value.record.position.group = bytes(32, 99);
    if (fault === "message incarnation") value.message.position.incarnation = bytes(24, 99);
    if (fault === "record incarnation") value.record.position.incarnation = bytes(24, 99);
    if (fault === "author") value.message.author = bytes(16, 99);
    if (fault === "direction") value.message.outgoing = false;
    if (fault === "position") value.message.position.position += 1n;
    if (fault === "metadata group") value.record.record.metadata.group = bytes(32, 99);
    const { client, native } = fixture(value);
    await render(<MessageDetails {...props} client={client} />);
    expect(screen.getByText("The saved message did not match this detail view.")).toBeTruthy();
    expect(screen.queryByText("A saved garden message")).toBeNull();
    expect(native.deliveryTargets).not.toHaveBeenCalled();
  },
);

test.each(["client", "incarnation", "message"])("an old %s read cannot appear in the replacement selection", async (change) => {
  const oldMessage = deferred<groups.GroupTextMessage | undefined>(), oldRecord = deferred<groups.GroupStoredRecord | undefined>();
  const old = fixture(); old.native.message.mockReturnValueOnce(oldMessage.promise); old.native.record.mockReturnValueOnce(oldRecord.promise);
  const view = await render(<MessageDetails {...props} client={old.client} />);
  const replacement = snapshot(change === "incarnation" ? bytes(24, 77) : incarnation);
  replacement.message.text = "The replacement message";
  if (change === "message") { replacement.message.id = bytes(32, 77); replacement.record.record.metadata.record = replacement.message.id; }
  const next = fixture(replacement);
  const client = change === "client" ? next.client : old.client;
  if (change !== "client") {
    old.native.message.mockResolvedValue(replacement.message); old.native.record.mockResolvedValue(replacement.record);
  }
  const nextProps = { ...props, client, incarnation: hex(replacement.record.position.incarnation), messageId: hex(replacement.message.id) };
  await view.rerender(<MessageDetails {...nextProps} />);
  expect((old.native.message.mock.calls[0]?.[4] as { signal: AbortSignal }).signal.aborted).toBe(true);
  await act(async () => { oldMessage.resolve(old.message); oldRecord.resolve(old.record); });
  expect(screen.getByText("The replacement message")).toBeTruthy();
  expect(screen.queryByText("A saved garden message")).toBeNull();
});

test("a historical incarnation is queried directly even when the same message id exists in the current one", async () => {
  const historical = bytes(24, 30), oldValue = snapshot(historical), current = snapshot();
  oldValue.message.text = "Historical copy"; current.message.text = "Current copy";
  const { client, native } = fixture(oldValue);
  native.message.mockImplementation(async (_group, selected) => hex(selected as ArrayBuffer) === hex(historical) ? oldValue.message : current.message);
  native.record.mockImplementation(async (_group, selected) => hex(selected as ArrayBuffer) === hex(historical) ? oldValue.record : current.record);
  await render(<MessageDetails {...props} incarnation={hex(historical)} client={client} />);
  expect(screen.getByText("Historical copy")).toBeTruthy();
  expect(screen.queryByText("Current copy")).toBeNull();
  expect(native.deliveryTargets.mock.calls[0]?.[1]).toEqual(historical);
  expect(native.messages).not.toHaveBeenCalled();
});

test("known facts remain visible during a silent point refresh and only change after matching reads complete", async () => {
  const delayedMessage = deferred<groups.GroupTextMessage | undefined>(), delayedRecord = deferred<groups.GroupStoredRecord | undefined>();
  const { client, native, message, record } = fixture();
  const view = await render(<MessageDetails {...props} client={client} />);
  native.message.mockReturnValueOnce(delayedMessage.promise); native.record.mockReturnValueOnce(delayedRecord.promise);
  await view.rerender(<MessageDetails {...props} client={client} revision={2} />);
  expect(screen.getByText("A saved garden message")).toBeTruthy();
  expect(screen.getByText("0 of 1 recipient devices confirmed saving this message.")).toBeTruthy();
  expect(screen.queryByLabelText("Loading message details")).toBeNull();
  await act(async () => { delayedMessage.resolve({ ...message, receivedCount: 1n }); });
  expect(screen.getByText("0 of 1 recipient devices confirmed saving this message.")).toBeTruthy();
  await act(async () => { delayedRecord.resolve({ ...record, progress: { ...record.progress, summary: { ...record.progress.summary, received: 1n, pending: 0n } } }); });
  expect(screen.getByText("1 of 1 recipient devices confirmed saving this message.")).toBeTruthy();
});

test("a point-read error retries only read APIs", async () => {
  const { client, native } = fixture();
  native.message.mockRejectedValueOnce(new Error("Storage is unavailable"));
  await render(<MessageDetails {...props} client={client} />);
  expect(screen.getByText("Storage is unavailable")).toBeTruthy();
  await press("Retry message details");
  expect(screen.getByText("A saved garden message")).toBeTruthy();
  expect(native.message).toHaveBeenCalledTimes(2);
  expect(native.execute).not.toHaveBeenCalled();
  expect(native.publishRequest).not.toHaveBeenCalled();
  expect(native.messages).not.toHaveBeenCalled();
});

test.each(["blur", "unmount"])("%s aborts point reads and prevents late recipient/event queries", async (end) => {
  const delayedMessage = deferred<groups.GroupTextMessage | undefined>(), delayedRecord = deferred<groups.GroupStoredRecord | undefined>();
  const { client, native, message, record } = fixture();
  native.message.mockReturnValueOnce(delayedMessage.promise); native.record.mockReturnValueOnce(delayedRecord.promise);
  const view = await render(<MessageDetails {...props} client={client} />);
  if (end === "unmount") await view.unmount();
  else await view.rerender(<MessageDetails {...props} client={client} active={false} />);
  expect((native.message.mock.calls[0]?.[4] as { signal: AbortSignal }).signal.aborted).toBe(true);
  await act(async () => { delayedMessage.resolve(message); delayedRecord.resolve(record); });
  expect(native.deliveryTargets).not.toHaveBeenCalled();
  expect(native.changes).not.toHaveBeenCalled();
  if (end === "blur") expect(screen.queryByText("A saved garden message")).toBeNull();
});
