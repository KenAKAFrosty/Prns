import { Text, View } from "react-native";
import { act, renderRouter, screen, fireEvent, waitFor, cleanup } from "expo-router/testing-library";
import App from "../src/App";
import { Shell } from "../src/ui/Shell";
import { IdentitiesScreen, IdentityScreen } from "../src/screens/IdentityScreen";
import { ContactDetailScreen } from "../src/screens/ContactDetailScreen";
import { NodesScreen } from "../src/screens/NodesScreen";

jest.mock("../src/runtime/IncomingContacts", () => ({ IncomingContacts: () => null }));
jest.mock("../src/use-foreground", () => ({ useForeground: () => true }));
jest.mock("expo-status-bar", () => ({ StatusBar: () => null }));
jest.mock("react-native-safe-area-context", () => {
  const { View } = require("react-native");
  return { SafeAreaProvider: View, SafeAreaView: View };
});
jest.mock("@react-native-async-storage/async-storage", () => ({ __esModule: true, default: {
  getItem: jest.fn(async (key: string) => mockStorage.get(key) ?? null),
  setItem: jest.fn(async (key: string, value: string) => { mockStorage.set(key, value); }),
} }));
jest.mock("@prns/app-platform", () => ({
  installationPresent: async () => true, bluetoothSelected: async () => false,
  start: async () => {}, stop: async () => {},
}));
jest.mock("@prns/app-bindings", () => ({
  prns_chat_app: { appOwner: () => mockOwner },
  prns_service_runtime: { ServiceRuntimeNetworkPhase: { Offline: 0, Ready: 2 }, ServiceRuntimePhase: { Ready: 1 } },
}));
jest.mock("../src/IdentityPanel", () => ({ IdentityPanel: ({ record, group, recipient, invitation, messagingScreen, onOpenGroup, onOpenInvitation, onInviteGroup, onOpenActivity, onOpenDetails, onOpenDelivery, incarnation, messageId, onOpenMessage }: {
  record: { metadata: { localLabel: string } }; group?: string; recipient?: string; invitation?: string; messagingScreen?: string; onOpenGroup?: (value: string) => void;
  onOpenInvitation?: (value: string) => void; onInviteGroup?: (value: string) => void; onOpenActivity?: () => void;
  onOpenDetails?: (value: string) => void; onOpenDelivery?: (value: string) => void;
  incarnation?: string; messageId?: string; onOpenMessage?: (group: string, incarnation: string, message: string) => void;
}) => {
  const { Text, Button, View } = require("react-native");
  if (onOpenMessage !== undefined) mockMessageNavigations.set(record.metadata.localLabel, onOpenMessage);
  if (onOpenGroup !== undefined) mockGroupNavigations.set(record.metadata.localLabel, onOpenGroup);
  if (onOpenDetails !== undefined && onOpenDelivery !== undefined) mockDetailNavigations.set(record.metadata.localLabel, { details: onOpenDetails, delivery: onOpenDelivery });
  return <View><Text>Identity content: {record.metadata.localLabel}</Text>
    <Text>Messaging screen: {messagingScreen}</Text>
    {incarnation !== undefined && <Text>Message incarnation: {incarnation}</Text>}
    {messageId !== undefined && <Text>Message record: {messageId}</Text>}
    <Button title="Open exact message" onPress={() => onOpenMessage?.(group!, "c".repeat(48), "d".repeat(64))} />{invitation !== undefined && <Text>Invitation content: {invitation}</Text>}
    <Button title="Review saved invitation" onPress={() => onOpenInvitation?.("b".repeat(64))} />
    <Button title="Invite saved contact" onPress={() => onInviteGroup?.("a".repeat(64))} />
    <Button title="Open conversation info" onPress={() => onOpenDetails?.(group!)} />
    <Button title="Open connection activity" onPress={() => onOpenDelivery?.(group!)} />
    <Button title="Open saved actions" onPress={onOpenActivity} />{recipient !== undefined && <Text>Recipient: {recipient}</Text>}{group === undefined
    ? <Button title="Open saved conversation" onPress={() => onOpenGroup?.("a".repeat(64))} /> : <Text>Conversation content: {group}</Text>}</View>;
} }));

jest.mock("../src/Identities", () => ({ Identities: ({ onSelect }: { onSelect: (reference: string | undefined) => void }) => {
  const { Text } = require("react-native");
  mockIdentitySelections.push(onSelect);
  return <Text>Identity creation fixture</Text>;
} }));

jest.mock("../src/contacts", () => ({
  ...jest.requireActual("../src/contacts"), loadContacts: async () => [mockContact],
}));
jest.mock("../src/nodes/Manage", () => ({ nodeDetailTabs: ["Overview", "Interfaces", "Controls", "Access"] }));
jest.mock("../src/nodes/Nodes", () => ({
  nodeSections: ["Nodes", "Access", "Pairing", "Changes"],
  Nodes: ({ section, targetId, detailTab, onOpenTarget, onSectionChange, onDetailTabChange }: {
    section: string; targetId?: string; detailTab: string;
    onOpenTarget(id: string): void; onSectionChange(section: string): void; onDetailTabChange(tab: string): void;
  }) => {
    const { View, Text, Button } = require("react-native");
    mockTargetNavigations.push(onOpenTarget);
    return <View><Text>Node section: {section}</Text><Text>Target: {targetId ?? "none"}</Text><Text>Detail tab: {detailTab}</Text>
      <Button title="Manage saved target" onPress={() => onOpenTarget("07".repeat(16))} />
      <Button title="Show node interfaces" onPress={() => onDetailTabChange("Interfaces")} />
      <Button title="Show pairing" onPress={() => onSectionChange("Pairing")} /></View>;
  },
}));
const mockIdentitySelections: ((reference: string | undefined) => void)[] = [];
const mockTargetNavigations: ((id: string) => void)[] = [];
const mockContact = { uri: "verified", alias: "Alex", claims: { displayName: "Alex", identityHash: new Uint8Array(16).fill(3).buffer,
  serviceNameHash: new Uint8Array(10).fill(4).buffer } };
const contactId = `${"03".repeat(16)}:${"04".repeat(10)}`;
const mockStorage = new Map<string, string>();
const mockMessageNavigations = new Map<string, (group: string, incarnation: string, message: string) => void>();
const mockDetailNavigations = new Map<string, { details: (group: string) => void; delivery: (group: string) => void }>();
const mockGroupNavigations = new Map<string, (group: string) => void>();
const record = (value: number, label: string) => ({
  id: { incarnation: new Uint8Array(16).fill(value).buffer, sequence: BigInt(value) },
  publicHash: new Uint8Array(16).fill(value).buffer,
  metadata: { localLabel: label, publishedName: label }, services: [0x67727073], desired: 0,
});
const records = [record(1, "Personal"), record(2, "Work")];
const mockOwner = {
  status: () => ({ phase: 1, cleanupComplete: false }),
  networkStatus: () => ({ phase: 0, generation: 0n }),
  identities: () => ({ identities: async () => records }),
};
const key = (value: number) => `${value.toString(16).padStart(2, "0").repeat(16)}:${value}:${value.toString(16).padStart(2, "0").repeat(16)}`;
async function open(initialUrl: string) {
  const view = await renderRouter({
    _layout: () => <App><Shell /></App>,
    conversations: () => <IdentityScreen section="Conversations" />,
    "conversations/new": () => <IdentityScreen section="Conversations" />,
    "contacts/[contact]": ContactDetailScreen,
    "conversations/group/[group]": () => <IdentityScreen section="Conversations" />,
    "conversations/group/[group]/details": () => <IdentityScreen section="Conversations" />,
    "conversations/group/[group]/delivery": () => <IdentityScreen section="Conversations" />,
    "conversations/group/[group]/message/[incarnation]/[message]": () => <IdentityScreen section="Conversations" />,
    "conversations/group/[group]/invite": () => <IdentityScreen section="Conversations" />,
    "conversations/invitation/[invitation]": () => <IdentityScreen section="Conversations" />,
    "conversations/activity": () => <IdentityScreen section="Conversations" />,
    contacts: () => <IdentityScreen section="Contacts" />,
    nodes: NodesScreen,
    "nodes/managed/[nodeId]": NodesScreen,
    "more/identities": IdentitiesScreen,
    "more/identities/[reference]": () => <IdentityScreen section="Identity" />,
    more: () => <View><Text>More settings</Text></View>,
  }, { initialUrl });
  await act(async () => {});
  return view;
}
beforeEach(() => { mockStorage.clear(); mockMessageNavigations.clear(); mockGroupNavigations.clear(); mockDetailNavigations.clear(); mockTargetNavigations.length = 0; mockIdentitySelections.length = 0; });
afterEach(cleanup);

test("opening a conversation and switching identity preserves exact route ownership", async () => {
  await open(`/conversations?owner=${encodeURIComponent(key(1))}`);
  await screen.findByText("Identity content: Personal");
  await fireEvent.press(screen.getByText("Open saved conversation"));
  await screen.findByText(`Conversation content: ${"a".repeat(64)}`);
  expect(screen.getByText("Identity content: Personal")).toBeTruthy();
  const oldCompletion = mockGroupNavigations.get("Personal")!;
  await fireEvent.press(screen.getByLabelText("Choose messaging identity"));
  await fireEvent.press(await screen.findByText("Work"));
  await screen.findByText("Identity content: Work");
  await act(async () => { oldCompletion("b".repeat(64)); });
  expect(screen.queryByText(`Conversation content: ${"b".repeat(64)}`)).toBeNull();
  expect(screen.queryByText(`Conversation content: ${"a".repeat(64)}`)).toBeNull();
  await fireEvent.press(screen.getByLabelText("Contacts"));
  await waitFor(() => expect(screen.getByText("Identity content: Work")).toBeTruthy());
});

test("a group link without an identity never uses whichever identity is selected", async () => {
  await open(`/conversations/group/${"a".repeat(64)}`);
  await screen.findByText("Choose a messaging identity from Conversations to open this page.");
  expect(screen.queryByText("Identity content: Personal")).toBeNull();
});

test("a stale identity link does not fall back to another saved identity", async () => {
  await open(`/conversations?owner=${encodeURIComponent(key(9))}`);
  await screen.findByText("This identity is not available in this installation. The route has not been changed to another identity.");
  expect(screen.queryByText("Identity content: Personal")).toBeNull();
});


test("contact conversation action preserves its exact identity and recipient after creation", async () => {
  await open(`/contacts/${contactId}?owner=${encodeURIComponent(key(2))}`);
  await fireEvent.press(await screen.findByText("New conversation"));
  await screen.findByText("Messaging screen: create");
  expect(screen.getByText("Identity content: Work")).toBeTruthy();
  expect(screen.getByText(`Recipient: ${contactId}`)).toBeTruthy();
  await fireEvent.press(screen.getByText("Open saved conversation"));
  await screen.findByText(`Conversation content: ${"a".repeat(64)}`);
  expect(screen.getByText(`Recipient: ${contactId}`)).toBeTruthy();
  expect(screen.getByText("Identity content: Work")).toBeTruthy();
});

test("contact create links without an explicit owner do not borrow the selected identity", async () => {
  await open(`/conversations/new?contact=${contactId}`);
  await screen.findByText("Choose a saved contact from this messaging identity.");
  expect(screen.queryByText("Identity content: Personal")).toBeNull();
});

test("node selection and detail tabs are routed; a stale completion cannot change the new route", async () => {
  await open("/nodes");
  await screen.findByText("Target: none");
  const stale = mockTargetNavigations.at(-1)!;
  await fireEvent.press(screen.getByText("Manage saved target"));
  await screen.findByText(`Target: ${"07".repeat(16)}`);
  await fireEvent.press(screen.getByText("Show node interfaces"));
  await screen.findByText("Detail tab: Interfaces");
  expect(screen.getByText(`Target: ${"07".repeat(16)}`)).toBeTruthy();
  await act(async () => stale("08".repeat(16)));
  expect(screen.queryByText(`Target: ${"08".repeat(16)}`)).toBeNull();
  await fireEvent.press(screen.getByText("Show pairing"));
  await screen.findByText("Node section: Pairing");
  expect(screen.getByText("Target: none")).toBeTruthy();
});

test("node deep links restore the exact target and tab", async () => {
  await open(`/nodes/managed/${"AB".repeat(16)}?tab=Controls`);
  await screen.findByText(`Target: ${"ab".repeat(16)}`);
  expect(screen.getByText("Detail tab: Controls")).toBeTruthy();
});

test("invalid node targets fail closed", async () => {
  await open("/nodes/managed/not-a-node?tab=Controls");
  await screen.findByText("The node or section reference is invalid. Choose a saved node from Nodes.");
  expect(screen.queryByText("Detail tab: Controls")).toBeNull();
});


test("a late identity-creation callback cannot select another identity or navigate after leaving its route", async () => {
  const route = await open(`/conversations?owner=${encodeURIComponent(key(1))}`);
  await screen.findByText("Identity content: Personal");
  await fireEvent.press(screen.getByLabelText("Choose messaging identity"));
  await fireEvent.press(await screen.findByText("Manage identities"));
  await screen.findByText("Identity creation fixture");
  const completedCreation = mockIdentitySelections.at(-1)!;
  await fireEvent.press(screen.getByLabelText("Contacts"));
  await screen.findByText("Identity content: Personal");
  expect(route.getPathname()).toBe("/contacts");
  await act(async () => { completedCreation(key(2)); });
  expect(route.getPathname()).toBe("/contacts");
  expect(screen.getByLabelText("Choose messaging identity")).toHaveTextContent("Personal");
  expect(screen.getByText("Identity content: Personal")).toBeTruthy();
  expect(screen.queryByText("Identity content: Work")).toBeNull();
});


test("focused invitation and saved-action routes preserve their explicit identity and return target", async () => {
  const view = await open(`/conversations?owner=${key(1)}`);
  await fireEvent.press(screen.getByText("Open saved conversation"));
  await screen.findByText(`Conversation content: ${"a".repeat(64)}`);
  await fireEvent.press(screen.getByText("Invite saved contact"));
  await screen.findByText("Messaging screen: invite");
  expect(view.getSearchParams()).toMatchObject({ owner: key(1), group: "a".repeat(64) });
  await fireEvent.press(screen.getByRole("link", { name: "Back to Conversation" }));
  await screen.findByText("Messaging screen: conversation");
  await fireEvent.press(screen.getByText("Review saved invitation"));
  await screen.findByText(`Invitation content: ${"b".repeat(64)}`);
  expect(view.getSearchParams()).toMatchObject({ owner: key(1), invitation: "b".repeat(64) });
  await fireEvent.press(screen.getByRole("link", { name: "Back to Conversations" }));
  await screen.findByText("Messaging screen: list");
  await fireEvent.press(screen.getByText("Open saved actions"));
  await screen.findByText("Messaging screen: activity");
  expect(view.getSearchParams()).toMatchObject({ owner: key(1) });
});

test.each([
  `/conversations/invitation/${"b".repeat(64)}`,
  `/conversations/group/${"a".repeat(64)}/invite`,
  "/conversations/activity",
])("focused link %s requires its owner rather than borrowing a selected identity", async (url) => {
  await open(url);
  await screen.findByText("Choose a messaging identity from Conversations to open this page.");
  expect(screen.queryByText("Identity content: Personal")).toBeNull();
});

test("malformed invitation IDs do not reach a selected identity client", async () => {
  await open(`/conversations/invitation/invalid?owner=${key(1)}`);
  await screen.findByText("The invitation reference is invalid.");
  expect(screen.queryByText("Identity content: Personal")).toBeNull();
});


test.each([
  ["details", "Open conversation info"], ["delivery", "Open connection activity"],
] as const)("%s routes retain the exact owner and conversation, including Back and late callbacks", async (destination, label) => {
  const group = "a".repeat(64);
  const view = await open(`/conversations/group/${group}?owner=${encodeURIComponent(key(2))}`);
  await screen.findByText("Identity content: Work");
  await fireEvent.press(screen.getByText(label));
  await screen.findByText(`Messaging screen: ${destination}`);
  expect(view.getPathname()).toBe(`/conversations/group/${group}/${destination}`);
  expect(view.getSearchParams()).toMatchObject({ owner: key(2), group });
  const retired = mockDetailNavigations.get("Work")![destination];
  await fireEvent.press(screen.getByRole("link", { name: "Back to Conversation" }));
  await screen.findByText("Messaging screen: conversation");
  expect(view.getSearchParams()).toMatchObject({ owner: key(2), group });
  await act(async () => { retired("b".repeat(64)); });
  expect(view.getPathname()).toBe(`/conversations/group/${group}`);
  expect(view.getSearchParams()).toMatchObject({ owner: key(2), group });

  const previousIdentity = mockDetailNavigations.get("Work")![destination];
  await fireEvent.press(screen.getByLabelText("Choose messaging identity"));
  await fireEvent.press(await screen.findByText("Personal"));
  await screen.findByText("Identity content: Personal");
  await act(async () => { previousIdentity(group); });
  expect(view.getPathname()).toBe("/conversations");
  expect(view.getSearchParams()).toMatchObject({ owner: key(1) });
  expect(screen.queryByText(`Conversation content: ${group}`)).toBeNull();
});

test.each(["details", "delivery"])("a %s deep link cannot borrow an implicit or missing identity", async (destination) => {
  await open(`/conversations/group/${"a".repeat(64)}/${destination}`);
  await screen.findByText("Choose a messaging identity from Conversations to open this page.");
  expect(screen.queryByText("Identity content: Personal")).toBeNull();
});


test("message details retain exact identity, group and incarnation with scoped Back and retired callbacks", async () => {
  const group = "a".repeat(64), incarnation = "c".repeat(48), message = "d".repeat(64);
  const view = await open(`/conversations/group/${group}?owner=${encodeURIComponent(key(2))}`);
  await fireEvent.press(await screen.findByText("Open exact message"));
  await screen.findByText("Messaging screen: message");
  expect(view.getSearchParams()).toMatchObject({ owner: key(2), group, incarnation, message });
  expect(screen.getByText(`Message incarnation: ${incarnation}`)).toBeTruthy();
  expect(screen.getByText(`Message record: ${message}`)).toBeTruthy();
  const retired = mockMessageNavigations.get("Work")!;
  await fireEvent.press(screen.getByRole("link", { name: "Back to Conversation" }));
  await screen.findByText("Messaging screen: conversation");
  expect(view.getSearchParams()).toMatchObject({ owner: key(2), group });
  await act(async () => retired(group, "e".repeat(48), message));
  expect(view.getPathname()).toBe(`/conversations/group/${group}`);
  const oldIdentity = mockMessageNavigations.get("Work")!;
  await fireEvent.press(screen.getByLabelText("Choose messaging identity"));
  await fireEvent.press(await screen.findByText("Personal"));
  await screen.findByText("Identity content: Personal");
  await act(async () => oldIdentity(group, incarnation, message));
  expect(view.getPathname()).toBe("/conversations");
  expect(view.getSearchParams()).toMatchObject({ owner: key(1) });
});

test("a retained message deep link preserves its incarnation without a loaded history page", async () => {
  const group = "a".repeat(64), incarnation = "e".repeat(48), message = "d".repeat(64);
  await open(`/conversations/group/${group}/message/${incarnation}/${message}?owner=${encodeURIComponent(key(2))}`);
  expect(await screen.findByText(`Message incarnation: ${incarnation}`)).toBeTruthy();
  expect(screen.getByText(`Message record: ${message}`)).toBeTruthy();
  expect(screen.getByText("Identity content: Work")).toBeTruthy();
});

test("a message link cannot borrow the currently selected identity", async () => {
  await open(`/conversations/group/${"a".repeat(64)}/message/${"c".repeat(48)}/${"d".repeat(64)}`);
  await screen.findByText("Choose a messaging identity from Conversations to open this page.");
  expect(screen.queryByText("Messaging screen: message")).toBeNull();
});

test.each([
  ["c".repeat(47), "d".repeat(64), ""],
  ["c".repeat(48), "not-a-record", ""],
])("invalid message references fail closed (%s, %s)", async (incarnation, message, query) => {
  await open(`/conversations/group/${"a".repeat(64)}/message/${incarnation}/${message}?owner=${encodeURIComponent(key(2))}${query}`);
  await screen.findByText("The message reference is invalid. Open a message from its conversation.");
  expect(screen.queryByText("Messaging screen: message")).toBeNull();
});


test.each(["incarnation", "message"])("query entries cannot replace a message path's exact %s", async (parameter) => {
  const group = "a".repeat(64), incarnation = "c".repeat(48), message = "d".repeat(64);
  await open(`/conversations/group/${group}/message/${incarnation}/${message}?owner=${encodeURIComponent(key(2))}&${parameter}=wrong&${parameter}=also-wrong`);
  expect(await screen.findByText(`Message incarnation: ${incarnation}`)).toBeTruthy();
  expect(screen.getByText(`Message record: ${message}`)).toBeTruthy();
});


test("a message link rejects repeated identity ownership", async () => {
  await open(`/conversations/group/${"a".repeat(64)}/message/${"c".repeat(48)}/${"d".repeat(64)}?owner=${encodeURIComponent(key(1))}&owner=${encodeURIComponent(key(2))}`);
  await screen.findByText("Choose a messaging identity from Conversations to open this page.");
  expect(screen.queryByText("Messaging screen: message")).toBeNull();
});
