import { act, fireEvent, render } from "@testing-library/react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { Pairing } from "../src/nodes/Pairing";

jest.mock("@prns/app-bindings", () => {
  const tags = new Proxy({}, { get: (_, key) => key });
  return { prns_host_uniffi: {
    RemoteControlControllerPairingSubject: {
      AwaitingOffer: class { tag = "AwaitingOffer"; inner: unknown; constructor(value: unknown) { this.inner = value; } },
      Attempt: class { tag = "Attempt"; inner: unknown; constructor(value: unknown) { this.inner = value; } },
    },
    RemoteControlControllerPairingFailureReason: { LinkClosed: 0, RequestFailed: 1, PersistenceFailed: 2 },
    RemoteControlControllerPairingSnapshot_Tags: tags, RemoteControlControllerPairingSnapshotSettlement_Tags: tags, RemoteControlPairingDiscoverySettlement_Tags: tags, RemoteControlInitiateRemoteControlControllerPairingSettlement_Tags: tags,
    RemoteControlControllerAuthority: { 0: "Operator", Operator: 0 }, RemoteControlRequestKind: { 0: "Describe", Describe: 0 },
    remoteControlControllerIdentityHash: (value: { publicKeys: Uint8Array }) => ({ value: value.publicKeys.slice(0, 16) }),
    remoteControlTargetIdentityHash: (value: { publicKeys: Uint8Array }) => ({ value: value.publicKeys.slice(0, 16) }),
    remoteControlPairingConfirmationCode: () => "000042",
  }};
});
function confirmation(id: number) {
  return { confirmation: { attemptId: { value: new Uint8Array(32).fill(id) }, context: { endpoint: { destinationHash: { value: new Uint8Array(16).fill(3) } }, linkId: { value: new Uint8Array(16).fill(9) } }, controller: { publicKeys: new Uint8Array(64).fill(4) }, target: { publicKeys: new Uint8Array(64).fill(5) }, permissions: { authority: 0, permittedRequests: { kinds: [0] } } } };
}
function status(current: unknown, terminal?: unknown) { return { tag: "Completed", inner: { value: { current, terminal } } }; }
function snapshot(id: number) { return status({ tag: "AwaitingApproval", inner: { confirmation: confirmation(id) } }); }
const target = { value: new Uint8Array(16).fill(5) };
function terminal(outcome: unknown, id = 1) { const { context, attemptId } = confirmation(id).confirmation;
  return { subject: { tag: "Attempt", inner: { context, attemptId } }, outcome }; }
function paired(id = 1) { return status({ tag: "Idle" }, terminal({ tag: "Paired", inner: { target } }, id)); }
function fixture() {
  const poll = jest.fn().mockResolvedValue(snapshot(1));
  const approve = jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: {} } });
  const reject = jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: {} } });
  const discovery = jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: { generation: 1n, observedAt: { value: 100n }, capacity: 64, evicted: 0n, observations: [] } } });
  const initiate = jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: {} } });
  const inventory = jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: { targets: [target] } } });
  const cancel = jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: {} } });
  const client = { remoteControlTargetInventory: inventory, cancelRemoteControlControllerPairing: cancel, remoteControlPairingDiscovery: discovery, initiateRemoteControlControllerPairing: initiate, remoteControlControllerPairingStatus: poll, approveRemoteControlControllerPairing: approve, rejectRemoteControlControllerPairing: reject } as unknown as rc.HostClientHandleLike;
  return { client, poll, approve, reject, discovery, initiate, inventory, cancel };
}
beforeEach(() => jest.useFakeTimers());
afterEach(() => jest.useRealTimers());

test("approval uses the exact displayed attempt only after explicit comparison", async () => {
  const f = fixture(); const changed = jest.fn();
  const screen = await render(<Pairing client={f.client} active changed={changed} />);
  expect(screen.getByText("000042")).toBeTruthy();
  await act(async () => await fireEvent.press(screen.getByText("Approve this pairing attempt")));
  expect(f.approve).not.toHaveBeenCalled();
  await fireEvent(screen.getByLabelText("Codes match and I approve these permissions"), "valueChange", true);
  await act(async () => await fireEvent.press(screen.getByText("Approve this pairing attempt")));
  expect(f.approve).toHaveBeenCalledWith({ attemptId: confirmation(1).confirmation.attemptId });
  expect(changed).not.toHaveBeenCalled(); // Approval alone is not paired or durably saved.
});

test("a replaced canonical attempt cannot inherit a prior code comparison", async () => {
  const f = fixture(); const screen = await render(<Pairing client={f.client} active changed={() => {}} />);
  await fireEvent(screen.getByLabelText("Codes match and I approve these permissions"), "valueChange", true);
  f.poll.mockResolvedValue(snapshot(2));
  await act(async () => { jest.advanceTimersByTime(1500); });
  await act(async () => await fireEvent.press(screen.getByText("Approve this pairing attempt")));
  expect(f.approve).not.toHaveBeenCalled();
  await act(async () => await fireEvent.press(screen.getByText("Reject this pairing attempt")));
  expect(f.reject).toHaveBeenCalledWith({ attemptId: confirmation(2).confirmation.attemptId });
});

test("inactive pairing stops polling and hides any formerly actionable snapshot", async () => {
  const f = fixture(); const screen = await render(<Pairing client={f.client} active changed={() => {}} />);
  const count = f.poll.mock.calls.length;
  await screen.rerender(<Pairing client={f.client} active={false} changed={() => {}} />);
  await act(async () => { jest.advanceTimersByTime(10000); });
  expect(f.poll).toHaveBeenCalledTimes(count);
  expect(screen.queryByText("Approve this pairing attempt")).toBeNull();
});


test("initiation uses the selected native endpoint/deadline and rejects an observed expired window", async () => {
  const f = fixture();
  f.poll.mockResolvedValue(status({ tag: "Idle" }));
  const entry = { endpoint: { destinationHash: { value: new Uint8Array(16).fill(6) } }, observedAt: { value: 80n }, expiresAt: { value: 500n }, hops: { value: 1 }, sourceInterface: { value: new Uint8Array(16) }, publicAppData: { value: new Uint8Array() } };
  f.discovery.mockResolvedValue({ tag: "Completed", inner: { value: { generation: 2n, observedAt: { value: 100n }, capacity: 64, evicted: 3n, observations: [entry] } } });
  const screen = await render(<Pairing client={f.client} active changed={() => {}} />);
  await act(async () => await fireEvent.press(screen.getByText("Choose pairing endpoint 060606060606")));
  await fireEvent.changeText(screen.getByLabelText("Pairing invitation code"), "0000002a");
  await act(async () => await fireEvent.press(screen.getByText("Begin pairing with selected endpoint")));
  expect(f.initiate).toHaveBeenCalledWith({ endpoint: entry.endpoint, expiresAt: entry.expiresAt, invitationCode: { value: 42 } });
  f.discovery.mockResolvedValue({ tag: "Completed", inner: { value: { generation: 3n, observedAt: { value: 501n }, capacity: 64, evicted: 3n, observations: [entry] } } });
  await act(async () => { jest.advanceTimersByTime(1500); });
  await fireEvent.changeText(screen.getByLabelText("Pairing invitation code"), "0000002a");
  await act(async () => await fireEvent.press(screen.getByText("Begin pairing with selected endpoint")));
  expect(f.initiate).toHaveBeenCalledTimes(1);
});


test("client replacement hides old proof while waiting and ignores old approval completion", async () => {
  const old = fixture(); const next = fixture(); const changed = jest.fn();
  let finishApproval!: (value: unknown) => void; let finishSnapshot!: (value: unknown) => void;
  old.approve.mockImplementation(() => new Promise((resolve) => { finishApproval = resolve; }));
  next.poll.mockImplementation(() => new Promise((resolve) => { finishSnapshot = resolve; }));
  const screen = await render(<Pairing client={old.client} active changed={changed} />);
  await fireEvent(screen.getByLabelText("Codes match and I approve these permissions"), "valueChange", true);
  await act(async () => await fireEvent.press(screen.getByText("Approve this pairing attempt")));
  await screen.rerender(<Pairing client={next.client} active changed={changed} />);
  expect(screen.queryByText("Approve this pairing attempt")).toBeNull();
  expect(screen.queryByText("000042")).toBeNull();
  await act(async () => finishApproval({ tag: "Completed", inner: { value: {} } }));
  expect(changed).not.toHaveBeenCalled();
  await act(async () => finishSnapshot(snapshot(2)));
  await act(async () => await fireEvent.press(screen.getByText("Approve this pairing attempt")));
  expect(next.approve).not.toHaveBeenCalled();
  await fireEvent(screen.getByLabelText("Codes match and I approve these permissions"), "valueChange", true);
  await act(async () => await fireEvent.press(screen.getByText("Approve this pairing attempt")));
  expect(next.approve).toHaveBeenCalledWith({ attemptId: confirmation(2).confirmation.attemptId });
});


test("only atomic Paired plus matching saved inventory gives success and navigation", async () => {
  const f = fixture(); const changed = jest.fn(); const open = jest.fn();
  f.poll.mockResolvedValue(status({ tag: "Idle" }));
  const screen = await render(<Pairing client={f.client} active changed={changed} onOpenPaired={open} />);
  expect(screen.queryByText(/Paired successfully/)).toBeNull(); expect(changed).not.toHaveBeenCalled();
  f.poll.mockResolvedValue(status({ tag: "AwaitingCompletion", inner: { confirmation: confirmation(1) } }));
  await act(async () => jest.advanceTimersByTime(1500));
  expect(screen.getByText(/Approved locally/)).toBeTruthy(); expect(screen.queryByText("Cancel pairing")).toBeNull();
  expect(screen.queryByText("Reject this pairing attempt")).toBeNull();
  f.poll.mockResolvedValue(status({ tag: "Persisting", inner: { ...confirmation(1).confirmation, authority: 0, permittedRequests: { kinds: [0] } } }));
  await act(async () => jest.advanceTimersByTime(1500));
  expect(screen.getByText(/Saving completed access/)).toBeTruthy(); expect(screen.queryByText(/Paired successfully/)).toBeNull();
  f.poll.mockResolvedValue(paired());
  await act(async () => jest.advanceTimersByTime(1500));
  expect(screen.getByText(/Paired successfully/)).toBeTruthy(); expect(changed).toHaveBeenCalledTimes(1);
  await fireEvent.press(screen.getByText("Manage paired node")); expect(open).toHaveBeenCalledWith(target);
  await act(async () => jest.advanceTimersByTime(1500)); expect(changed).toHaveBeenCalledTimes(1);
});

test.each([
  [{ tag: "Rejected" }, "You rejected this pairing attempt"],
  [{ tag: "Expired" }, "The pairing window expired"],
  [{ tag: "Cancelled" }, "Pairing was cancelled before approval"],
  [{ tag: "Failed", inner: { reason: 0 } }, "does not mean the node rejected"],
  [{ tag: "Failed", inner: { reason: 1 } }, "pairing request did not complete"],
  [{ tag: "Failed", inner: { reason: 2 } }, "The node may have granted access"],
])("terminal outcome %j has truthful recovery without inferred success", async (outcome, message) => {
  const f = fixture(); const changed = jest.fn();
  f.poll.mockResolvedValue(status({ tag: "Idle" }, terminal(outcome)));
  const screen = await render(<Pairing client={f.client} active changed={changed} />);
  expect(screen.getByText(new RegExp(message as string))).toBeTruthy();
  expect(screen.queryByText(/Paired successfully/)).toBeNull(); expect(changed).not.toHaveBeenCalled();
});

test("Paired with missing or unreadable inventory cannot enable Manage; an explicit status check can reconcile it", async () => {
  const f = fixture(); const changed = jest.fn(); f.poll.mockResolvedValue(paired());
  f.inventory.mockResolvedValue({ tag: "Completed", inner: { value: { targets: [] } } });
  const screen = await render(<Pairing client={f.client} active changed={changed} onOpenPaired={jest.fn()} />);
  expect(screen.getByText(/not in saved nodes/)).toBeTruthy(); expect(screen.queryByText("Manage paired node")).toBeNull();
  expect(changed).not.toHaveBeenCalled();
  f.inventory.mockRejectedValueOnce(new Error("Read unavailable"));
  await act(async () => await fireEvent.press(screen.getByText("Check pairing status")));
  expect(screen.getByText(/saved access could not be checked/)).toBeTruthy();
  f.inventory.mockResolvedValue({ tag: "Completed", inner: { value: { targets: [target] } } });
  await act(async () => await fireEvent.press(screen.getByText("Check pairing status")));
  expect(screen.getByText("Manage paired node")).toBeTruthy(); expect(changed).toHaveBeenCalledTimes(1);
});

test("cancel submits the exact displayed unapproved subject, with same-tick duplicate admission guarded", async () => {
  const buttons = jest.spyOn(require("../src/ui/primitives") as typeof import("../src/ui/primitives"), "Button");
  const f = fixture(); const { context } = confirmation(1).confirmation;
  f.poll.mockResolvedValue(status({ tag: "AwaitingOffer", inner: { context } }));
  f.cancel.mockReturnValue(new Promise(() => {}));
  const screen = await render(<Pairing client={f.client} active changed={jest.fn()} />);
  await act(async () => { const cancel = (buttons.mock.calls.findLast(([props]) => props.title === "Cancel pairing")![0].onPress as () => void); cancel(); cancel(); });
  expect(f.cancel).toHaveBeenCalledTimes(1);
  expect(f.cancel).toHaveBeenCalledWith(expect.objectContaining({ tag: "AwaitingOffer", inner: { context } }));
  const next = fixture();
  await screen.rerender(<Pairing client={next.client} active changed={jest.fn()} />);
  await act(async () => await fireEvent.press(screen.getByText("Cancel pairing")));
  expect(next.cancel).toHaveBeenCalledWith(expect.objectContaining({ tag: "Attempt", inner: { context, attemptId: confirmation(1).confirmation.attemptId } }));
});

test("terminal result for a different displayed attempt cannot override the active ceremony", async () => {
  const f = fixture(); const changed = jest.fn(); f.poll.mockResolvedValue(snapshot(2));
  const screen = await render(<Pairing client={f.client} active changed={changed} />);
  f.poll.mockResolvedValue(paired(1)); await act(async () => jest.advanceTimersByTime(1500));
  expect(screen.queryByText(/Paired successfully/)).toBeNull(); expect(changed).not.toHaveBeenCalled(); expect(f.inventory).not.toHaveBeenCalled();
});

test("late cancellation after losing focus cannot publish an error into a resumed route", async () => {
  const f = fixture(); let reject!: (cause: unknown) => void;
  f.cancel.mockImplementation(() => new Promise((_, fail) => { reject = fail; }));
  const screen = await render(<Pairing client={f.client} active changed={jest.fn()} />);
  await act(async () => await fireEvent.press(screen.getByText("Cancel pairing")));
  await screen.rerender(<Pairing client={f.client} active={false} changed={jest.fn()} />);
  await screen.rerender(<Pairing client={f.client} active changed={jest.fn()} />);
  await act(async () => reject(new Error("Old route failed")));
  expect(screen.queryByText(/Old route failed/)).toBeNull(); expect(screen.getByText("Cancel pairing")).toBeTruthy();
});

test("a failed new Begin cannot relabel the previous Paired terminal as its own success", async () => {
  const f = fixture(); f.poll.mockResolvedValue(paired());
  const entry = { endpoint: confirmation(1).confirmation.context.endpoint, expiresAt: { value: 500n }, hops: { value: 1 } };
  f.discovery.mockResolvedValue({ tag: "Completed", inner: { value: { observedAt: { value: 100n }, capacity: 64, evicted: 0n, observations: [entry] } } });
  const screen = await render(<Pairing client={f.client} active changed={jest.fn()} />);
  expect(screen.getByText(/Paired successfully/)).toBeTruthy();
  await fireEvent.press(screen.getByText("Choose pairing endpoint 030303030303"));
  await fireEvent.changeText(screen.getByLabelText("Pairing invitation code"), "0000002a");
  f.initiate.mockResolvedValue({ tag: "Failed", inner: { failure: "Unavailable" } });
  await act(async () => await fireEvent.press(screen.getByText("Begin pairing with selected endpoint")));
  expect(screen.queryByText(/Paired successfully/)).toBeNull(); expect(screen.getByText(/Pairing action was not confirmed/)).toBeTruthy();
});


test("a retained control callback from before leaving cannot cancel a later visit on the same client", async () => {
  const buttons = jest.spyOn(require("../src/ui/primitives") as typeof import("../src/ui/primitives"), "Button");
  const f = fixture();
  const screen = await render(<Pairing client={f.client} active changed={jest.fn()} />);
  const cancel = (buttons.mock.calls.findLast(([props]) => props.title === "Cancel pairing")![0].onPress as () => void);
  await screen.rerender(<Pairing client={f.client} active={false} changed={jest.fn()} />);
  await screen.rerender(<Pairing client={f.client} active changed={jest.fn()} />);
  await act(async () => cancel());
  expect(f.cancel).not.toHaveBeenCalled();
  await act(async () => await fireEvent.press(screen.getByText("Cancel pairing")));
  expect(f.cancel).toHaveBeenCalledTimes(1);
});

test("the same attempt bytes from a different physical link do not match the reviewed context", async () => {
  const f = fixture(); const changed = jest.fn();
  const screen = await render(<Pairing client={f.client} active changed={changed} />);
  const receipt = terminal({ tag: "Paired", inner: { target } });
  receipt.subject.inner.context.linkId = { value: new Uint8Array(16).fill(8) };
  f.poll.mockResolvedValue(status({ tag: "Idle" }, receipt));
  await act(async () => jest.advanceTimersByTime(1500));
  expect(screen.queryByText(/Paired successfully/)).toBeNull(); expect(changed).not.toHaveBeenCalled();
});

afterEach(() => { jest.restoreAllMocks(); });
