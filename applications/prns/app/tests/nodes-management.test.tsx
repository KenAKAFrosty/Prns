import { act, fireEvent, render } from "@testing-library/react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { Manage } from "../src/nodes/Manage";
import { Operations } from "../src/nodes/Operations";

jest.mock("@prns/app-bindings", () => {
  const tags = new Proxy({}, { get: (_, key) => key });
  const variants = new Proxy({}, { get: (_, key) => class { tag = key; inner: unknown; constructor(value?: unknown) { this.inner = value; } } });
  const enumeration = (...names: string[]) => Object.fromEntries(names.flatMap((name, index) => [[name, index], [index, name]]));
  return { prns_host_uniffi: {
    RemoteControlRequest: variants, RemoteControlRequest_Tags: tags,
    RemoteControlResponse_Tags: tags, RemoteControlExchangeSettlement_Tags: tags,
    RemoteControlHostRemoteControlError_Tags: tags, RemoteControlConnectRemoteControlTargetError_Tags: tags,
    RemoteControlSendErrorRemoteControlEstablishLinkFailure_Tags: tags, RemoteControlEstablishLinkFailure_Tags: tags,
    RemoteControlEstablishLinkRejection: enumeration("NoRouteToDestination", "NotDirectlyReachable"),
    RemoteControlResolveRemoteControlTargetSettlement_Tags: tags,
    HostCommand: variants, CommandSettlement_Tags: tags, CommandOutcome_Tags: tags,
    RemoteControlOperationState_Tags: tags, RemoteControlOperationDestination_Tags: tags,
    RemoteControlInterfacePage: variants, RemoteControlControllerPage: variants, RemoteControlPeerPage: variants,
    RemoteControlInterfaceContinuation_Tags: tags, RemoteControlControllerContinuation_Tags: tags,
    remoteControlRequestRequiresAdministrator: () => false,
    RemoteControlRequestKind: enumeration("Describe", "SetNodeName", "DescribePower", "SetSystemPower", "SetGnssPower", "SetDisplayVisibility", "SetDisplayAutoOff", "SetEspRadioMode", "SleepRadios", "WakeRadios", "AnnounceSelf", "DescribeBuild", "DescribeNodeName", "InventoryControllers", "InspectWifiTransaction", "InventoryInterfaces"),
    RemoteControlSystemPower: enumeration("Awake", "Asleep"), RemoteControlGnssPower: enumeration("Off", "On"),
    RemoteControlDisplayVisibility: enumeration("Hidden", "Visible"), RemoteControlDisplayAutoOff: enumeration("Disabled", "Enabled"),
    RemoteControlEspRadioMode: enumeration("Bluetooth", "AccessPoint"),
    RemoteControlInterfacePower: enumeration("Off", "On"), RemoteControlInterfaceMode: enumeration("Full"),
  }};
});
const target = { value: new Uint8Array(16).fill(7) };
function completed(response: unknown) { return { tag: "Completed", inner: { response, rttMillis: 1n } }; }
function fixture() {
  const exchange = jest.fn().mockResolvedValue(completed({ tag: "Describe", inner: { value: { availableRequests: { kinds: [rc.RemoteControlRequestKind.SetNodeName] } } } }));
  const submit = jest.fn().mockReturnValue(9007199254740993n);
  const client = { remoteControlTargetExchange: exchange } as unknown as rc.HostClientHandleLike;
  const operations = { submitTarget: submit, cancel: jest.fn(), retire: jest.fn() } as unknown as rc.RemoteControlOperationsLike;
  return { exchange, submit, client, operations };
}
async function press(screen: Awaited<ReturnType<typeof render>>, text: string) {
  await act(async () => { await fireEvent.press(screen.getByText(text)); });
}

test("Describe gates controls; reviewed mutation retains its native ID and requires a new read", async () => {
  const buttons = jest.spyOn(require("../src/ui/primitives") as typeof import("../src/ui/primitives"), "Button");
  const f = fixture(); const changed = jest.fn();
  const screen = await render(<Manage {...f} target={target} records={[]} active changed={changed} />);
  expect(screen.queryByText("Review node name")).toBeNull();
  await press(screen, "Read node capabilities");
  await press(screen, "Controls");
  expect(screen.queryByText("Node power: Asleep")).toBeNull();
  await fireEvent.changeText(screen.getByLabelText("New node name"), "Garden node");
  await press(screen, "Review node name");
  expect(f.submit).not.toHaveBeenCalled();
  await act(async () => { const apply = (buttons.mock.calls.findLast(([props]) => props.title === "Apply reviewed change")![0].onPress as () => void); apply(); apply(); });
  expect(f.submit).toHaveBeenCalledTimes(1);
  expect(f.submit.mock.calls[0]).toEqual([f.client, target.value, expect.objectContaining({ tag: "SetNodeName", inner: { name: { value: "Garden node" } } })]);
  expect(screen.getByText(/Change 9007199254740993 has been submitted/)).toBeTruthy();
  expect(screen.queryByText("Review node name")).toBeNull();
  expect(changed).toHaveBeenCalledTimes(1);
});

test("an unknown outcome blocks new changes and never submits a retry", async () => {
  const f = fixture();
  const records = [{ operationId: 4n, requestKind: rc.RemoteControlRequestKind.SetNodeName, destination: { tag: "Target", inner: { identity: target } }, state: { tag: "OutcomeUnknown", inner: { failure: { tag: "Timeout" } } } }] as unknown as rc.RemoteControlOperationSnapshot[];
  const screen = await render(<Manage {...f} target={target} records={records} active changed={() => {}} />);
  await press(screen, "Read node capabilities"); await press(screen, "Controls");
  await press(screen, "Review node name");
  expect(screen.queryByText("Apply reviewed change")).toBeNull();
  expect(f.submit).not.toHaveBeenCalled();
  expect(screen.getByText(/change is still pending or its result is unknown/)).toBeTruthy();
});

test("hiding a pending read aborts it and ignores its late capability result", async () => {
  const f = fixture(); let finish!: (value: unknown) => void;
  f.exchange.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const screen = await render(<Manage {...f} target={target} records={[]} active changed={() => {}} />);
  await press(screen, "Read node capabilities");
  const signal = f.exchange.mock.calls[0]![2].signal as AbortSignal;
  await screen.rerender(<Manage {...f} target={target} records={[]} active={false} changed={() => {}} />);
  expect(signal.aborted).toBe(true);
  await act(async () => finish(completed({ tag: "Describe", inner: { value: { availableRequests: { kinds: [rc.RemoteControlRequestKind.SetNodeName] } } } })));
  await screen.rerender(<Manage {...f} target={target} records={[]} active changed={() => {}} />);
  expect(screen.queryByText("Review node name")).toBeNull();
  expect(f.exchange).toHaveBeenCalledTimes(1);
});

test("retained unknown result is still visible after remount and retirement is explicit", async () => {
  const f = fixture(); const changed = jest.fn();
  const records = [{ operationId: 5n, requestKind: rc.RemoteControlRequestKind.SetNodeName, destination: { tag: "Target", inner: { identity: target } }, state: { tag: "OutcomeUnknown", inner: { failure: { tag: "Timeout" } } } }] as unknown as rc.RemoteControlOperationSnapshot[];
  const first = await render(<Operations operations={f.operations} records={records} changed={changed} />);
  await first.unmount();
  const screen = await render(<Operations operations={f.operations} records={records} changed={changed} />);
  expect(screen.getByText("Set Node Name · change 5")).toBeTruthy();
  expect(f.submit).not.toHaveBeenCalled();
  await press(screen, "Review removal of change 5");
  expect(f.operations.retire).not.toHaveBeenCalled();
  await press(screen, "Remove result");
  expect(f.operations.retire).toHaveBeenCalledWith(5n);
});


test("a replacement client clears old capabilities and releases an abandoned read", async () => {
  const old = fixture(); const next = fixture(); let finish!: (value: unknown) => void;
  const screen = await render(<Manage {...old} target={target} records={[]} active changed={() => {}} />);
  await press(screen, "Read node capabilities"); await press(screen, "Controls");
  await fireEvent.changeText(screen.getByLabelText("New node name"), "Retained draft");
  old.exchange.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  await press(screen, "Read node capabilities");
  const signal = old.exchange.mock.calls[1]![2].signal as AbortSignal;
  await screen.rerender(<Manage {...next} target={target} records={[]} active changed={() => {}} />);
  expect(signal.aborted).toBe(true);
  expect(screen.queryByText("Review node name")).toBeNull();
  // A pending old read must not leave the new client's button stuck disabled.
  await press(screen, "Read node capabilities");
  expect(next.exchange).toHaveBeenCalledTimes(1);
  expect(screen.getByLabelText("New node name").props.value).toBe("Retained draft");
  await act(async () => finish(completed({ tag: "Describe", inner: { value: { availableRequests: { kinds: [rc.RemoteControlRequestKind.SetSystemPower] } } } })));
  expect(screen.queryByText("Node power: Asleep")).toBeNull();
  expect(screen.getByText("Review node name")).toBeTruthy();
});


const serviceDestination = new Uint8Array(16).fill(0x42);
const taggedValue = (tag: string, value: unknown) => ({ tag, inner: { value } });
function routeFailure(rejection = 0) {
  return { tag: "Failed", inner: { failure: taggedValue("Connect", taggedValue("EstablishLink",
    taggedValue("Failed", taggedValue("Rejected", rejection)))) } };
}
function discoveryFixture() {
  const f = fixture();
  const resolve = jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: { endpoint: { destinationHash: { value: serviceDestination } } } } });
  const discover = jest.fn().mockResolvedValue({ tag: "Succeeded", inner: { outcome: { tag: "PathDiscovered", inner: { hops: 1 } } } });
  const client = { ...f.client, resolveRemoteControlTarget: resolve, execute: discover } as unknown as rc.HostClientHandleLike;
  return { ...f, client, resolve, discover };
}
async function missingRoute(screen: Awaited<ReturnType<typeof render>>, exchange: jest.Mock) {
  exchange.mockResolvedValueOnce(routeFailure());
  await press(screen, "Read node capabilities");
  expect(screen.getByText(/This node has not been discovered/)).toBeTruthy();
  expect(screen.getByText("Find node")).toBeTruthy();
}

test("known-route reads send no discovery traffic and only a typed missing route offers Find node", async () => {
  const f = discoveryFixture();
  const screen = await render(<Manage {...f} target={target} records={[]} active changed={() => {}} />);
  await press(screen, "Read node capabilities");
  expect(screen.queryByText("Find node")).toBeNull();
  expect(f.resolve).not.toHaveBeenCalled(); expect(f.discover).not.toHaveBeenCalled();
  f.exchange.mockResolvedValueOnce(routeFailure(1));
  await press(screen, "Read node capabilities");
  expect(screen.queryByText("Find node")).toBeNull();
  await missingRoute(screen, f.exchange);
  expect(f.discover).not.toHaveBeenCalled(); expect(f.submit).not.toHaveBeenCalled();
});

test("Find node discovers the canonical service destination then reads capabilities once", async () => {
  const buttons = jest.spyOn(require("../src/ui/primitives") as typeof import("../src/ui/primitives"), "Button");
  const f = discoveryFixture();
  const screen = await render(<Manage {...f} target={target} records={[]} active changed={() => {}} />);
  await missingRoute(screen, f.exchange);
  await act(async () => { const find = (buttons.mock.calls.findLast(([props]) => props.title === "Find node")![0].onPress as () => void); find(); find(); });
  expect(f.resolve).toHaveBeenCalledTimes(1);
  expect(f.resolve).toHaveBeenCalledWith(target, expect.objectContaining({ signal: expect.anything() }));
  expect(f.discover).toHaveBeenCalledTimes(1);
  expect(f.discover).toHaveBeenCalledWith(expect.objectContaining({ tag: "RequestPath", inner: { destination: serviceDestination } }), expect.objectContaining({ signal: expect.anything() }));
  expect(f.exchange).toHaveBeenCalledTimes(2);
  expect(f.exchange.mock.calls[1]?.[0]).toEqual(target.value);
  expect(f.exchange.mock.calls[1]?.[1]).toEqual(expect.objectContaining({ tag: "Describe" }));
  expect(screen.queryByText("Find node")).toBeNull();
  expect(f.submit).not.toHaveBeenCalled();
});

test("path-discovery timeout leaves a recovery action and sends no capability request or mutation", async () => {
  const f = discoveryFixture();
  const screen = await render(<Manage {...f} target={target} records={[]} active changed={() => {}} />);
  await missingRoute(screen, f.exchange);
  f.discover.mockResolvedValueOnce({ tag: "Failed", inner: { failure: { tag: "DeliveryTimedOut" } } });
  await press(screen, "Find node");
  expect(screen.getByText(/could not be found/)).toBeTruthy();
  expect(screen.getByText("Find node")).toBeTruthy();
  expect(f.exchange).toHaveBeenCalledTimes(1); expect(f.submit).not.toHaveBeenCalled();
});

test.each(["resolve", "path"])("leaving the route during %s prevents further discovery/read work", async (stage) => {
  const f = discoveryFixture();
  const screen = await render(<Manage {...f} target={target} records={[]} active changed={() => {}} />);
  await missingRoute(screen, f.exchange);
  let finish!: (value: unknown) => void;
  const pending = stage === "resolve" ? f.resolve : f.discover;
  pending.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  await press(screen, "Find node");
  await screen.rerender(<Manage {...f} target={target} records={[]} active={false} changed={() => {}} />);
  await act(async () => finish(stage === "resolve"
    ? { tag: "Completed", inner: { value: { endpoint: { destinationHash: { value: serviceDestination } } } } }
    : { tag: "Succeeded", inner: { outcome: { tag: "PathDiscovered", inner: { hops: 1 } } } }));
  expect(f.discover).toHaveBeenCalledTimes(stage === "resolve" ? 0 : 1);
  expect(f.exchange).toHaveBeenCalledTimes(1); expect(f.submit).not.toHaveBeenCalled();
});

test.each(["client", "target"])("a stale discovery completion cannot read a replacement %s", async (replacement) => {
  const f = discoveryFixture(); const next = discoveryFixture();
  const screen = await render(<Manage {...f} target={target} records={[]} active changed={() => {}} />);
  await missingRoute(screen, f.exchange);
  let finish!: (value: unknown) => void;
  f.discover.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  await press(screen, "Find node");
  await screen.rerender(<Manage {...(replacement === "client" ? next : f)} target={replacement === "target" ? { value: new Uint8Array(16).fill(8) } : target} records={[]} active changed={() => {}} />);
  await act(async () => finish({ tag: "Succeeded", inner: { outcome: { tag: "PathDiscovered", inner: { hops: 1 } } } }));
  expect(f.exchange).toHaveBeenCalledTimes(1); expect(next.exchange).not.toHaveBeenCalled();
  expect(screen.queryByText("Find node")).toBeNull();
  expect(screen.queryByText(/Available actions:/)).toBeNull();
  expect(f.submit).not.toHaveBeenCalled(); expect(next.submit).not.toHaveBeenCalled();
});


test("busy target resolution remains retryable without starting discovery or another read", async () => {
  const f = discoveryFixture();
  const screen = await render(<Manage {...f} target={target} records={[]} active changed={() => {}} />);
  await missingRoute(screen, f.exchange);
  f.resolve.mockResolvedValueOnce({ tag: "Failed", inner: { failure: { tag: "Busy" } } });
  await press(screen, "Find node");
  expect(screen.getByText(/could not be found: Busy/)).toBeTruthy();
  expect(screen.getByText("Find node")).toBeTruthy();
  expect(f.discover).not.toHaveBeenCalled();
  expect(f.exchange).toHaveBeenCalledTimes(1); expect(f.submit).not.toHaveBeenCalled();
});

afterEach(() => { jest.restoreAllMocks(); });
