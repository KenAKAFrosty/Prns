import { act, fireEvent, render } from "@testing-library/react-native";
import { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { Nodes } from "../src/nodes/Nodes";
import { NodesScreen } from "../src/screens/NodesScreen";

const mockNavigate = jest.fn();
let mockRouteParams: { nodeId?: string; section?: string; tab?: string } = {};
let mockOwner: owners.ServiceRuntimeLike | undefined;
jest.mock("expo-router", () => ({ Link: ({ href, children }: { href: unknown; children: React.ReactElement }) => {
  const React = require("react"); return React.cloneElement(children, { onPress: () => mockNavigate(href) });
}, useLocalSearchParams: () => mockRouteParams, usePathname: () => "/nodes",
useRouter: () => ({ push: mockNavigate, replace: mockNavigate }) }));
jest.mock("../src/runtime/InstallationProvider", () => ({ useInstallation: () => ({ owner: mockOwner }) }));
jest.mock("../src/navigation", () => ({ ...jest.requireActual("../src/navigation"),
  useRouteLease: () => ({ active: true, isCurrent: () => true }) }));
beforeEach(() => { mockRouteParams = {}; mockOwner = undefined; });
afterEach(() => { jest.useRealTimers(); mockNavigate.mockClear(); });
jest.mock("../src/nodes/Access", () => ({ Access: () => null }));
jest.mock("../src/nodes/Pairing", () => ({ Pairing: ({ active, onOpenPaired }: { active: boolean; onOpenPaired: (target: unknown) => void }) => {
  const { Button } = require("react-native");
  return active ? <Button title="Open confirmed paired fixture" onPress={() => onOpenPaired({ value: new Uint8Array(16).fill(7) })} /> : null;
} }));
jest.mock("../src/nodes/Manage", () => ({ nodeDetailTabs: ["Overview", "Interfaces", "Controls", "Access"], Manage: ({ target, tab }: { target: unknown; tab?: string }) => {
  const { Text } = require("react-native"); mockManagedTargets.push(target);
  return <Text>Managed detail: {tab ?? "Overview"}</Text>;
} }));
const mockManagedTargets: unknown[] = [];
jest.mock("../src/nodes/Operations", () => ({ Operations: () => null }));
jest.mock("@prns/app-bindings", () => {
  const tags = new Proxy({}, { get: (_, key) => key });
  return {
    prns_chat_app: { appRemoteControlOperations: () => ({ operations: () => [] }) },
    prns_host_uniffi: { RemoteControlTargetInventorySettlement_Tags: tags, RemoteControlForgetRemoteControlTargetSettlement_Tags: tags },
  };
});
const target = { value: new Uint8Array(16).fill(7) };
function fixture() {
  const forget = jest.fn().mockReturnValue(new Promise(() => {}));
  const client = { remoteControlTargetInventory: jest.fn().mockResolvedValue({ tag: "Completed", inner: { value: { targets: [target] } } }), forgetRemoteControlTarget: forget };
  const owner = { host: () => client } as unknown as owners.ServiceRuntimeLike;
  return { client, owner, forget };
}

test("forget guards inactive callbacks and same-tick duplicate confirmation", async () => {
  const buttons = jest.spyOn(require("../src/ui/primitives") as typeof import("../src/ui/primitives"), "Button");
  const f = fixture(); const screen = await render(<Nodes owner={f.owner} active />);
  await act(async () => await fireEvent.press(screen.getByText("Review forgetting 070707070707")));
  await screen.rerender(<Nodes owner={f.owner} active={false} />);
  const callback = () => (buttons.mock.calls.findLast(([props]) => props.title === "Forget this saved node")![0].onPress as () => void);
  await act(async () => callback()());
  expect(f.forget).not.toHaveBeenCalled();
  await screen.rerender(<Nodes owner={f.owner} active />);
  await act(async () => { const confirm = callback(); confirm(); confirm(); });
  expect(f.forget).toHaveBeenCalledTimes(1);
  expect(f.forget).toHaveBeenCalledWith(target);
});

test("owner replacement clears a pending forget and ignores its late failure", async () => {
  const old = fixture(); const next = fixture(); let reject!: (cause: unknown) => void;
  old.forget.mockReturnValue(new Promise((_, fail) => { reject = fail; }));
  const screen = await render(<Nodes owner={old.owner} active />);
  await act(async () => await fireEvent.press(screen.getByText("Review forgetting 070707070707")));
  await act(async () => await fireEvent.press(screen.getByText("Forget this saved node")));
  await screen.rerender(<Nodes owner={next.owner} active />);
  expect(screen.queryByText("Forget this saved node")).toBeNull();
  await act(async () => reject(new Error("Old owner closed")));
  expect(screen.queryByText(/Removal was not confirmed/)).toBeNull();
  await act(async () => await fireEvent.press(screen.getByText("Review forgetting 070707070707")));
  await act(async () => await fireEvent.press(screen.getByText("Forget this saved node")));
  expect(next.forget).toHaveBeenCalledTimes(1);
});


test("saved target action delegates navigation and exact routed target survives decoded inventory replacement", async () => {
  jest.useFakeTimers();
  const f = fixture(); const navigate = jest.fn();
  const screen = await render(<Nodes owner={f.owner} active onOpenTarget={navigate} />);
  await act(async () => await fireEvent.press(screen.getByText("Manage 070707070707")));
  expect(navigate).toHaveBeenCalledWith("07".repeat(16));
  expect(screen.queryByText(/Managed detail/)).toBeNull();
  await screen.rerender(<Nodes owner={f.owner} active targetId={"07".repeat(16)} detailTab="Interfaces" />);
  expect(screen.getByText("Managed detail: Interfaces")).toBeTruthy();
  const selected = mockManagedTargets.at(-1);
  f.client.remoteControlTargetInventory.mockResolvedValueOnce({ tag: "Completed", inner: { value: { targets: [{ value: new Uint8Array(16).fill(7) }] } } });
  expect(screen.queryByText("Refresh saved nodes")).toBeNull();
  await act(async () => jest.advanceTimersByTime(2000));
  expect(mockManagedTargets.at(-1)).toBe(selected);
  await screen.unmount(); jest.useRealTimers();
});

test("an unknown routed target never silently selects another saved node", async () => {
  const f = fixture();
  const screen = await render(<Nodes owner={f.owner} active targetId={"08".repeat(16)} />);
  expect(screen.getByText(/This node is not saved/)).toBeTruthy();
  expect(screen.queryByText(/Managed detail/)).toBeNull();
});


test("confirmed pairing action opens the exact saved node through the owning route", async () => {
  const f = fixture(); const navigate = jest.fn();
  const screen = await render(<Nodes owner={f.owner} active section="Pairing" onOpenTarget={navigate} />);
  await act(async () => await fireEvent.press(screen.getByText("Open confirmed paired fixture")));
  expect(navigate).toHaveBeenCalledWith("07".repeat(16));
});


test("the Nodes overview reaches This phone before an installation is opened", async () => {
  const screen = await render(<NodesScreen />);
  expect(screen.getByRole("button", { name: "Open phone setup" })).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "This phone" }));
  expect(mockNavigate).toHaveBeenCalledWith("/nodes/phone");
});

test("This phone stays reachable offline and appears only once after networking starts", async () => {
  mockOwner = { host: () => { throw new Error("Networking is stopped"); } } as unknown as owners.ServiceRuntimeLike;
  const screen = await render(<NodesScreen />);
  expect(screen.getByText(/Node management is unavailable/)).toBeTruthy();
  await fireEvent.press(screen.getByRole("button", { name: "This phone" }));
  expect(mockNavigate).toHaveBeenCalledWith("/nodes/phone");
  const f = fixture(); mockOwner = f.owner;
  await screen.rerender(<NodesScreen />);
  expect(screen.getAllByRole("button", { name: "This phone" })).toHaveLength(1);
  expect(screen.getByText("Manage 070707070707")).toBeTruthy();
  expect(f.forget).not.toHaveBeenCalled();
});

test("This phone is an overview action, not a replacement for scoped remote-node routes", async () => {
  const f = fixture(); mockOwner = f.owner;
  mockRouteParams = { nodeId: "07".repeat(16), tab: "Interfaces" };
  const screen = await render(<NodesScreen />);
  expect(screen.queryByRole("button", { name: "This phone" })).toBeNull();
  expect(screen.getByText("Managed detail: Interfaces")).toBeTruthy();
  mockRouteParams = { section: "Pairing" };
  await screen.rerender(<NodesScreen />);
  expect(screen.queryByRole("button", { name: "This phone" })).toBeNull();
  expect(screen.getByText("Open confirmed paired fixture")).toBeTruthy();
});

afterEach(() => { jest.restoreAllMocks(); });
