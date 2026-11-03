import { act, fireEvent, render } from "@testing-library/react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { Controllers } from "../src/nodes/Controllers";
import { Interface } from "../src/nodes/Interface";

jest.mock("../src/nodes/LoRa", () => ({ LoRa: () => null }));
jest.mock("@prns/app-bindings", () => {
  const tags = new Proxy({}, { get: (_, key) => key });
  const variants = new Proxy({}, { get: (_, key) => class { tag = key; inner: unknown; constructor(value?: unknown) { this.inner = value; } } });
  const enumeration = (...names: string[]) => Object.fromEntries(names.flatMap((name, index) => [[name, index], [index, name]]));
  return { prns_host_uniffi: {
    RemoteControlRequest: variants, RemoteControlInterfacePage: variants, RemoteControlPeerPage: variants, RemoteControlControllerPage: variants,
    RemoteControlDecodeRemoteControlControllerCardSettlement_Tags: tags, RemoteControlControllerContinuation_Tags: tags,
    RemoteControlInterfaceConfigOutcome_Tags: tags, RemoteControlDiscoveryGroupsInventoryOutcome_Tags: tags,
    RemoteControlRadioConfiguration_Tags: tags, RemoteControlRadioSaved_Tags: tags, RemoteControlInterfacePeersOutcome_Tags: tags, RemoteControlPeerContinuation_Tags: tags, RemoteControlRadioStatus_Tags: tags,
    RemoteControlRequestKind: enumeration("Describe", "AuthorizeController", "RevokeController", "InventoryControllers", "ReplaceInterfaceDiscoveryGroups", "SetInterfacePower", "InventoryInterfacePeers"),
    RemoteControlInterfacePower: enumeration("Off", "On"), RemoteControlInterfaceMode: enumeration("Full"), RemoteControlInterfaceKind: enumeration("AutoUdp"),
    remoteControlRequestRequiresAdministrator: jest.fn((kind: number) => [1, 2, 3].includes(kind)),
    remoteControlControllerIdentityHash: (value: { publicKeys: Uint8Array }) => ({ value: value.publicKeys.slice(0, 16) }),
    decodeRemoteControlControllerCard: jest.fn(() => ({ tag: "Completed", inner: { value: { controller: { publicKeys: new Uint8Array(64).fill(7) } } } })),
  }};
});

test("operator delegation uses the shared administrator filter and exact reviewed permissions", async () => {
  const review = jest.fn();
  const screen = await render(<Controllers supported={{ kinds: [0, 1, 2, 3] }} inventory={undefined} disabled={false} blocked={false} read={() => {}} review={review} />);
  await fireEvent.changeText(screen.getByLabelText("Controller card to authorize"), "public-controller-card");
  await act(async () => await fireEvent.press(screen.getByText("Review controller identity")));
  expect(rc.remoteControlRequestRequiresAdministrator).toHaveBeenCalledWith(rc.RemoteControlRequestKind.AuthorizeController);
  expect(screen.queryByLabelText("Permit Authorize Controller")).toBeNull();
  await fireEvent(screen.getByLabelText("Permit Describe"), "valueChange", true);
  await act(async () => await fireEvent.press(screen.getByText("Review controller authorization")));
  expect(review).not.toHaveBeenCalled();
  await fireEvent(screen.getByLabelText("I verified this controller identity"), "valueChange", true);
  await act(async () => await fireEvent.press(screen.getByText("Review controller authorization")));
  expect(review).toHaveBeenCalledWith(expect.objectContaining({ tag: "AuthorizeController", inner: { controller: { publicKeys: new Uint8Array(64).fill(7) }, permittedRequests: { kinds: [0] } } }));
});

test("revocation carries the exact inventory identity and unfinished work blocks it", async () => {
  const review = jest.fn(); const hash = { value: new Uint8Array(16).fill(8) };
  const inventory = { hashes: [hash], continuation: { tag: "Complete" } } as rc.RemoteControlControllerInventory;
  const screen = await render(<Controllers supported={{ kinds: [2, 3] }} inventory={inventory} disabled={false} blocked read={() => {}} review={review} />);
  await act(async () => await fireEvent.press(screen.getByText("Review revoking 080808080808")));
  expect(review).not.toHaveBeenCalled();
  await screen.rerender(<Controllers supported={{ kinds: [2, 3] }} inventory={inventory} disabled={false} blocked={false} read={() => {}} review={review} />);
  await act(async () => await fireEvent.press(screen.getByText("Review revoking 080808080808")));
  expect(review).toHaveBeenCalledWith(expect.objectContaining({ tag: "RevokeController", inner: { hash } }));
});

test("discovery replacement requires a supported outcome and empty list is an explicit review", async () => {
  const review = jest.fn(); const id = { value: new Uint8Array(16).fill(2) };
  const entry = { id, kind: 0, mode: 0, enabled: true, rxBytes: 0n, txBytes: 0n } as rc.RemoteControlInterfaceEntry;
  const props = { entry, supported: { kinds: [4] }, configuration: undefined, peers: undefined, radio: undefined, disabled: false, blocked: false, read: () => {}, review };
  const screen = await render(<Interface {...props} discovery={{ tag: "Unsupported" } as rc.RemoteControlDiscoveryGroupsInventoryOutcome} />);
  await act(async () => await fireEvent.press(screen.getByText(`Edit interface ${"02".repeat(16)} settings`)));
  expect(screen.queryByText("Review replacing discovery groups")).toBeNull();
  await screen.rerender(<Interface {...props} discovery={{ tag: "Groups", inner: { value: { groups: ["one", "two"] } } } as rc.RemoteControlDiscoveryGroupsInventoryOutcome} />);
  await act(async () => await fireEvent.press(screen.getByText("Copy current discovery groups into editor")));
  expect(screen.getByLabelText(`Interface ${"02".repeat(16)} discovery groups`).props.value).toBe("one\ntwo");
  await fireEvent.changeText(screen.getByLabelText(`Interface ${"02".repeat(16)} discovery groups`), "");
  expect(review).not.toHaveBeenCalled();
  await act(async () => await fireEvent.press(screen.getByText("Review replacing discovery groups")));
  expect(review).toHaveBeenCalledWith(expect.objectContaining({ tag: "ReplaceInterfaceDiscoveryGroups", inner: { id, groups: { groups: [] } } }));
});
