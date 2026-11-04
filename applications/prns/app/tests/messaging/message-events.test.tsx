import "./native-mock";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import { prns_groups as groups, prns_groups_service as service } from "@prns/app-bindings";
import { MessageEvents } from "../../src/messaging/MessageEvents";
import { hex } from "../../src/bytes";

const buffer = (size: number, fill: number) => new Uint8Array(size).fill(fill).buffer;
const group = buffer(32, 1), incarnation = buffer(24, 2), messageId = buffer(32, 3), otherId = buffer(32, 4);
const endpoint = { id: buffer(16, 5), incarnation: 1n };
const token = (position: bigint, selected = incarnation) => ({ group, incarnation: selected, position });
const receipt = (record = messageId) => ({ group, record, target: endpoint });
const change = (cursor: bigint, kind: service.GroupChangeKind): service.GroupChange => ({ cursor, kind, operation: buffer(16, 0) });
function page(next: bigint, head = next, changes: service.GroupChange[] = [], historyPruned = false): groups.GroupRecordChanges {
  return { changes, next: token(next), head: token(head), historyPruned };
}
const props = { group: hex(group), incarnation: hex(incarnation), messageId: hex(messageId), position: 10n, active: true, revision: 1 };
const asClient = (changes: jest.Mock) => ({ changes }) as unknown as groups.ClientLike;
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
afterEach(cleanup);

test("filters exact message events and never associates rejected envelopes or unrelated receipts", async () => {
  const changes = jest.fn().mockResolvedValue(page(16n, 16n, [
    change(10n, new service.GroupChangeKind.Published(messageId)),
    change(11n, new service.GroupChangeKind.Received(messageId)),
    change(12n, new service.GroupChangeKind.ReceiptQueued(receipt())),
    change(13n, new service.GroupChangeKind.Receipt(receipt())),
    change(14n, new service.GroupChangeKind.Published(otherId)),
    change(15n, new service.GroupChangeKind.Receipt(receipt(otherId))),
    change(16n, new service.GroupChangeKind.Rejected({ envelope: messageId, reason: service.GroupRejectReason.InvalidPacket })),
  ]));
  const deviceName = jest.fn(() => "Garden tablet");
  await render(<MessageEvents {...props} client={asClient(changes)} deviceName={deviceName} />);
  expect(changes.mock.calls[0]?.slice(0, 3)).toEqual([group, token(9n), 50]);
  expect(screen.getByText("Saved on this phone")).toBeTruthy();
  expect(screen.getByText("Received and saved on this phone")).toBeTruthy();
  expect(screen.getByText("Receipt queued for sender")).toBeTruthy();
  expect(screen.getByText("Receipt confirmed from Garden tablet")).toBeTruthy();
  expect(screen.getAllByText(/Receipt confirmed/)).toHaveLength(1);
  expect(deviceName).toHaveBeenCalledWith(endpoint);
  expect(screen.queryByText(/Rejected/)).toBeNull();
  await fireEvent.press(screen.getByLabelText("About recorded activity"));
  expect(screen.getByText(/no recorded event times/)).toBeTruthy();
});

test("continues across an empty pruned page and retains its incomplete-history note", async () => {
  const changes = jest.fn().mockResolvedValueOnce(page(20n, 21n, [], true))
    .mockResolvedValueOnce(page(21n, 21n, [change(21n, new service.GroupChangeKind.Receipt(receipt()))]))
    .mockResolvedValue(page(21n));
  const client = asClient(changes);
  const view = await render(<MessageEvents {...props} client={client} />);
  expect(changes.mock.calls[1]?.[1]).toEqual(token(20n));
  expect(screen.getByText("Receipt confirmed from device 05050505")).toBeTruthy();
  await view.rerender(<MessageEvents {...props} client={client} revision={2} />);
  expect(changes.mock.calls[2]?.[1]).toEqual(token(21n));
  expect(screen.getByText(/earlier activity is no longer retained/)).toBeTruthy();
  expect(screen.getAllByText(/Receipt confirmed/)).toHaveLength(1);
});

test("limits each load to four fifty-row pages and coalesces double presses", async () => {
  const buttons = jest.spyOn(require("../../src/ui/primitives") as typeof import("../../src/ui/primitives"), "Button");
  const pending = deferred<groups.GroupRecordChanges>();
  const changes = jest.fn().mockImplementation((_group, after: groups.GroupChangeToken) => {
    if (after.position === 209n) return pending.promise;
    const events = Array.from({ length: 50 }, (_, index) => change(after.position + BigInt(index + 1), new service.GroupChangeKind.Published(otherId)));
    return Promise.resolve(page(after.position + 50n, 250n, events));
  });
  await render(<MessageEvents {...props} client={asClient(changes)} />);
  expect(changes).toHaveBeenCalledTimes(4);
  expect(changes.mock.calls.map((call) => call[1].position)).toEqual([9n, 59n, 109n, 159n]);
  expect(changes.mock.calls.every((call) => call[2] === 50)).toBe(true);
  const more = (buttons.mock.calls.findLast(([props]) => props.title === "Load more events")![0].onPress as () => void);
  await act(async () => { more(); more(); });
  expect(changes).toHaveBeenCalledTimes(5);
  expect(screen.getByText("Load more events")).toBeDisabled();
  await act(async () => { pending.resolve(page(250n, 250n, [change(250n, new service.GroupChangeKind.Received(messageId))])); });
  expect(screen.getByText("Received and saved on this phone")).toBeTruthy();
  expect(screen.queryByText("Load more events")).toBeNull();
});

test("background revision reads keep known rows visible and retry failures from the same cursor", async () => {
  const pending = deferred<groups.GroupRecordChanges>();
  const changes = jest.fn().mockResolvedValueOnce(page(10n, 10n, [change(10n, new service.GroupChangeKind.Published(messageId))]))
    .mockReturnValueOnce(pending.promise).mockResolvedValueOnce(page(11n, 11n, [change(11n, new service.GroupChangeKind.Receipt(receipt()))]));
  const client = asClient(changes);
  const view = await render(<MessageEvents {...props} client={client} />);
  await view.rerender(<MessageEvents {...props} client={client} revision={2} />);
  expect(screen.getByText("Saved on this phone")).toBeTruthy();
  expect(screen.queryByLabelText("Loading recorded activity")).toBeNull();
  await act(async () => { pending.reject(new Error("read failed")); });
  expect(screen.getByText("Saved on this phone")).toBeTruthy();
  await act(async () => { await fireEvent.press(screen.getByText("Retry recorded activity")); });
  expect(changes.mock.calls[2]?.[1]).toEqual(token(10n));
  expect(screen.getByText("Receipt confirmed from device 05050505")).toBeTruthy();
});

test.each(["client", "incarnation", "message"])("late replies cannot cross a changed %s selection", async (field) => {
  const late = deferred<groups.GroupRecordChanges>();
  const changes = jest.fn().mockReturnValueOnce(late.promise).mockImplementation((_group, after: groups.GroupChangeToken) =>
    Promise.resolve({ ...page(10n), next: { ...after, position: 10n }, head: { ...after, position: 10n } }));
  const client = asClient(changes);
  const view = await render(<MessageEvents {...props} client={client} />);
  const next = { ...props, client };
  if (field === "client") next.client = asClient(changes);
  if (field === "incarnation") next.incarnation = hex(buffer(24, 8));
  if (field === "message") next.messageId = hex(otherId);
  await view.rerender(<MessageEvents {...next} />);
  expect(changes.mock.calls[0]?.[3].signal.aborted).toBe(true);
  await act(async () => { late.resolve(page(10n, 10n, [change(10n, new service.GroupChangeKind.Published(messageId))])); });
  expect(screen.queryByText("Saved on this phone")).toBeNull();
  expect(screen.getByText("No retained events for this message have been found.")).toBeTruthy();
});

test("blur discards an unfinished read, focus resumes retained rows, and unmount aborts", async () => {
  const pending = deferred<groups.GroupRecordChanges>(), afterFocus = deferred<groups.GroupRecordChanges>();
  const changes = jest.fn().mockResolvedValueOnce(page(10n, 10n, [change(10n, new service.GroupChangeKind.Published(messageId))]))
    .mockReturnValueOnce(pending.promise).mockReturnValueOnce(afterFocus.promise);
  const client = asClient(changes);
  const view = await render(<MessageEvents {...props} client={client} />);
  await view.rerender(<MessageEvents {...props} client={client} revision={2} />);
  await view.rerender(<MessageEvents {...props} client={client} revision={2} active={false} />);
  expect(changes.mock.calls[1]?.[3].signal.aborted).toBe(true);
  await act(async () => { pending.resolve(page(11n, 11n, [change(11n, new service.GroupChangeKind.Receipt(receipt()))])); });
  await view.rerender(<MessageEvents {...props} client={client} revision={2} />);
  expect(changes.mock.calls[2]?.[1]).toEqual(token(10n));
  expect(screen.getByText("Saved on this phone")).toBeTruthy();
  expect(screen.queryByText(/Receipt confirmed/)).toBeNull();
  await view.unmount();
  expect(changes.mock.calls[2]?.[3].signal.aborted).toBe(true);
  await act(async () => { afterFocus.resolve(page(99n)); });
  expect(changes).toHaveBeenCalledTimes(3);
});

test.each(["foreign group", "foreign incarnation", "backward next", "head before next", "stalled continuation", "unordered events", "event past next", "oversized page", "foreign receipt"])(
  "rejects %s without attaching events or continuing", async (fault) => {
    const value = page(11n, 11n, [change(10n, new service.GroupChangeKind.Published(messageId))]);
    if (fault === "foreign group") value.next.group = buffer(32, 8);
    if (fault === "foreign incarnation") value.head.incarnation = buffer(24, 8);
    if (fault === "backward next") value.next.position = 8n;
    if (fault === "head before next") value.head.position = 10n;
    if (fault === "stalled continuation") value.next.position = 9n;
    if (fault === "unordered events") value.changes.push(change(10n, new service.GroupChangeKind.Received(messageId)));
    if (fault === "event past next") value.changes[0]!.cursor = 12n;
    if (fault === "oversized page") value.changes = Array.from({ length: 51 }, () => value.changes[0]!);
    if (fault === "foreign receipt") value.changes = [change(10n, new service.GroupChangeKind.Receipt({ ...receipt(), group: buffer(32, 8) }))];
    const changes = jest.fn().mockResolvedValue(value);
    await render(<MessageEvents {...props} client={asClient(changes)} />);
    expect(changes).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("Saved on this phone")).toBeNull();
    expect(screen.getByText("Retry recorded activity")).toBeTruthy();
  },
);

test("zero insertion position starts at zero and invalid selection never reaches native", async () => {
  const changes = jest.fn().mockResolvedValue(page(0n));
  const client = asClient(changes);
  const view = await render(<MessageEvents {...props} position={0n} client={client} />);
  expect(changes.mock.calls[0]?.[1]).toEqual(token(0n));
  await view.rerender(<MessageEvents {...props} group="bad" client={client} />);
  expect(changes).toHaveBeenCalledTimes(1);
  expect(screen.getByText("Retry recorded activity")).toBeTruthy();
});


test("a retained load-more callback cannot read after unmount", async () => {
  const buttons = jest.spyOn(require("../../src/ui/primitives") as typeof import("../../src/ui/primitives"), "Button");
  const changes = jest.fn().mockImplementation((_group, after: groups.GroupChangeToken) => Promise.resolve(page(after.position + 1n, 100n)));
  const view = await render(<MessageEvents {...props} client={asClient(changes)} />);
  const loadMore = (buttons.mock.calls.findLast(([props]) => props.title === "Load more events")![0].onPress as () => void);
  expect(changes).toHaveBeenCalledTimes(4);
  await view.unmount();
  await act(async () => { loadMore(); });
  expect(changes).toHaveBeenCalledTimes(4);
});

test("a backward head on refresh cannot replace known events", async () => {
  const changes = jest.fn().mockResolvedValueOnce(page(10n, 10n, [change(10n, new service.GroupChangeKind.Published(messageId))]))
    .mockResolvedValueOnce(page(9n));
  const client = asClient(changes);
  const view = await render(<MessageEvents {...props} client={client} />);
  await view.rerender(<MessageEvents {...props} client={client} revision={2} />);
  expect(screen.getByText("Saved on this phone")).toBeTruthy();
  expect(screen.getByText("Retry recorded activity")).toBeTruthy();
});

afterEach(() => { jest.restoreAllMocks(); });
