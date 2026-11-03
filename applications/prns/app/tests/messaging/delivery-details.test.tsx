import { act, cleanup, render, screen } from "@testing-library/react-native";
import { prns_groups_core as core } from "@prns/app-bindings";
import type { prns_groups as groups } from "@prns/app-bindings";
import { DeliveryDetails } from "../../src/messaging/DeliveryDetails";
jest.mock("@prns/app-bindings", () => ({ prns_groups: {}, prns_groups_core: {
  GroupSyncPhase: { Unknown: 0, Waiting: 1, Connecting: 2, Synchronizing: 3, RetryScheduled: 4, StorageBlocked: 5, MembershipCatchUp: 6 },
  GroupSyncIssue: { Transport: 0, StorageBusy: 1, StorageFull: 2, StorageUnavailable: 3, MembershipChanged: 4, TimeUncertain: 5, Rejected: 6, UnsupportedPeer: 7, Configuration: 8 },
} }));
const bytes = (n: number, size = 32) => new Uint8Array(size).fill(n).buffer;
const control = { head: bytes(1), era: bytes(2) };
const conversation = { group: bytes(3), control } as groups.GroupConversation;
function progress(): groups.GroupSyncProgress {
  return { running: true, observed: true, control, peers: [{
    endpoint: { id: bytes(4), incarnation: 1n }, identity: bytes(5, 16), phase: core.GroupSyncPhase.RetryScheduled,
    pending: true, retryAfterMillis: 2_500n, failures: 2, updatedAgoMillis: 5_000n, lastIssue: core.GroupSyncIssue.Transport,
  }] };
}
function client(read: jest.Mock): groups.ClientLike { return { synchronizationProgress: read } as unknown as groups.ClientLike; }
beforeEach(() => jest.useFakeTimers());
afterEach(async () => { await cleanup(); jest.useRealTimers(); });

test("visible connection activity reports recent retry evidence without claiming receipts", async () => {
  const read = jest.fn(progress);
  await render(<DeliveryDetails client={client(read)} conversation={conversation} contacts={[]} active />);
  expect(screen.getByText("Next connection attempt in about 3s")).toBeTruthy();
  expect(screen.getByText("Last connection update 5s ago")).toBeTruthy();
  expect(screen.getByText("The last connection attempt was not confirmed.")).toBeTruthy();
  expect(screen.getByText(/not a history of any one message/)).toBeTruthy();
  const previous = screen.getByText("Next connection attempt in about 3s");
  await act(async () => jest.advanceTimersByTime(2_000));
  expect(read).toHaveBeenCalledTimes(2);
  expect(screen.getByText("Next connection attempt in about 3s")).toBe(previous);
  expect(screen.queryByText("Checking connection activity…")).toBeNull();
});

test.each(["unobserved", "partial", "stopped", "old control"])("%s snapshots are truthful unknown states", async (state) => {
  const value = progress();
  if (state === "unobserved") { value.observed = false; value.control = undefined; value.peers = []; }
  if (state === "partial") value.observed = false;
  if (state === "stopped") value.running = false;
  if (state === "old control") value.control = { ...control, head: bytes(9) };
  await render(<DeliveryDetails client={client(jest.fn(() => value))} conversation={conversation} contacts={[]} active />);
  if (state === "partial") {
    expect(screen.getByText(/Only some device activity is available/)).toBeTruthy();
    expect(screen.getByText("Next connection attempt in about 3s")).toBeTruthy();
  } else {
    expect(screen.queryByText("Next connection attempt in about 3s")).toBeNull();
    expect(screen.queryByText("The last connection attempt was not confirmed.")).toBeNull();
    expect(screen.getByText(state === "stopped" ? /No live connection activity/ : /Waiting for activity from this group/)).toBeTruthy();
  }
});

test("equal decoded buffers keep the read lifetime; hidden and replaced owners retire it", async () => {
  const read = jest.fn(progress), first = client(read);
  const view = await render(<DeliveryDetails client={first} conversation={conversation} contacts={[]} active />);
  const decoded = { ...conversation, group: conversation.group.slice(0), control: { head: control.head.slice(0), era: control.era.slice(0) } };
  await view.rerender(<DeliveryDetails client={first} conversation={decoded} contacts={[]} active />);
  expect(read).toHaveBeenCalledTimes(1);
  await view.rerender(<DeliveryDetails client={first} conversation={decoded} contacts={[]} active={false} />);
  await act(async () => jest.advanceTimersByTime(6_000));
  expect(read).toHaveBeenCalledTimes(1);
  const replacementRead = jest.fn(() => ({ running: false, observed: false, control: undefined, peers: [] }));
  await view.rerender(<DeliveryDetails client={client(replacementRead)} conversation={decoded} contacts={[]} active />);
  expect(screen.queryByText("Next connection attempt in about 3s")).toBeNull();
  expect(screen.getByText(/No live connection activity/)).toBeTruthy();
  await act(async () => jest.advanceTimersByTime(2_000));
  expect(read).toHaveBeenCalledTimes(1);
  expect(replacementRead).toHaveBeenCalledTimes(2);
});

test("read errors recover automatically without turning them into peer failures", async () => {
  const read = jest.fn().mockImplementationOnce(() => { throw new Error("closed"); }).mockImplementation(progress);
  await render(<DeliveryDetails client={client(read)} conversation={conversation} contacts={[]} active />);
  expect(screen.getByText(/Connection activity is unavailable/)).toBeTruthy();
  await act(async () => jest.advanceTimersByTime(2_000));
  expect(screen.queryByText(/Connection activity is unavailable/)).toBeNull();
  expect(screen.getByText("Next connection attempt in about 3s")).toBeTruthy();
});
