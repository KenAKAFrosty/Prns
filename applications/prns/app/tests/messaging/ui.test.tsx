import "./native-mock";
import { Platform } from "react-native";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { prns_groups as groups, prns_groups_core as protocol, prns_chat_app as app } from "@prns/app-bindings";
import type { prns_identities_core as core } from "@prns/app-bindings";
import { contactKey, type SavedContact } from "../../src/contacts";
import { useCallback, useLayoutEffect, useState } from "react";
import { Messaging as RoutedMessaging, type MessagingProps } from "../../src/messaging/Messaging";

// Model only route transitions: the production screen never renders a hidden
// overview or owns navigation. Explicit callback spies remain under test control.
function Messaging(props: MessagingProps) {
  const [route, setRoute] = useState<Partial<Pick<MessagingProps, "screen" | "group" | "invitation">>>({});
  useLayoutEffect(() => { setRoute({}); }, [props.screen, props.group, props.invitation, props.identity]);
  const openGroup = useCallback((group: string) => setRoute({ screen: "conversation", group }), []);
  const openInvitation = useCallback((invitation: string) => setRoute({ screen: "invitation", invitation }), []);
  const invite = useCallback((group: string) => setRoute({ screen: "invite", group }), []);
  const details = useCallback((group: string) => setRoute({ screen: "details", group }), []);
  const delivery = useCallback((group: string) => setRoute({ screen: "delivery", group }), []);
  const activity = useCallback(() => setRoute({ screen: "activity" }), []);
  const create = useCallback(() => setRoute({ screen: "create" }), []);
  return <RoutedMessaging {...props} {...route} onOpenGroup={props.onOpenGroup ?? openGroup}
    onOpenInvitation={props.onOpenInvitation ?? openInvitation} onInviteGroup={props.onInviteGroup ?? invite}
    onOpenDetails={props.onOpenDetails ?? details} onOpenDelivery={props.onOpenDelivery ?? delivery}
    onOpenActivity={props.onOpenActivity ?? activity} onOpenCreate={props.onOpenCreate ?? create} />;
}
import { History } from "../../src/messaging/History";
import { captureInput, inputId, submitInput } from "../../src/messaging/input";

const bytes = (size: number, value = 1) => new Uint8Array(size).fill(value).buffer;
const identity: core.IdentityReference = { id: { incarnation: bytes(16), sequence: 1n }, publicHash: bytes(16, 2) };
const contact: SavedContact = { uri: "verified-by-parent", alias: "Alex", claims: {
  publicIdentity: bytes(64), identityHash: bytes(16, 3), destinationHash: bytes(16, 4), displayName: "Alex", serviceNameHash: bytes(10),
} };
const profile: groups.GroupProfile = { maxMembers: 256, maxRecordBodyBytes: 2054, maxPlaintextBytes: 16384,
  maxWireBytes: 65536, maxSnapshotBytes: 262144, maxEnrollmentFrameBytes: 524288 };
const capabilities = { canSend: true, canInvite: true, canRemoveMembers: true, canLeave: false, canClose: true, canDeleteLocal: false };
const row: groups.GroupConversation = { group: bytes(32, 5), title: "Garden", revision: 1n,
  control: { head: bytes(32), era: bytes(32, 2) }, lifecycle: groups.GroupLifecycle.Active,
  ownerIdentity: identity.publicHash, capabilities, memberCount: 2n, profile };
const members: groups.GroupConversationMember[] = [
  { identity: identity.publicHash, isLocal: true, isOwner: true, endpoint: { id: bytes(32, 2), incarnation: 1n } },
  { identity: contact.claims.identityHash, isLocal: false, isOwner: false, endpoint: { id: bytes(32, 3), incarnation: 2n } },
];
const summary: groups.GroupConversationSummary = { group: row.group, title: row.title, ownerIdentity: row.ownerIdentity,
  control: row.control, lifecycle: row.lifecycle, capabilities, memberCount: row.memberCount, ready: true };
const summaries = { items: [summary], next: undefined };
const data = new Map<string, string>();
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (reason: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function result(id: ArrayBuffer, intent: groups.GroupIntent): groups.GroupOperation {
  return { request: { id, intent }, scope: new groups.GroupOperationScope.Create(), state: new groups.GroupOperationState.Completed(new groups.GroupOutcome.Saved()) };
}
function fixture(reference = identity) {
  const observation = { next: jest.fn(({ signal }: { signal: AbortSignal }) => new Promise<void>((_resolve, reject) => {
    if (signal.aborted) { reject(new Error("aborted")); return; }
    signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
  })), uniffiDestroy: jest.fn() };
  const value = {
    identity: jest.fn(async () => reference.publicHash), subscribe: jest.fn(async () => observation),
    conversations: jest.fn<Promise<groups.GroupConversationPage>, unknown[]>(async () => summaries),
    conversation: jest.fn<Promise<groups.GroupConversation>, unknown[]>(async () => row),
    members: jest.fn<Promise<groups.GroupMemberPage>, unknown[]>(async () => ({ members, control: row.control, next: undefined })),
    message: jest.fn<Promise<groups.GroupTextMessage | undefined>, unknown[]>(async () => undefined),
    record: jest.fn<Promise<groups.GroupStoredRecord | undefined>, unknown[]>(async () => undefined),
    invitation: jest.fn<Promise<groups.GroupInvitation | undefined>, unknown[]>(async () => undefined),
    invitations: jest.fn<Promise<groups.GroupInvitationPage>, unknown[]>(async () => ({ items: [], next: undefined })),
    operations: jest.fn<Promise<groups.GroupOperation[]>, unknown[]>(async () => []), operation: jest.fn<Promise<groups.GroupOperation | undefined>, unknown[]>(async () => undefined),
    submit: jest.fn(async (id: ArrayBuffer, intent: groups.GroupIntent) => result(id, intent)), dismissOperation: jest.fn<Promise<void>, [ArrayBuffer]>(async () => undefined),
    messages: jest.fn<Promise<groups.GroupTextPage>, unknown[]>(async () => ({ messages: [], visitedHeads: [], currentIncarnation: bytes(24) })),
  };
  return { native: value, client: value as unknown as groups.ClientLike, observation };
}
beforeEach(() => {
  data.clear(); jest.clearAllMocks();
  let counter = 10;
  jest.mocked(groups.createRequestId).mockImplementation(() => bytes(32, counter++));
  jest.mocked(groups.textPublication).mockImplementation((group, text) => ({ group, namespace: 1, schema: 1, targets: new groups.GroupTargets.OtherMembers(), body: new TextEncoder().encode(text).buffer as ArrayBuffer }));
  jest.mocked(AsyncStorage.getItem).mockImplementation(async (key) => data.get(key) ?? null);
  jest.mocked(AsyncStorage.setItem).mockImplementation(async (key, value) => { data.set(key, value); });
  jest.mocked(AsyncStorage.removeItem).mockImplementation(async (key) => { data.delete(key); });
});
afterEach(cleanup);
async function press(label: string) { await act(async () => { await fireEvent.press(screen.getByRole("button", { name: label })); }); }
async function menu(label: string, action: string) {
  await press(label);
  const dismiss = screen.container.queryAll((value) => typeof value.props.onDismiss === "function")[0]!.props.onDismiss;
  await press(action);
  if (Platform.OS === "ios") await act(async () => { dismiss(); });
}

test("subscribe precedes snapshots; switching away releases waiter without stopping service", async () => {
  const { native, client, observation } = fixture();
  const rendered = await render(<Messaging screen="list" identity={identity} client={client} contacts={[contact]} />);
  expect(native.subscribe.mock.invocationCallOrder[0]).toBeLessThan(native.conversations.mock.invocationCallOrder[0]!);
  expect(screen.getByRole("button", { name: "Garden · 2 members · Active" })).toBeTruthy();
  await rendered.rerender(<Messaging screen="list" identity={identity} client={client} contacts={[contact]} active={false} />);
  expect(observation.next.mock.calls[0]?.[0].signal.aborted).toBe(true);
  expect(observation.uniffiDestroy).toHaveBeenCalledTimes(1);
});

test.each([true, false])("native revision refresh keeps the composer editable until fresh send permission is %s", async (canSend) => {
  const { native, client, observation } = fixture();
  const changed = deferred<void>();
  observation.next.mockReturnValueOnce(changed.promise);
  await render(<Messaging screen="conversation" group={"05".repeat(32)} identity={identity} client={client} contacts={[]} />);
  const input = screen.getByLabelText("Message");
  await fireEvent(input, "focus");
  await act(async () => { await fireEvent.changeText(input, "Keep typing"); });
  expect(screen.getByRole("button", { name: "Send message" })).toBeEnabled();

  const refreshed = deferred<groups.GroupConversation>();
  native.conversation.mockReturnValueOnce(refreshed.promise);
  await act(async () => { changed.resolve(); });
  for (const label of ["selected conversation", "conversations", "invitations", "activity"]) {
    expect(screen.queryByLabelText(`Loading ${label}`)).toBeNull();
  }
  expect(screen.queryByText("Refresh conversations page")).toBeNull();
  expect(screen.queryByText("Refresh conversations")).toBeNull();
  expect(native.subscribe).toHaveBeenCalledTimes(1);
  expect(native.conversations).not.toHaveBeenCalled();
  expect(native.invitations).not.toHaveBeenCalled();
  expect(native.operations).not.toHaveBeenCalled();
  // Retain the same input instead of disabling or replacing the focused field.
  expect(screen.getByLabelText("Message")).toBe(input);
  expect(input.props.editable).toBe(true);
  expect(screen.getByRole("button", { name: "Send message" })).toBeEnabled();
  await act(async () => { await fireEvent.changeText(input, "Keep typing during refresh"); });
  expect(input).toHaveDisplayValue("Keep typing during refresh");
  expect(native.submit).not.toHaveBeenCalled();

  await act(async () => {
    refreshed.resolve({ ...row, group: row.group.slice(0),
    control: { head: row.control.head.slice(0), era: row.control.era.slice(0) },
    capabilities: { ...capabilities, canSend } }); });
  expect(screen.getByLabelText("Message")).toBe(input);
  expect(input).toHaveDisplayValue("Keep typing during refresh");
  expect(input.props.editable).toBe(canSend);
  if (canSend) expect(screen.getByRole("button", { name: "Send message" })).toBeEnabled();
  else expect(screen.getByRole("button", { name: "Send message" })).toBeDisabled();
  expect(native.submit).not.toHaveBeenCalled();
});

test("message history shows initial loading and retains its layout during later refreshes", async () => {
  const { native, client } = fixture();
  const first = deferred<groups.GroupTextPage>();
  const refreshed = deferred<groups.GroupTextPage>();
  const message: groups.GroupTextMessage = { id: bytes(32),
    position: { group: row.group, incarnation: bytes(24), position: 1n }, author: identity.publicHash,
    text: "Keep this message visible", outgoing: true, receivedCount: 0n, recipientCount: 1n, sentAtMillis: 0n, membershipChanged: false };
  native.messages.mockReturnValueOnce(first.promise).mockReturnValueOnce(refreshed.promise);
  const rendered = await render(<History client={client} conversation={row} contacts={[]} revision={1} active />);
  expect(screen.getByLabelText("Loading messages")).toBeTruthy();
  await act(async () => { first.resolve({ messages: [message], visitedHeads: [], currentIncarnation: bytes(24) }); });
  const displayed = screen.getByText(message.text);
  expect(screen.queryByLabelText("Loading messages")).toBeNull();

  await rendered.rerender(<History client={client} conversation={{ ...row, revision: 2n }} contacts={[]} revision={2} active />);
  expect(native.messages).toHaveBeenCalledTimes(2);
  expect(screen.getByText(message.text)).toBe(displayed);
  expect(screen.queryByLabelText("Loading messages")).toBeNull();
  await act(async () => { refreshed.resolve({ messages: [{ ...message, receivedCount: 1n }], visitedHeads: [], currentIncarnation: bytes(24) }); });
  expect(screen.getByText(message.text)).toBe(displayed);
  expect(screen.getByText("Saved by 1 of 1 devices")).toBeTruthy();
});

test("known unread badges stay visible until a refreshed page check finishes", async () => {
  const { native, client, observation } = fixture();
  const changed = deferred<void>();
  observation.next.mockReturnValueOnce(changed.promise);
  const message: groups.GroupTextMessage = { id: bytes(32), position: { group: row.group, incarnation: bytes(24), position: 1n },
    author: contact.claims.identityHash, text: "Unread message", outgoing: false, receivedCount: 0n, recipientCount: 1n, sentAtMillis: 0n, membershipChanged: false };
  native.messages.mockResolvedValue({ messages: [message], visitedHeads: [], currentIncarnation: bytes(24) });
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  const badge = screen.getByText("Unread messages");
  native.conversations.mockResolvedValue({ items: [{ ...summary }], next: undefined });
  const refreshed = deferred<groups.GroupTextPage>();
  native.messages.mockReturnValueOnce(refreshed.promise);
  await act(async () => { changed.resolve(); });
  expect(native.messages).toHaveBeenCalledTimes(2);
  expect(screen.getByText("Unread messages")).toBe(badge);
  await act(async () => { refreshed.resolve({ messages: [], visitedHeads: [], currentIncarnation: bytes(24) }); });
  expect(screen.queryByText("Unread messages")).toBeNull();
});

test("native changes update the conversation list without a routine refresh control", async () => {
  const { native, client, observation } = fixture();
  const changed = deferred<void>();
  observation.next.mockReturnValueOnce(changed.promise);
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  expect(screen.queryByText("Refresh conversations")).toBeNull();
  expect(screen.queryByText("Refresh conversations page")).toBeNull();
  expect(screen.queryByLabelText("Conversations page 1")).toBeNull();
  native.conversations.mockResolvedValue({ items: [summary, { ...summary, group: bytes(32, 9), title: "Arrived automatically" }], next: undefined });
  await act(async () => { changed.resolve(); });
  expect(screen.getByRole("button", { name: "Arrived automatically · 2 members · Active" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Garden · 2 members · Active" })).toBeTruthy();
  expect(native.subscribe).toHaveBeenCalledTimes(1);
});

test("failed observation offers a retry that reconnects automatic updates", async () => {
  const { native, client } = fixture();
  native.subscribe.mockRejectedValueOnce(new Error("Observation unavailable"));
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  expect(screen.getByText("Observation unavailable")).toBeTruthy();
  await press("Retry conversations");
  expect(screen.queryByText("Observation unavailable")).toBeNull();
  expect(screen.getByRole("button", { name: "Garden · 2 members · Active" })).toBeTruthy();
  expect(native.subscribe).toHaveBeenCalledTimes(2);
  expect(screen.queryByText("Retry conversations")).toBeNull();
});

test("failed page and detail reads retain their targeted retry paths", async () => {
  const { native, client } = fixture();
  native.conversations.mockRejectedValueOnce(new Error("Catalog unavailable"));
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  expect(screen.getByText("Catalog unavailable")).toBeTruthy();
  await press("Retry conversations page");
  native.conversation.mockRejectedValueOnce(new Error("Detail unavailable"));
  await press("Garden · 2 members · Active");
  expect(screen.getByText("Detail unavailable")).toBeTruthy();
  await press("Retry conversation");
  expect(screen.queryByText("Detail unavailable")).toBeNull();
  expect(screen.getByLabelText("Message")).toBeTruthy();
});

test.each([
  ["list", "Review invitation to Invite"], ["details", "Remove Alex"], ["activity", "Remove from history"],
] as const)("loaded %s controls stay enabled through background reads", async (route, label) => {
  const { native, client, observation } = fixture();
  const changed = deferred<void>(); observation.next.mockReturnValueOnce(changed.promise);
  const invitation: groups.GroupInvitation = { id: bytes(32, 7), group: row.group, title: "Invite", inviter: contact.claims.identityHash,
    recipient: identity.publicHash, profile, outgoing: false, state: groups.GroupInvitationState.Pending,
    expiresAtSecs: 9000000000n, waitingForDelivery: false, canCancel: false, canDismiss: false };
  const operation = result(bytes(32, 33), new groups.GroupIntent.Create({ title: "Saved result", profile }));
  native.invitations.mockResolvedValue({ items: [invitation], next: undefined });
  native.operations.mockResolvedValue([operation]);
  await render(<Messaging screen={route} group={"05".repeat(32)} identity={identity} client={client} contacts={[contact]} />);
  const control = screen.getByRole("button", { name: label });
  const detail = deferred<groups.GroupConversation>(), offers = deferred<groups.GroupInvitationPage>();
  const activity = deferred<groups.GroupOperation[]>(), roster = deferred<groups.GroupMemberPage>();
  native.conversation.mockReturnValueOnce(detail.promise); native.invitations.mockReturnValueOnce(offers.promise);
  native.operations.mockReturnValueOnce(activity.promise); native.members.mockReturnValueOnce(roster.promise);
  await act(async () => { changed.resolve(); });
  expect(screen.getByRole("button", { name: label })).toBe(control); expect(control).toBeEnabled();
  await act(async () => {
    detail.resolve(row); offers.resolve({ items: [invitation], next: undefined });
    activity.resolve([operation]); roster.resolve({ members, control: row.control, next: undefined });
  });
  expect(control).toBeEnabled(); expect(native.submit).not.toHaveBeenCalled(); expect(native.dismissOperation).not.toHaveBeenCalled();
});

test("lost reply keeps exact request and resumes without minting another ID", async () => {
  const { native, client } = fixture();
  native.submit.mockRejectedValueOnce(new Error("Lost native reply"));
  await render(<Messaging screen="create" identity={identity} client={client} contacts={[]} />);
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "Roof");
  await press("Create conversation");
  expect(screen.getByText("Lost native reply")).toBeTruthy();
  expect(screen.queryByText("Continue without the result")).toBeNull();
  expect(screen.getByText("Check result")).toBeEnabled();
  await press("Check result");
  expect(native.submit).toHaveBeenCalledTimes(2);
  expect(native.submit.mock.calls[1]?.[0]).toEqual(native.submit.mock.calls[0]?.[0]);
  expect(native.submit.mock.calls[1]?.[1]).toEqual(native.submit.mock.calls[0]?.[1]);
  expect(groups.createRequestId).toHaveBeenCalledTimes(1);
  expect(screen.queryByText("Check result")).toBeNull();
});

test("saved action remains recoverable when invitation snapshot fails", async () => {
  const { native, client } = fixture();
  const draft = captureInput(identity, new groups.GroupIntent.Create({ title: "Saved", profile }));
  native.submit.mockRejectedValueOnce(new Error("Lost reply"));
  await expect(submitInput(client, identity, draft, { clock: new protocol.GroupClockReading.Unknown() })).rejects.toThrow();
  native.invitations.mockRejectedValue(new Error("Invitation read failed"));
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  expect(screen.getByText("Check result")).toBeEnabled();
  await press("Check result");
  expect(native.submit.mock.calls[1]?.[0]).toEqual(inputId(draft));
});

test("identity replacement cannot redirect an accepted action or let its late reply edit the new form", async () => {
  const first = fixture(); const late = deferred<groups.GroupOperation>();
  first.native.submit.mockReturnValue(late.promise);
  const rendered = await render(<Messaging screen="create" identity={identity} client={first.client} contacts={[]} />);
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "Original");
  await press("Create conversation");
  const replacement = { ...identity, id: { ...identity.id, sequence: 2n }, publicHash: bytes(16, 9) };
  const second = fixture(replacement);
  await rendered.rerender(<Messaging screen="create" identity={replacement} client={second.client} contacts={[]} />);
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "New identity draft");
  const call = first.native.submit.mock.calls[0]!;
  await act(async () => { late.resolve(result(call[0], call[1])); });
  expect(second.native.submit).not.toHaveBeenCalled();
  expect(screen.getByLabelText("New conversation title")).toHaveDisplayValue("New identity draft");
});

test("empty text page retains its cursor; device receipts never claim human reading", async () => {
  const { native, client } = fixture();
  const cursor = { group: row.group, incarnation: bytes(24), before: 9n };
  native.messages.mockImplementation(async (_group, before, limit) => {
    if (limit === 100) return { messages: [], visitedHeads: [], currentIncarnation: bytes(24) };
    if (before === undefined) return { messages: [], visitedHeads: [], currentIncarnation: bytes(24), nextBefore: cursor };
    return { messages: [{ id: bytes(32), position: { group: row.group, incarnation: bytes(24), position: 8n }, author: identity.publicHash,
      text: "Earlier history", outgoing: true, receivedCount: 1n, recipientCount: 2n, sentAtMillis: 0n, membershipChanged: true }], visitedHeads: [], currentIncarnation: bytes(24) };
  });
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  await press("Garden · 2 members · Active");
  expect(screen.getByText("Load earlier messages")).toBeEnabled();
  await press("Load earlier messages");
  expect(native.messages.mock.calls.filter((call) => call[2] === 50)[1]?.[1]).toEqual(cursor);
  expect(screen.getByText("Earlier history")).toBeTruthy();
  expect(screen.getByText("Saved by 1 of 2 devices · membership changed")).toBeTruthy();
});

test("owner removal and Close use canonical permissions and separate confirmations", async () => {
  const { native, client } = fixture();
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[contact]} />);
  await press("Garden · 2 members · Active");
  expect(screen.queryByText("Leave group")).toBeNull();
  expect(screen.queryByText("Delete local history")).toBeNull();
  await menu("Conversation options", "Conversation info");
  expect(screen.queryByText("Leave group")).toBeNull();
  expect(screen.queryByText("Delete local history")).toBeNull();
  await press("Remove Alex");
  expect(native.submit).not.toHaveBeenCalled();
  expect(screen.getByText(/does not erase copies already held/)).toBeTruthy();
  await press("Confirm: Remove member");
  expect(native.submit.mock.calls[0]?.[1]).toEqual(new groups.GroupIntent.RemoveMember({ group: row.group, member: members[1]!.endpoint }));
  await press("Manage conversation");
  await press("Close group for everyone");
  await press("Manage conversation");
  expect(screen.getByText(/permanently closes the group for everyone/)).toBeTruthy();
  await press("Confirm: Close group for everyone");
  expect(native.submit.mock.calls[1]?.[1]).toEqual(new groups.GroupIntent.Close({ group: row.group }));
});

test("member Leave and local Delete remain separate actions and explanations", async () => {
  const { native, client } = fixture();
  native.conversation.mockResolvedValue({ ...row, capabilities: { ...capabilities, canClose: false, canInvite: false, canRemoveMembers: false, canLeave: true, canDeleteLocal: true } });
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  await press("Garden · 2 members · Active");
  await menu("Conversation options", "Conversation info");
  await press("Manage conversation");
  await press("Leave group");
  await press("Manage conversation"); expect(screen.getByText(/existing local history stays/)).toBeTruthy();
  await press("Keep group unchanged");
  await press("Manage conversation");
  await press("Delete local history");
  await press("Manage conversation"); expect(screen.getByText(/does not revoke membership/)).toBeTruthy();
  await press("Confirm: Delete local history");
  expect(native.submit.mock.calls[0]?.[1]).toEqual(new groups.GroupIntent.DeleteLocal({ group: row.group }));
});

test("incoming invitation uses exact invitation ID for acceptance", async () => {
  const { native, client } = fixture();
  const invitation: groups.GroupInvitation = { id: bytes(32, 7), group: row.group, title: "Invite", inviter: contact.claims.identityHash, recipient: identity.publicHash,
    profile, outgoing: false, state: groups.GroupInvitationState.Pending, expiresAtSecs: 9000000000n, waitingForDelivery: false, canCancel: false, canDismiss: false };
  native.invitations.mockResolvedValue({ items: [invitation], next: undefined });
  native.invitation.mockResolvedValue(invitation);
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[contact]} />);
  expect(screen.queryByRole("button", { name: "Accept Invite" })).toBeNull();
  await press("Review invitation to Invite");
  expect(native.invitation).toHaveBeenCalledWith(invitation.id, expect.any(BigInt), expect.objectContaining({ signal: expect.any(AbortSignal) }));
  expect(native.submit).not.toHaveBeenCalled();
  await press("Accept Invite");
  expect(native.submit.mock.calls[0]?.[1]).toEqual(new groups.GroupIntent.Respond({ invitation: invitation.id, accept: true }));
});

test("sending uses the canonical text publication and definite capacity refusal leaves cleanup available", async () => {
  const { native, client } = fixture();
  native.submit.mockImplementation(async (id, intent) => ({ ...result(id, intent), state: new groups.GroupOperationState.Rejected(protocol.GroupRejectionReason.StorageCapacity) }));
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  await press("Garden · 2 members · Active");
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Message"), "Hello group"); });
  await press("Send message");
  expect(groups.textPublication).toHaveBeenCalledWith(row.group, "Hello group");
  const published = native.submit.mock.calls[0]?.[1];
  expect(published?.tag).toBe(groups.GroupIntent_Tags.Publish);
  expect(screen.getByLabelText("Message")).toHaveDisplayValue("Hello group");
  expect(screen.queryByText("Check result")).toBeNull();
  await menu("Conversation options", "Conversation info");
  await press("Manage conversation");
  expect(screen.getByText("Close group for everyone")).toBeEnabled();
});

test("checking an older uncertain message preserves a newly typed identical draft", async () => {
  const { native, client } = fixture();
  native.submit.mockImplementation(async (id, intent) => {
    const operation = { ...result(id, intent), state: new groups.GroupOperationState.Uncertain(new protocol.GroupApiError.Unavailable()) };
    native.operations.mockResolvedValue([operation]);
    return operation;
  });
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  await press("Garden · 2 members · Active");
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Message"), "OK"); });
  await press("Send message");
  expect(screen.getByLabelText("Message")).toHaveDisplayValue("");
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Message"), "OK"); });
  await menu("Conversation options", "Action history");
  await press("Check status");
  await press("Open related conversation");
  expect(screen.getByLabelText("Message")).toHaveDisplayValue("OK");
  expect(native.submit).toHaveBeenCalledTimes(2);
  expect(native.submit.mock.calls[1]?.[0]).toEqual(native.submit.mock.calls[0]?.[0]);
  expect(groups.createRequestId).toHaveBeenCalledTimes(1);
});

test("snapshot recovery leaves an identical current draft intact", async () => {
  const { native, client } = fixture();
  const view = await render(<Messaging screen="conversation" group={"05".repeat(32)} identity={identity} client={client} contacts={[]} />);
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Message"), "OK"); });
  await view.unmount();
  const intent = new groups.GroupIntent.Publish(groups.textPublication(row.group, "OK"));
  const saved = captureInput(identity, intent);
  native.submit.mockRejectedValueOnce(new Error("lost reply"));
  await expect(submitInput(client, identity, saved, { clock: new protocol.GroupClockReading.Unknown() })).rejects.toThrow("lost reply");
  const evidence = deferred<groups.GroupOperation | undefined>();
  native.operation.mockReturnValue(evidence.promise);
  await render(<Messaging screen="conversation" group={"05".repeat(32)} identity={identity} client={client} contacts={[]} />);
  expect(screen.getByLabelText("Message")).toHaveDisplayValue("OK");
  await act(async () => { evidence.resolve(result(inputId(saved), intent)); });
  expect(screen.getByLabelText("Message")).toHaveDisplayValue("OK");
  expect(native.submit).toHaveBeenCalledTimes(1);
});

test("expired input requires explicit release, preserves failed storage and never replays", async () => {
  const { native, client, observation } = fixture();
  const changed = deferred<void>();
  observation.next.mockReturnValueOnce(changed.promise);
  native.submit.mockRejectedValue(new protocol.GroupApiError.Retry({ reason: protocol.GroupRetryError.Expired }));
  await render(<Messaging screen="create" identity={identity} client={client} contacts={[]} />);
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "Roof");
  await press("Create conversation");
  expect(screen.getByText(/It may already have completed/)).toBeTruthy();
  expect(screen.getByText("Create conversation")).toBeDisabled();
  await act(async () => { changed.resolve(); });
  expect(screen.getByText("Continue without the result")).toBeEnabled();
  jest.mocked(AsyncStorage.removeItem).mockRejectedValueOnce(new Error("cannot release saved input"));
  await press("Continue without the result");
  expect(screen.getByText("cannot release saved input")).toBeTruthy();
  expect(screen.getByText("Check result")).toBeEnabled();
  expect(screen.getByText("Create conversation")).toBeDisabled();
  await press("Continue without the result");
  expect(screen.queryByText("Check result")).toBeNull();
  expect(screen.getByText("Create conversation")).toBeEnabled();
  expect(screen.getByLabelText("New conversation title")).toHaveDisplayValue("Roof");
  expect(native.submit).toHaveBeenCalledTimes(1);
  expect(groups.createRequestId).toHaveBeenCalledTimes(1);
});

test("expiry release choice is retired on selection and client replacement", async () => {
  const first = fixture();
  first.native.submit.mockRejectedValue(new protocol.GroupApiError.Retry({ reason: protocol.GroupRetryError.Expired }));
  const view = await render(<Messaging screen="create" identity={identity} client={first.client} contacts={[]} />);
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "Roof");
  await press("Create conversation");
  expect(screen.getByText("Continue without the result")).toBeTruthy();
  await view.rerender(<Messaging screen="conversation" group={"05".repeat(32)} identity={identity} client={first.client} contacts={[]} />);
  expect(screen.queryByText("Continue without the result")).toBeNull();
  await press("Check result");
  expect(screen.getByText("Continue without the result")).toBeTruthy();
  const second = fixture();
  await view.rerender(<Messaging screen="conversation" group={"05".repeat(32)} identity={identity} client={second.client} contacts={[]} />);
  expect(screen.queryByText("Continue without the result")).toBeNull();
  expect(screen.getByText("Check result")).toBeEnabled();
  expect(second.native.submit).not.toHaveBeenCalled();
});

test("a retained acknowledgement on restart unlocks input without submitting another request", async () => {
  const { native, client } = fixture();
  const draft = captureInput(identity, new groups.GroupIntent.Create({ title: "Already saved", profile }));
  native.submit.mockRejectedValueOnce(new Error("Lost reply"));
  await expect(submitInput(client, identity, draft, { clock: new protocol.GroupClockReading.Unknown() })).rejects.toThrow();
  native.operation.mockResolvedValue(result(inputId(draft), new groups.GroupIntent.Create({ title: "Already saved", profile })));
  await render(<Messaging screen="create" identity={identity} client={client} contacts={[]} />);
  expect(screen.queryByText("Check result")).toBeNull();
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "Next group");
  expect(screen.getByText("Create conversation")).toBeEnabled();
  expect(native.operation).toHaveBeenCalledWith(inputId(draft), expect.anything());
  expect(native.submit).toHaveBeenCalledTimes(1);
  expect([...data.keys()].filter((key) => key.startsWith("prns.app.v1.group-input."))).toHaveLength(0);
});


test("an exact conversation route restores its draft without crossing identities", async () => {
  const { client } = fixture();
  const first = await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  await press("Garden · 2 members · Active");
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Message"), "Keep my draft"); });
  await first.unmount();
  const second = await render(<Messaging screen="conversation" group={"05".repeat(32)} identity={identity} client={client} contacts={[]} />);
  expect(screen.getByLabelText("Message").props.value).toBe("Keep my draft");
  const other = { ...identity, id: { ...identity.id, sequence: 2n } };
  const otherClient = fixture(other).client;
  await second.rerender(<Messaging screen="list" identity={other} client={otherClient} contacts={[]} />);
  expect(screen.queryByLabelText("Message")).toBeNull();
  await press("Garden · 2 members · Active");
  expect(screen.getByLabelText("Message").props.value).toBe("");
});


test("local viewed markers advance only while the conversation is open in an active panel", async () => {
  const { native, client } = fixture();
  const token = { group: row.group, incarnation: bytes(24), position: 5n };
  native.messages.mockImplementation(async () => ({ messages: [{ id: bytes(32), position: { ...token }, author: contact.claims.identityHash,
    text: "Incoming", outgoing: false, receivedCount: 0n, recipientCount: 1n, sentAtMillis: 0n, membershipChanged: false }],
    visitedHeads: [{ ...token }], currentIncarnation: token.incarnation }));
  const form = await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  expect(screen.getByText("Unread messages")).toBeTruthy();
  expect([...data.keys()].some((key) => key.startsWith("prns.ui.viewed."))).toBe(false);
  await press("Garden · 2 members · Active");
  expect(screen.queryByText("Unread messages")).toBeNull();
  const viewedKey = [...data.keys()].find((key) => key.startsWith("prns.ui.viewed."))!;
  expect(Object.values(JSON.parse(data.get(viewedKey)!).heads)).toEqual(["5"]);
  await form.rerender(<Messaging screen="list" identity={identity} client={client} contacts={[]} active={false} />);
  token.position = 8n;
  expect(Object.values(JSON.parse(data.get(viewedKey)!).heads)).toEqual(["5"]);
  await form.rerender(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  expect(Object.values(JSON.parse(data.get(viewedKey)!).heads)).toEqual(["8"]);
});


test("a rendered page is marked viewed after delayed local marker loading", async () => {
  const { native, client } = fixture();
  const gate = deferred<string | null>();
  const fallback = jest.mocked(AsyncStorage.getItem).getMockImplementation()!;
  jest.mocked(AsyncStorage.getItem).mockImplementation((key) => key.startsWith("prns.ui.viewed.") ? gate.promise : fallback(key));
  const token = { group: row.group, incarnation: bytes(24), position: 7n };
  native.messages.mockResolvedValue({ messages: [], visitedHeads: [token], currentIncarnation: token.incarnation });
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  await press("Garden · 2 members · Active");
  expect([...data.keys()].some((key) => key.startsWith("prns.ui.viewed."))).toBe(false);
  await act(async () => { gate.resolve(null); });
  const viewedKey = [...data.keys()].find((key) => key.startsWith("prns.ui.viewed."))!;
  expect(Object.values(JSON.parse(data.get(viewedKey)!).heads)).toEqual(["7"]);
});


test.each(["reply", "failure"])("same-identity client replacement ignores an old submit %s and preserves the new waiter", async (outcome) => {
  const first = fixture(); const oldReply = deferred<groups.GroupOperation>();
  first.native.submit.mockReturnValue(oldReply.promise);
  const view = await render(<Messaging screen="create" identity={identity} client={first.client} contacts={[]} />);
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "Keep this draft");
  await press("Create conversation");
  const original = first.native.submit.mock.calls[0]!;
  const second = fixture(); const newReply = deferred<groups.GroupOperation>();
  second.native.submit.mockReturnValue(newReply.promise);
  await view.rerender(<Messaging screen="create" identity={identity} client={second.client} contacts={[]} />);
  expect(first.observation.uniffiDestroy).toHaveBeenCalledTimes(1);
  expect(screen.getByLabelText("New conversation title")).toHaveDisplayValue("Keep this draft");
  expect(screen.getByText("Check result")).toBeEnabled();
  await press("Check result");
  expect(second.native.submit.mock.calls[0]?.slice(0, 2)).toEqual(original.slice(0, 2));
  expect(groups.createRequestId).toHaveBeenCalledTimes(1);
  const snapshots = second.native.conversations.mock.calls.length;
  await act(async () => {
    if (outcome === "reply") oldReply.resolve(result(original[0], original[1]));
    else oldReply.reject(new Error("Previous client failed"));
  });
  expect(screen.queryByText("Previous client failed")).toBeNull();
  expect(screen.getByLabelText("New conversation title")).toHaveDisplayValue("Keep this draft");
  expect(screen.getByText("Check result")).toBeDisabled();
  expect(second.native.conversations).toHaveBeenCalledTimes(snapshots);
  await act(async () => { newReply.resolve(result(original[0], original[1])); });
  expect(screen.queryByText("Check result")).toBeNull();
  expect(screen.getByLabelText("New conversation title")).toHaveDisplayValue("");
});

test.each(["reply", "failure"])("same-identity client replacement ignores an old dismiss %s without unlocking the new action", async (outcome) => {
  const operation = result(bytes(32, 42), new groups.GroupIntent.Create({ title: "Recorded", profile }));
  const first = fixture(); const oldReply = deferred<void>();
  first.native.operations.mockResolvedValue([operation]);
  first.native.dismissOperation.mockReturnValue(oldReply.promise);
  const view = await render(<Messaging screen="activity" identity={identity} client={first.client} contacts={[]} />);
  await press("Remove from history");
  const second = fixture(); const newReply = deferred<void>();
  second.native.operations.mockResolvedValue([operation]);
  second.native.dismissOperation.mockReturnValue(newReply.promise);
  await view.rerender(<Messaging screen="activity" identity={identity} client={second.client} contacts={[]} />);
  expect(screen.getByText("Remove from history")).toBeEnabled();
  await press("Remove from history");
  const snapshots = second.native.conversations.mock.calls.length;
  await act(async () => {
    if (outcome === "reply") oldReply.resolve();
    else oldReply.reject(new Error("Previous dismissal failed"));
  });
  expect(screen.queryByText("Previous dismissal failed")).toBeNull();
  expect(screen.getByText("Remove from history")).toBeDisabled();
  expect(second.native.conversations).toHaveBeenCalledTimes(snapshots);
  await act(async () => { newReply.resolve(); });
  expect(screen.getByText("Remove from history")).toBeEnabled();
});

test("replacement retires native facts and confirmations while preserving selected conversation and text drafts", async () => {
  const first = fixture();
  const view = await render(<Messaging screen="list" identity={identity} client={first.client} contacts={[]} />);
  await press("Garden · 2 members · Active");
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Message"), "Unsent text"); });
  await menu("Conversation options", "Conversation info");
  await press("Manage conversation");
  await press("Close group for everyone");
  await press("Manage conversation");
  expect(screen.getByText("Confirm: Close group for everyone")).toBeTruthy();
  const second = fixture(); const rows = deferred<groups.GroupConversationPage>();
  const detail = deferred<groups.GroupConversation>();
  second.native.conversations.mockReturnValue(rows.promise);
  second.native.conversation.mockReturnValue(detail.promise);
  await view.rerender(<Messaging screen="list" identity={identity} client={second.client} contacts={[]} />);
  expect(screen.queryByRole("button", { name: "Garden · 2 members · Active" })).toBeNull();
  expect(screen.queryByText("Confirm: Close group for everyone")).toBeNull();
  expect(screen.queryByText("Close group for everyone")).toBeNull();
  await act(async () => { rows.resolve(summaries); detail.resolve(row); });
  await view.rerender(<Messaging screen="conversation" group={"05".repeat(32)} identity={identity} client={second.client} contacts={[]} />);
  expect(screen.getByLabelText("Message")).toHaveDisplayValue("Unsent text");
  expect(second.native.submit).not.toHaveBeenCalled();
});

test("a snapshot completing after client replacement cannot acknowledge the current saved input", async () => {
  const first = fixture();
  const saved = captureInput(identity, new groups.GroupIntent.Create({ title: "Retained", profile }));
  first.native.submit.mockRejectedValueOnce(new Error("Reply lost"));
  await expect(submitInput(first.client, identity, saved, { clock: new protocol.GroupClockReading.Unknown() })).rejects.toThrow();
  const oldResult = deferred<groups.GroupOperation | undefined>();
  first.native.operation.mockReturnValue(oldResult.promise);
  const view = await render(<Messaging screen="list" identity={identity} client={first.client} contacts={[]} />);
  const second = fixture();
  await view.rerender(<Messaging screen="list" identity={identity} client={second.client} contacts={[]} />);
  expect(screen.getByText("Check result")).toBeEnabled();
  const savedKey = [...data.keys()].find((key) => key.startsWith("prns.app.v1.group-input."))!;
  const original = data.get(savedKey);
  await act(async () => { oldResult.resolve(result(inputId(saved), new groups.GroupIntent.Create({ title: "Retained", profile }))); });
  expect(data.get(savedKey)).toBe(original);
  expect(screen.getByText("Check result")).toBeEnabled();
  expect(first.native.operation).toHaveBeenCalledTimes(1);
});


test.each(["reply", "failure"])("an old unread %s cannot repopulate facts after same-identity client replacement", async (outcome) => {
  const first = fixture(); const unread = deferred<groups.GroupTextPage>();
  first.native.messages.mockReturnValue(unread.promise);
  const view = await render(<Messaging screen="list" identity={identity} client={first.client} contacts={[]} />);
  expect(first.native.messages).toHaveBeenCalled();
  const second = fixture();
  await view.rerender(<Messaging screen="list" identity={identity} client={second.client} contacts={[]} />);
  await act(async () => {
    if (outcome === "reply") unread.resolve({ messages: [], currentIncarnation: bytes(24),
      visitedHeads: [{ group: row.group, incarnation: bytes(24), position: 99n }] });
    else unread.reject(new Error("Old unread check failed"));
  });
  expect(screen.queryByText("Unread messages")).toBeNull();
  expect(screen.queryByText("Unread status not checked")).toBeNull();
});

test("catalog paging and exact conversation routes do not load unrelated rosters or catalogs", async () => {
  const { native, client } = fixture();
  const second = { ...summary, group: bytes(32, 9), title: "Second page" };
  native.conversations.mockImplementation(async (after) => after === undefined
    ? { items: [summary], next: row.group } : { items: [second], next: undefined });
  const view = await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  await press("Next conversations");
  expect(screen.queryByRole("button", { name: "Garden · 2 members · Active" })).toBeNull();
  expect(screen.getByRole("button", { name: "Second page · 2 members · Active" })).toBeTruthy();
  expect(native.conversations.mock.calls.at(-1)?.[0]).toEqual(row.group);
  expect(native.conversations.mock.calls.every((call) => (call[1] as { maxRows: number }).maxRows === 32)).toBe(true);
  const reads = native.conversations.mock.calls.length;
  await view.rerender(<Messaging screen="conversation" group={"05".repeat(32)} identity={identity} client={client} contacts={[]} />);
  expect(native.conversation).toHaveBeenCalledWith(row.group, { maxRows: 32, maxBytes: 65536n }, expect.anything());
  expect(screen.getByText("Garden")).toBeTruthy();
  expect(native.conversations).toHaveBeenCalledTimes(reads);
  expect(native.members).not.toHaveBeenCalled(); expect(native.operations).not.toHaveBeenCalled();
});

test.each(["create", "invitation", "message", "details", "delivery"] as const)("the %s route keeps unrelated lists and unread scans inactive on native changes", async (selectedScreen) => {
  const { native, client, observation } = fixture();
  const changed = deferred<void>();
  observation.next.mockReturnValueOnce(changed.promise);
  await render(<RoutedMessaging screen={selectedScreen} group={"05".repeat(32)} invitation={"07".repeat(32)}
    incarnation={"01".repeat(24)} messageId={"08".repeat(32)} identity={identity} client={client} contacts={[]} />);
  expect(native.subscribe).toHaveBeenCalledTimes(1);
  const exactReads = native.conversation.mock.calls.length + native.invitation.mock.calls.length + native.message.mock.calls.length;
  await act(async () => { changed.resolve(); });
  expect(native.conversations).not.toHaveBeenCalled();
  expect(native.invitations).not.toHaveBeenCalled();
  expect(native.operations).not.toHaveBeenCalled();
  expect(native.messages).not.toHaveBeenCalled();
  expect(native.subscribe).toHaveBeenCalledTimes(1);
  if (selectedScreen !== "create") {
    expect(native.conversation.mock.calls.length + native.invitation.mock.calls.length + native.message.mock.calls.length).toBeGreaterThan(exactReads);
  }
});

test("action history reads its list context without scanning unread messages", async () => {
  const { native, client } = fixture();
  await render(<RoutedMessaging screen="activity" identity={identity} client={client} contacts={[]} />);
  expect(native.operations).toHaveBeenCalled();
  expect(native.conversations).toHaveBeenCalled();
  expect(native.invitations).toHaveBeenCalled();
  expect(native.messages).not.toHaveBeenCalled();
  expect(native.conversation).not.toHaveBeenCalled();
});

test("filtered empty catalog and invitation pages keep their native continuations", async () => {
  const { native, client } = fixture();
  native.conversations.mockImplementation(async (after) => after === undefined
    ? { items: [], next: bytes(32, 4) } : summaries);
  native.invitations.mockImplementation(async (_now, after) => ({ items: [], next: after === undefined ? bytes(32, 8) : undefined }));
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  expect(screen.getByLabelText("Next conversations")).toBeEnabled();
  expect(screen.getByLabelText("Next invitations")).toBeEnabled();
  await press("Next conversations"); await press("Next invitations");
  expect(native.conversations.mock.calls.at(-1)?.[0]).toEqual(bytes(32, 4));
  expect(native.invitations.mock.calls.at(-1)?.[1]).toEqual(bytes(32, 8));
  expect(screen.getByRole("button", { name: "Garden · 2 members · Active" })).toBeTruthy();
});

test("a short retained-operation page continues from its native last request ID", async () => {
  const { native, client } = fixture();
  const operation = result(bytes(32, 33), new groups.GroupIntent.Create({ title: "Short page", profile }));
  native.operations.mockImplementation(async (after) => after === undefined ? [operation] : []);
  await render(<Messaging screen="activity" identity={identity} client={client} contacts={[]} />);
  await press("Next activity");
  expect(native.operations.mock.calls.at(-1)?.[0]).toEqual(operation.request.id);
  expect(native.operations.mock.calls.at(-1)?.[1]).toEqual({ maxRows: 16, maxBytes: 1048576n });
  expect(screen.getByLabelText("Next activity")).toBeDisabled();
});

test("membership changes reset the member cursor and refresh exact detail", async () => {
  const { native, client } = fixture();
  let changed = false;
  const updated = { ...row, control: { ...row.control, head: bytes(32, 7) }, revision: 2n };
  native.conversation.mockImplementation(async () => changed ? updated : row);
  native.members.mockImplementation(async (_group, after) => {
    if (after !== undefined) changed = true;
    return { members: [members[0]!], next: changed ? undefined : members[0]!.endpoint,
      control: changed ? updated.control : row.control };
  });
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[]} />);
  await press("Garden · 2 members · Active"); await menu("Conversation options", "Conversation info");
  await press("Next members");
  expect(native.members.mock.calls.some((call) => call[1] !== undefined)).toBe(true);
  expect(native.members.mock.calls.at(-1)?.[1]).toBeUndefined();
  expect(screen.queryByLabelText("Members page 1")).toBeNull();
  expect(screen.getByText("You · owner")).toBeTruthy();
});

test("Create resolves an editable capacity before retaining input and preserves that profile on retry", async () => {
  const { native, client } = fixture();
  const selected = { ...profile, maxMembers: 512, maxPlaintextBytes: 32768 };
  jest.mocked(app.applicationGroupProfile).mockReturnValueOnce(selected);
  native.submit.mockRejectedValueOnce(new Error("Lost profile reply"));
  await render(<Messaging screen="create" identity={identity} client={client} contacts={[]} />);
  expect(screen.queryByLabelText("Group member capacity")).toBeNull();
  await press("Conversation options");
  expect(screen.getByLabelText("Group member capacity")).toHaveDisplayValue("256");
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "Larger selected group");
  await fireEvent.changeText(screen.getByLabelText("Group member capacity"), "512");
  await press("Conversation options");
  await press("Create conversation");
  expect(app.applicationGroupProfile).toHaveBeenCalledWith(512);
  const first = native.submit.mock.calls[0]!;
  expect(first[1]).toEqual(new groups.GroupIntent.Create({ title: "Larger selected group", profile: selected }));
  await press("Check result");
  expect(native.submit.mock.calls[1]?.[0]).toEqual(first[0]);
  expect(native.submit.mock.calls[1]?.[1]).toEqual(first[1]);
  expect(app.applicationGroupProfile).toHaveBeenCalledTimes(1);
});


test("member-capacity choice survives remount without becoming a global group limit", async () => {
  const { client } = fixture();
  const first = await render(<Messaging screen="create" identity={identity} client={client} contacts={[]} />);
  await press("Conversation options");
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Group member capacity"), "1024"); });
  await first.unmount();
  await render(<Messaging screen="create" identity={identity} client={client} contacts={[]} />);
  await press("Conversation options");
  expect(screen.getByLabelText("Group member capacity")).toHaveDisplayValue("1024");
  expect(app.applicationGroupProfile).not.toHaveBeenCalled();
});


test("a contact creates a conversation then requires an explicit invitation for that verified identity", async () => {
  const { native, client } = fixture(); const openGroup = jest.fn();
  // A newly created group has only its owner; the verified contact has not joined.
  native.conversation.mockResolvedValue({ ...row, memberCount: 1n });
  native.members.mockResolvedValue({ members: [members[0]!], control: row.control, next: undefined });
  native.submit.mockImplementation(async (id, intent) => ({ ...result(id, intent), state: new groups.GroupOperationState.Completed(
    intent.tag === groups.GroupIntent_Tags.Create ? new groups.GroupOutcome.Created(row.group) : new groups.GroupOutcome.Saved()) }));
  const view = await render(<Messaging identity={identity} client={client} contacts={[contact]} screen="create" recipient={contactKey(contact)} onOpenGroup={openGroup} />);
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "With Alex");
  await press("Create conversation");
  expect(native.submit).toHaveBeenCalledTimes(1);
  expect(native.submit.mock.calls[0]?.[1].tag).toBe(groups.GroupIntent_Tags.Create);
  expect(openGroup).toHaveBeenCalledWith("05".repeat(32));
  await view.rerender(<Messaging identity={identity} client={client} contacts={[contact]} screen="conversation" group={"05".repeat(32)} recipient={contactKey(contact)} onOpenGroup={openGroup} />);
  expect(native.submit).toHaveBeenCalledTimes(1);
  await menu("Conversation options", "Invite people");
  expect(native.submit).toHaveBeenCalledTimes(1);
  expect(screen.getByText("Review recipient")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Send invitation to Alex" })).toBeEnabled();
  await press("Send invitation to Alex");
  expect(native.submit.mock.calls[1]?.[1]).toEqual(new groups.GroupIntent.Invite({ group: row.group, recipient: contact.claims.identityHash }));
});

test("a removed contact cannot silently create or invite a replacement recipient", async () => {
  const { native, client } = fixture();
  await render(<Messaging identity={identity} client={client} contacts={[]} screen="create" recipient={contactKey(contact)} />);
  await fireEvent.changeText(screen.getByLabelText("New conversation title"), "With Alex");
  expect(screen.getByText("Create conversation")).toBeDisabled();
  expect(screen.getByText(/selected contact is not available/)).toBeTruthy();
  expect(native.submit).not.toHaveBeenCalled();
});


test("focused invitation Accept and Decline remain steady during an exact-detail background refresh", async () => {
  const { native, client, observation } = fixture();
  const changed = deferred<void>(); observation.next.mockReturnValueOnce(changed.promise);
  const invitation: groups.GroupInvitation = { id: bytes(32, 7), group: row.group, title: "Invite", inviter: contact.claims.identityHash,
    recipient: identity.publicHash, profile, outgoing: false, state: groups.GroupInvitationState.Pending,
    expiresAtSecs: 9000000000n, waitingForDelivery: false, canCancel: false, canDismiss: false };
  native.invitations.mockResolvedValue({ items: [invitation], next: undefined });
  native.invitation.mockResolvedValue(invitation);
  await render(<Messaging screen="list" identity={identity} client={client} contacts={[contact]} />);
  await press("Review invitation to Invite");
  const controls = ["Accept Invite", "Decline Invite"].map((name) => screen.getByRole("button", { name }));
  expect(native.invitation).toHaveBeenCalledWith(invitation.id, expect.any(BigInt), expect.objectContaining({ signal: expect.any(AbortSignal) }));
  const refreshed = deferred<groups.GroupInvitation | undefined>();
  native.invitation.mockReturnValueOnce(refreshed.promise);
  await act(async () => changed.resolve());
  expect(screen.queryByLabelText("Loading invitation")).toBeNull();
  for (const [index, name] of ["Accept Invite", "Decline Invite"].entries()) {
    expect(screen.getByRole("button", { name })).toBe(controls[index]); expect(controls[index]).toBeEnabled();
  }
  await act(async () => refreshed.resolve({ ...invitation }));
  for (const control of controls) expect(control).toBeEnabled();
  expect(native.submit).not.toHaveBeenCalled(); expect(native.dismissOperation).not.toHaveBeenCalled();
  expect(native.subscribe).toHaveBeenCalledTimes(1);
});


test("the conversation menu opens exact info and delivery without submitting or exposing rare controls inline", async () => {
  const { native, client } = fixture();
  const details = jest.fn(), delivery = jest.fn(), activity = jest.fn();
  await render(<Messaging identity={identity} client={client} contacts={[]} screen="conversation"
    group={"05".repeat(32)} onOpenDetails={details} onOpenDelivery={delivery} onOpenActivity={activity} />);
  expect(screen.getByRole("button", { name: "Send message" })).toHaveTextContent("Send");
  for (const label of ["Conversation info", "Connection activity", "Action history", "Close group for everyone"]) {
    expect(screen.queryByRole("button", { name: label })).toBeNull();
  }
  await menu("Conversation options", "Conversation info");
  expect(details).toHaveBeenCalledTimes(1);
  expect(details).toHaveBeenCalledWith("05".repeat(32));
  expect(delivery).not.toHaveBeenCalled();
  await menu("Conversation options", "Connection activity");
  expect(delivery).toHaveBeenCalledTimes(1);
  expect(delivery).toHaveBeenCalledWith("05".repeat(32));
  await menu("Conversation options", "Action history");
  expect(activity).toHaveBeenCalledTimes(1);
  expect(native.submit).not.toHaveBeenCalled();
  expect(native.dismissOperation).not.toHaveBeenCalled();
});

test("conversation list keeps New accessible and action history in its menu", async () => {
  const { native, client } = fixture();
  const create = jest.fn(), activity = jest.fn();
  await render(<Messaging identity={identity} client={client} contacts={[]} screen="list"
    onOpenCreate={create} onOpenActivity={activity} />);
  expect(screen.getByRole("button", { name: "New conversation" })).toHaveTextContent("New");
  expect(screen.queryByRole("button", { name: "Action history" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Review saved actions" })).toBeNull();
  await press("New conversation");
  expect(create).toHaveBeenCalledTimes(1);
  expect(activity).not.toHaveBeenCalled();
  await menu("Conversation options", "Action history");
  expect(activity).toHaveBeenCalledTimes(1);
  expect(native.submit).not.toHaveBeenCalled();
});


test("an exact message route reads retained history without a current conversation detail", async () => {
  const { native, client } = fixture();
  const incarnation = bytes(24, 7), recordId = bytes(32, 8);
  const message = jest.fn(async () => undefined), record = jest.fn(async () => undefined);
  Object.assign(native, { message, record });
  native.conversation.mockRejectedValue(new Error("There is no active conversation"));
  await render(<Messaging identity={identity} client={client} contacts={[contact]} screen="message"
    group={"05".repeat(32)} incarnation={"07".repeat(24)} messageId={"08".repeat(32)} />);
  expect(screen.getByText("This message is no longer available on this phone. It may have been deleted.")).toBeTruthy();
  expect(message).toHaveBeenCalledWith(row.group, incarnation, recordId, expect.any(Object), expect.objectContaining({ signal: expect.any(Object) }));
  expect(record).toHaveBeenCalledWith(row.group, incarnation, recordId, expect.any(Object), expect.objectContaining({ signal: expect.any(Object) }));
  expect(native.conversation).not.toHaveBeenCalled();
});
