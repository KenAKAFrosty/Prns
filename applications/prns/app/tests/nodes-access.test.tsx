import { act, fireEvent, render } from "@testing-library/react-native";
import { prns_host_uniffi as rc, prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { Access } from "../src/nodes/Access";
import { Nodes } from "../src/nodes/Nodes";

const mockOperations = jest.fn(() => []);
jest.mock("../src/nodes/Manage", () => ({ Manage: () => null }));
jest.mock("../src/nodes/Pairing", () => ({ Pairing: () => null }));
jest.mock("../src/nodes/Operations", () => ({ Operations: () => null }));

jest.mock("@prns/app-bindings", () => {
  const tags = new Proxy({}, { get: (_, key) => key });
  return {
    prns_chat_app: { appRemoteControlOperations: () => ({ operations: () => mockOperations() }) },
    prns_host_uniffi: {
    RemoteControlTargetInventorySettlement_Tags: tags,
    RemoteControlControllerIdentitySettlement_Tags: tags,
    RemoteControlEncodeRemoteControlControllerCardSettlement_Tags: tags,
    RemoteControlDecodeRemoteControlTargetAccessCardSettlement_Tags: tags,
    RemoteControlSaveRemoteControlTargetAccessCardSettlement_Tags: tags,
    RemoteControlControllerAuthority: { 0: "Operator", Operator: 0 },
    RemoteControlRequestKind: { 0: "Describe", Describe: 0 },
    remoteControlControllerIdentityHash: (value: { publicKeys: Uint8Array }) => ({ value: value.publicKeys.slice(0, 16) }),
    remoteControlTargetIdentityHash: (value: { publicKeys: Uint8Array }) => ({ value: value.publicKeys.slice(0, 16) }),
    encodeRemoteControlControllerCard: jest.fn(() => ({ tag: "Completed", inner: { value: "controller-card" } })),
    decodeRemoteControlTargetAccessCard: jest.fn(),
  }};
});
const controller = { publicKeys: new Uint8Array(64).fill(4) };
const card = { recipient: controller, access: { target: { publicKeys: new Uint8Array(64).fill(9) }, authority: 0, permittedRequests: { kinds: [0] } } };
function fixture() {
  const save = jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: { tag: "Inserted" } } });
  const client = { remoteControlControllerIdentity: jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: controller } }), saveRemoteControlTargetAccessCard: save } as unknown as rc.HostClientHandleLike;
  return { client, save };
}
async function review(screen: Awaited<ReturnType<typeof render>>, value = "target-card") {
  await fireEvent.changeText(screen.getByLabelText("Node access card"), value);
  await act(async () => await fireEvent.press(screen.getByText("Review access card")));
}
beforeEach(() => { mockOperations.mockImplementation(() => []); (rc.decodeRemoteControlTargetAccessCard as jest.Mock).mockReturnValue({ tag: "Completed", inner: { value: card } }); });

test("valid card requires explicit trust and saves the exact canonical card", async () => {
  const f = fixture(); const changed = jest.fn();
  const screen = await render(<Access client={f.client} active changed={changed} />);
  await review(screen);
  await act(async () => await fireEvent.press(screen.getByText("Save reviewed node access")));
  expect(f.save).not.toHaveBeenCalled();
  await fireEvent(screen.getByLabelText("I trust the source and reviewed the exact access"), "valueChange", true);
  await act(async () => await fireEvent.press(screen.getByText("Save reviewed node access")));
  expect(f.save).toHaveBeenCalledWith(card); expect(changed).toHaveBeenCalledTimes(1);
});

test("wrong recipient cannot be saved and changing card text clears prior review", async () => {
  const f = fixture();
  const screen = await render(<Access client={f.client} active changed={() => {}} />);
  await review(screen);
  await fireEvent(screen.getByLabelText("I trust the source and reviewed the exact access"), "valueChange", true);
  await fireEvent.changeText(screen.getByLabelText("Node access card"), "replacement");
  expect(screen.queryByText("Save reviewed node access")).toBeNull();
  (rc.decodeRemoteControlTargetAccessCard as jest.Mock).mockReturnValue({ tag: "Completed", inner: { value: { ...card, recipient: { publicKeys: new Uint8Array(64).fill(5) } } } });
  await review(screen, "wrong-recipient");
  expect(screen.getByText(/belongs to a different controller/)).toBeTruthy();
  await act(async () => await fireEvent.press(screen.getByText("Save reviewed node access")));
  expect(f.save).not.toHaveBeenCalled();
});

test("malformed card fails before persistence and a conflicting import is not retried", async () => {
  const f = fixture();
  const screen = await render(<Access client={f.client} active changed={() => {}} />);
  (rc.decodeRemoteControlTargetAccessCard as jest.Mock).mockReturnValue({ tag: "Failed", inner: { failure: { tag: "InvalidTextEnvelope" } } });
  await review(screen, "bad");
  expect(f.save).not.toHaveBeenCalled(); expect(screen.queryByText("Save reviewed node access")).toBeNull();
  (rc.decodeRemoteControlTargetAccessCard as jest.Mock).mockReturnValue({ tag: "Completed", inner: { value: card } });
  f.save.mockResolvedValue({ tag: "Failed", inner: { failure: { tag: "Conflict" } } });
  await review(screen);
  await fireEvent(screen.getByLabelText("I trust the source and reviewed the exact access"), "valueChange", true);
  await act(async () => await fireEvent.press(screen.getByText("Save reviewed node access")));
  expect(f.save).toHaveBeenCalledTimes(1); expect(screen.getByText(/Existing access is not replaced automatically/)).toBeTruthy();
});


test("inactive access cannot save even through its direct button callback", async () => {
  const buttons = jest.spyOn(require("../src/ui/primitives") as typeof import("../src/ui/primitives"), "Button");
  const f = fixture(); const screen = await render(<Access client={f.client} active changed={() => {}} />);
  await review(screen);
  await fireEvent(screen.getByLabelText("I trust the source and reviewed the exact access"), "valueChange", true);
  await screen.rerender(<Access client={f.client} active={false} changed={() => {}} />);
  // Bypass React Native disabled handling to exercise the action's own guard.
  await act(async () => (buttons.mock.calls.findLast(([props]) => props.title === "Save reviewed node access")![0].onPress as () => void)());
  expect(f.save).not.toHaveBeenCalled();
});

test("replacement client clears controller trust and suppresses a prior save completion", async () => {
  const old = fixture(); const next = fixture(); const changed = jest.fn();
  let finish!: (value: unknown) => void;
  old.save.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  (next.client.remoteControlControllerIdentity as jest.Mock).mockReturnValue(new Promise(() => {}));
  const screen = await render(<Access client={old.client} active changed={changed} />);
  await review(screen);
  await fireEvent(screen.getByLabelText("I trust the source and reviewed the exact access"), "valueChange", true);
  await act(async () => await fireEvent.press(screen.getByText("Save reviewed node access")));
  await screen.rerender(<Access client={next.client} active changed={changed} />);
  expect(screen.queryByText("controller-card")).toBeNull();
  expect(screen.getByLabelText("I trust the source and reviewed the exact access").props.value).toBe(false);
  await act(async () => finish({ tag: "Completed", inner: { value: { tag: "Inserted" } } }));
  expect(changed).not.toHaveBeenCalled();
  expect(screen.getByLabelText("Node access card").props.value).toBe("target-card");
  expect(next.save).not.toHaveBeenCalled();
});


afterEach(() => { jest.useRealTimers(); });

test("Access avoids hidden reads while Changes keeps retained operations current", async () => {
  jest.useFakeTimers();
  const f = fixture();
  const inventory = jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: { targets: [] } } });
  const client = { ...f.client, remoteControlTargetInventory: inventory };
  const owner = { host: () => client } as unknown as owners.ServiceRuntimeLike;
  const screen = await render(<Nodes owner={owner} active section="Access" />);
  expect(screen.getByText("controller-card")).toBeTruthy();
  expect(inventory).not.toHaveBeenCalled();
  const snapshots = mockOperations.mock.calls.length;
  await act(async () => jest.advanceTimersByTime(4000));
  expect(inventory).not.toHaveBeenCalled();
  expect(mockOperations.mock.calls.length).toBe(snapshots);
  await screen.rerender(<Nodes owner={owner} active section="Changes" />);
  const changesSnapshots = mockOperations.mock.calls.length;
  await act(async () => jest.advanceTimersByTime(2000));
  expect(inventory).not.toHaveBeenCalled();
  expect(mockOperations.mock.calls.length).toBeGreaterThan(changesSnapshots);
  await screen.unmount();
});

test("a controller read can retry after an already admitted inventory releases its lane", async () => {
  jest.useFakeTimers();
  const f = fixture();
  let inventoryPending = true;
  let settleInventory!: (value: unknown) => void;
  const inventory = jest.fn(() => new Promise((resolve) => { settleInventory = resolve; }));
  const readController = jest.fn(() => Promise.resolve(inventoryPending
    ? { tag: "Failed", inner: { failure: { tag: "Busy" } } }
    : { tag: "Completed", inner: { value: controller } }));
  const client = { ...f.client, remoteControlTargetInventory: inventory, remoteControlControllerIdentity: readController };
  const owner = { host: () => client } as unknown as owners.ServiceRuntimeLike;
  const screen = await render(<Nodes owner={owner} active section="Nodes" />);
  expect(inventory).toHaveBeenCalledTimes(1);
  await screen.rerender(<Nodes owner={owner} active section="Access" />);
  expect(screen.queryByText("controller-card")).toBeNull();
  expect(screen.getByText("Controller card could not be loaded: Busy")).toBeTruthy();
  // Aborting the old UI waiter does not release a native operation already admitted.
  await act(async () => { inventoryPending = false; settleInventory({ tag: "Completed", inner: { value: { targets: [] } } }); });
  await act(async () => jest.advanceTimersByTime(4000));
  expect(inventory).toHaveBeenCalledTimes(1);
  expect(readController).toHaveBeenCalledTimes(1);
  await act(async () => await fireEvent.press(screen.getByText("Retry loading controller card")));
  expect(readController).toHaveBeenCalledTimes(2);
  expect(screen.getByText("controller-card")).toBeTruthy();
  expect(screen.queryByText(/Controller card could not be loaded/)).toBeNull();
  expect(f.save).not.toHaveBeenCalled();
  await screen.unmount();
});


test("inventory success cannot clear an operations error and hidden errors stay with their section", async () => {
  jest.useFakeTimers();
  const f = fixture();
  mockOperations.mockImplementation(() => { throw new Error("Saved changes unavailable"); });
  const inventory = jest.fn()
    .mockResolvedValueOnce({ tag: "Failed", inner: { failure: { tag: "Busy" } } })
    .mockResolvedValue({ tag: "Completed", inner: { value: { targets: [] } } });
  const client = { ...f.client, remoteControlTargetInventory: inventory };
  const owner = { host: () => client } as unknown as owners.ServiceRuntimeLike;
  const screen = await render(<Nodes owner={owner} active section="Nodes" />);
  expect(screen.getByText("Busy")).toBeTruthy();
  expect(screen.getByText("Saved changes unavailable")).toBeTruthy();
  await act(async () => jest.advanceTimersByTime(2000));
  expect(screen.queryByText("Busy")).toBeNull();
  expect(screen.getByText("Saved changes unavailable")).toBeTruthy();
  inventory.mockResolvedValue({ tag: "Failed", inner: { failure: { tag: "Busy" } } });
  await act(async () => jest.advanceTimersByTime(2000));
  expect(screen.getByText("Busy")).toBeTruthy();
  await screen.rerender(<Nodes owner={owner} active section="Access" />);
  expect(screen.getByText("controller-card")).toBeTruthy();
  expect(screen.queryByText("Busy")).toBeNull();
  expect(screen.queryByText("Saved changes unavailable")).toBeNull();
  await screen.rerender(<Nodes owner={owner} active section="Changes" />);
  expect(screen.getByText("Saved changes unavailable")).toBeTruthy();
  expect(screen.queryByText("Busy")).toBeNull();
  await screen.unmount();
});

afterEach(() => { jest.restoreAllMocks(); });
