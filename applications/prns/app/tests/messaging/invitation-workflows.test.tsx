import "./native-mock";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react-native";
import { prns_groups as groups } from "@prns/app-bindings";
import { hex } from "../../src/bytes";
import { type SavedContact } from "../../src/contacts";
import { Invite } from "../../src/messaging/Invite";
import { InvitationDetails } from "../../src/messaging/Invitations";
import { actionContext, actionDestination } from "../../src/messaging/action-context";

const bytes = (size: number, value = 1) => new Uint8Array(size).fill(value).buffer;
const contact: SavedContact = { uri: "already-verified-card", alias: "Alex Garden", claims: {
  publicIdentity: bytes(64), identityHash: bytes(16, 3), destinationHash: bytes(16, 4), displayName: "Alex", serviceNameHash: bytes(10),
} };
const other: SavedContact = { ...contact, alias: "Bea", claims: { ...contact.claims, identityHash: bytes(16, 9), displayName: "Beatrice" } };
const profile: groups.GroupProfile = { maxMembers: 256, maxRecordBodyBytes: 2054, maxPlaintextBytes: 16384,
  maxWireBytes: 65536, maxSnapshotBytes: 262144, maxEnrollmentFrameBytes: 524288 };
const conversation: groups.GroupConversation = { group: bytes(32, 5), title: "Garden", revision: 1n,
  control: { head: bytes(32), era: bytes(32, 2) }, lifecycle: groups.GroupLifecycle.Active,
  ownerIdentity: bytes(16, 2), capabilities: { canSend: true, canInvite: true, canRemoveMembers: true, canLeave: false, canClose: true, canDeleteLocal: false },
  memberCount: 1n, profile };
const member: groups.GroupConversationMember = { identity: contact.claims.identityHash, isLocal: false, isOwner: false,
  endpoint: { id: bytes(32, 3), incarnation: 1n } };
const invitation: groups.GroupInvitation = { id: bytes(32, 7), group: conversation.group, title: conversation.title, profile,
  inviter: bytes(16, 2), recipient: contact.claims.identityHash, expiresAtSecs: 2_000_000_000n,
  outgoing: true, state: groups.GroupInvitationState.Pending, waitingForDelivery: false, canCancel: true, canDismiss: false };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((yes) => { resolve = yes; }); return { promise, resolve }; }
function fixture() {
  const native = {
    members: jest.fn<Promise<groups.GroupMemberPage>, unknown[]>(async () => ({ members: [], control: conversation.control, next: undefined })),
    invitations: jest.fn<Promise<groups.GroupInvitationPage>, unknown[]>(async () => ({ items: [], next: undefined })),
  };
  return { native, client: native as unknown as groups.ClientLike };
}
function inviteProps(client: groups.ClientLike) {
  return { client, conversation, contacts: [contact, other], active: true, disabled: false, revision: 1, invite: jest.fn<void, [ArrayBuffer]>() };
}
function detailsProps(value = invitation) {
  return { invitation: value, contacts: [contact], disabled: false, respond: jest.fn(), cancel: jest.fn(), dismiss: jest.fn(), openGroup: jest.fn() };
}
function operation(intent: groups.GroupIntent, outcome: groups.GroupOutcome = new groups.GroupOutcome.Saved()): groups.GroupOperation {
  return { request: { id: bytes(32, 11), intent }, scope: new groups.GroupOperationScope.Create(), state: new groups.GroupOperationState.Completed(outcome) };
}
async function press(name: string) { await act(async () => { await fireEvent.press(screen.getByRole("button", { name })); }); }
afterEach(cleanup);

test("saved-contact search selects an explicit recipient review before sending the exact identity", async () => {
  const { client } = fixture(), props = inviteProps(client);
  await render(<Invite {...props} />);
  await act(async () => { await fireEvent.changeText(screen.getByLabelText("Search contacts to invite"), "garden"); });
  expect(screen.queryByRole("button", { name: "Choose Bea to invite" })).toBeNull();
  await press("Choose Alex Garden to invite");
  expect(screen.getByText("Review recipient")).toBeTruthy();
  expect(screen.queryByLabelText("Search contacts to invite")).toBeNull();
  expect(screen.getByRole("button", { name: "Choose another person" })).toBeTruthy();
  expect(screen.getByText(hex(contact.claims.identityHash))).toBeTruthy();
  expect(props.invite).not.toHaveBeenCalled();
  await press("Send invitation to Alex Garden");
  expect(props.invite).toHaveBeenCalledTimes(1);
  expect(props.invite).toHaveBeenCalledWith(contact.claims.identityHash);
});

test("an existing member is identified and cannot be invited again", async () => {
  const { client, native } = fixture(), props = inviteProps(client);
  native.members.mockResolvedValue({ members: [member], control: conversation.control, next: undefined });
  await render(<Invite {...props} />);
  await press("Choose Alex Garden to invite");
  expect(screen.getByText("Already a member")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Send invitation to Alex Garden" })).toBeDisabled();
  expect(native.invitations).not.toHaveBeenCalled();
  expect(props.invite).not.toHaveBeenCalled();
});

test.each([groups.GroupInvitationState.Pending, groups.GroupInvitationState.JoinRequested])("outgoing invitation state %s supplies a pending hint and prevents duplicates", async (state) => {
  const { client, native } = fixture(), props = inviteProps(client);
  native.invitations.mockResolvedValue({ items: [{ ...invitation, state }], next: undefined });
  await render(<Invite {...props} />);
  await press("Choose Alex Garden to invite");
  expect(screen.getByText("Invitation already pending")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Send invitation to Alex Garden" })).toBeDisabled();
});

test("raw identity entry requires 32 hexadecimal characters and explicit review", async () => {
  const { client } = fixture(), props = inviteProps(client);
  await render(<Invite {...props} contacts={[]} />);
  await press("Enter an identity address");
  const input = screen.getByLabelText("Invite identity address");
  for (const invalid of ["abcd", "g".repeat(32)]) {
    await act(async () => { await fireEvent.changeText(input, invalid); });
    expect(screen.getByRole("button", { name: "Review invitation" })).toBeDisabled();
  }
  const raw = "A1B2".repeat(8);
  await act(async () => { await fireEvent.changeText(input, raw); });
  await press("Review invitation");
  expect(screen.getByText(raw.toLowerCase())).toBeTruthy();
  expect(props.invite).not.toHaveBeenCalled();
  await press("Send invitation to Identity address");
  expect(hex(props.invite.mock.calls[0]![0])).toBe(raw.toLowerCase());
});

test("a membership page from a different control cannot authorize recipient confirmation", async () => {
  const { client, native } = fixture(), props = inviteProps(client);
  native.members.mockResolvedValue({ members: [], control: { ...conversation.control, head: bytes(32, 99) }, next: undefined });
  await render(<Invite {...props} />);
  await press("Choose Alex Garden to invite");
  expect(screen.getByRole("alert")).toHaveTextContent(/Membership changed/);
  expect(screen.getByRole("button", { name: "Send invitation to Alex Garden" })).toBeDisabled();
  expect(native.invitations).not.toHaveBeenCalled();
});

test("changing control rejects late membership replies from the previous review", async () => {
  const { client, native } = fixture(), props = inviteProps(client);
  const old = deferred<groups.GroupMemberPage>(), fresh = deferred<groups.GroupMemberPage>();
  native.members.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
  const view = await render(<Invite {...props} />);
  await press("Choose Alex Garden to invite");
  const updated = { ...conversation, control: { ...conversation.control, head: bytes(32, 88) } };
  await view.rerender(<Invite {...props} conversation={updated} />);
  await act(async () => { old.resolve({ members: [], control: conversation.control, next: undefined }); });
  expect(screen.getByRole("button", { name: "Send invitation to Alex Garden" })).toBeDisabled();
  await act(async () => { fresh.resolve({ members: [member], control: updated.control, next: undefined }); });
  expect(screen.getByText("Already a member")).toBeTruthy();
  expect(props.invite).not.toHaveBeenCalled();
});

test("replacing the client invalidates its completed review until the replacement answers", async () => {
  const old = fixture(), replacement = fixture(), props = inviteProps(old.client);
  const fresh = deferred<groups.GroupMemberPage>();
  replacement.native.members.mockReturnValueOnce(fresh.promise);
  const view = await render(<Invite {...props} />);
  await press("Choose Alex Garden to invite");
  expect(screen.getByRole("button", { name: "Send invitation to Alex Garden" })).toBeEnabled();
  await view.rerender(<Invite {...props} client={replacement.client} />);
  expect(screen.getByRole("button", { name: "Send invitation to Alex Garden" })).toBeDisabled();
  await act(async () => { fresh.resolve({ members: [member], control: conversation.control, next: undefined }); });
  expect(screen.getByText("Already a member")).toBeTruthy();
  expect(props.invite).not.toHaveBeenCalled();
});

test("a replaced client's late empty result cannot overwrite the replacement's member finding", async () => {
  const old = fixture(), replacement = fixture(), props = inviteProps(old.client);
  const pending = deferred<groups.GroupMemberPage>();
  old.native.members.mockReturnValueOnce(pending.promise);
  replacement.native.members.mockResolvedValue({ members: [member], control: conversation.control, next: undefined });
  const view = await render(<Invite {...props} />);
  await press("Choose Alex Garden to invite");
  await view.rerender(<Invite {...props} client={replacement.client} />);
  await act(async () => { pending.resolve({ members: [], control: conversation.control, next: undefined }); });
  expect(screen.getByText("Already a member")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Send invitation to Alex Garden" })).toBeDisabled();
  expect(props.invite).not.toHaveBeenCalled();
});

test("removing a saved contact after review prevents confirmation", async () => {
  const { client } = fixture(), props = inviteProps(client);
  const view = await render(<Invite {...props} />);
  await press("Choose Alex Garden to invite");
  await view.rerender(<Invite {...props} contacts={[other]} />);
  expect(screen.getByRole("alert")).toHaveTextContent(/This contact is no longer saved/);
  await press("Send invitation to Alex Garden");
  expect(props.invite).not.toHaveBeenCalled();
});

test.each([true, false])("expired invitation guidance respects outgoing=%s", async (outgoing) => {
  await render(<InvitationDetails {...detailsProps({ ...invitation, outgoing, state: groups.GroupInvitationState.Expired, canCancel: false, canDismiss: true })} />);
  expect(screen.getByText(outgoing ? "Send a new invitation if this person should still join." : "Ask the group owner for a new invitation.")).toBeTruthy();
  expect(screen.getByText(/^Expires /)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Accept Garden" })).toBeNull();
});

test.each([true, false])("saved join delivery guidance respects outgoing=%s", async (outgoing) => {
  await render(<InvitationDetails {...detailsProps({ ...invitation, outgoing, state: groups.GroupInvitationState.JoinRequested, waitingForDelivery: true })} />);
  expect(screen.getByText(outgoing
    ? "Joining will continue automatically when this phone’s pending deliveries finish or expire."
    : "Your acceptance is saved. The owner is finishing pending deliveries; joining will continue automatically.")).toBeTruthy();
});

test("Joined opens the exact invitation's conversation through its callback", async () => {
  const props = detailsProps({ ...invitation, outgoing: false, state: groups.GroupInvitationState.Joined, canCancel: false });
  await render(<InvitationDetails {...props} />);
  await press("Open conversation");
  expect(props.openGroup).toHaveBeenCalledTimes(1);
  expect(props.respond).not.toHaveBeenCalled();
  expect(props.cancel).not.toHaveBeenCalled();
});

test("saved action destinations retain exact created, invited, responded and off-page published IDs", () => {
  const created = bytes(32, 31), offPage = bytes(32, 32), invitationId = bytes(32, 33);
  const create = new groups.GroupIntent.Create({ title: "New garden", profile });
  expect(actionDestination(operation(create))).toEqual({ screen: "create" });
  expect(actionDestination(operation(create, new groups.GroupOutcome.Created(created)))).toEqual({ screen: "conversation", group: hex(created) });
  const invite = operation(new groups.GroupIntent.Invite({ group: offPage, recipient: contact.claims.identityHash }));
  expect(actionDestination(invite)).toEqual({ screen: "invite", group: hex(offPage) });
  expect(actionContext(invite, [], [], [contact])).toBe(`Alex Garden · Conversation ${hex(offPage).slice(0, 8)}`);
  const respond = operation(new groups.GroupIntent.Respond({ invitation: invitationId, accept: true }));
  expect(actionDestination(respond)).toEqual({ screen: "invitation", invitation: hex(invitationId) });
  expect(actionContext(respond, [], [], [])).toBe(`Invitation ${hex(invitationId).slice(0, 8)}`);
  const publish = operation(new groups.GroupIntent.Publish({ group: offPage, namespace: 1, schema: 1, body: bytes(3), targets: new groups.GroupTargets.OtherMembers() }));
  expect(actionDestination(publish)).toEqual({ screen: "conversation", group: hex(offPage) });
  expect(actionContext(publish, [], [], [])).toBe(`Conversation ${hex(offPage).slice(0, 8)}`);
});
