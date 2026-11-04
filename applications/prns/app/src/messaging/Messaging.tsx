import { Disclosure } from "../ui/Disclosure";
import { useAppPalette } from "../ui/theme";
import { ActionMenu, Badge, Button, ListRow, PageHeader, Panel, Text, TextInput } from "../ui/primitives";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { SetStateAction } from "react";
import { ActivityIndicator, Alert, StyleSheet, View } from "react-native";
import { prns_groups as groups, prns_chat_app as app } from "@prns/app-bindings";
import type { prns_identities_core as core } from "@prns/app-bindings";
import { identityKey } from "../identity-reference";
import { contactKey } from "../contacts";
import type { SavedContact } from "../contacts";
import { hex } from "../bytes";
import { acknowledgeInput, captureInput, deviceClock, encodeIntent, inputId, inputIntent, loadInput, sameInput, PendingInputConflict, InputRejected, InputExpired, releaseExpiredInput, submitInput } from "./input";
import type { SavedInput } from "./input";
import { actionName, contactName, errorMessage, operationStatus, terminal } from "./presentation";
import { History } from "./History";
import { MessageDetails } from "./MessageDetails";
import { usePaged } from "./usePaged";
import { useConversation, useInvitation, CATALOG_PAGE, OPERATION_PAGE } from "./useConversation";
import { Paging } from "./Paging";
import { Members } from "./Members";
import { Invite } from "./Invite";
import { InvitationDetails, InvitationRow } from "./Invitations";
import { DeliveryDetails } from "./DeliveryDetails";
import { actionContext, actionDestination } from "./action-context";
import { useConversationDraft } from "./drafts";
import { decodeViewed, viewedDefault, markViewed, unreadStatus } from "./viewed";
import type { Unread } from "./viewed";
import { useViewState, messagingDefault, decodeMessaging } from "../view-state";

export type MessagingScreen = "list" | "create" | "conversation" | "invite" | "invitation" | "activity" | "details" | "delivery" | "message";
export type MessagingProps = {
  identity: core.IdentityReference; client: groups.ClientLike; contacts: readonly SavedContact[]; active?: boolean;
  group?: string | undefined; onOpenGroup?: ((group: string) => void) | undefined;
  recipient?: string | undefined;
  incarnation?: string | undefined; messageId?: string | undefined;
  onOpenMessage?: ((group: string, incarnation: string, messageId: string) => void) | undefined;
  invitation?: string | undefined;
  onOpenInvitation?: ((id: string) => void) | undefined;
  onInviteGroup?: ((group: string) => void) | undefined;
  onOpenDetails?: ((group: string) => void) | undefined;
  onOpenDelivery?: ((group: string) => void) | undefined;
  onOpenActivity?: (() => void) | undefined;
  onOpenCreate?: (() => void) | undefined;
  screen: MessagingScreen;
};
/** A replacement immutable identity gets fresh state; drafts remain scoped to it. */
export function Messaging(props: MessagingProps) {
  return <IdentityMessaging key={identityKey(props.identity)} {...props} />;
}
type Confirmation = Readonly<{ intent: groups.GroupIntent; title: string; consequence: string }>;
function IdentityMessaging({ identity, client, contacts, active = true, group, recipient, onOpenGroup, invitation, onOpenInvitation, onInviteGroup, onOpenActivity, onOpenCreate, onOpenDetails, onOpenDelivery, incarnation, messageId, onOpenMessage, screen }: MessagingProps) {
  const styles = useStyles();
  const selectedContact = contacts.find((contact) => contactKey(contact) === recipient);
  const viewed = useViewState(`prns.ui.viewed.${identityKey(identity)}.v1`, viewedDefault, decodeViewed);
  const [unread, setUnread] = useState<Readonly<Record<string, Unread>>>({});
  const [pending, setPending] = useState<SavedInput>();
  const [expired, setExpired] = useState<InputExpired>();
  const [inputChecked, setInputChecked] = useState(false);
  const [inputBlocked, setInputBlocked] = useState(false);
  const view = useViewState(`prns.ui.messaging.${identityKey(identity)}.v2`, messagingDefault, decodeMessaging);
  const { title, memberCapacity } = view.value;
  const selected = group;
  useLayoutEffect(() => { setExpired(undefined); }, [client, selected]);
  const draft = useConversationDraft(identityKey(identity), selected);
  const clearMatching = draft.clearMatching;
  const draftNow = useRef({ group: selected, text: draft.text });
  useLayoutEffect(() => { draftNow.current = { group: selected, text: draft.text }; }, [selected, draft.text]);
  const changeView = view.change;
  const setSelected = useCallback((selected: string) => {
    onOpenGroup?.(selected);
  }, [onOpenGroup]);
  const setTitle = useCallback((update: SetStateAction<string>) => changeView((previous) => ({ ...previous,
    title: typeof update === "function" ? update(previous.title) : update })), [changeView]);
  const setMemberCapacity = useCallback((memberCapacity: string) => {
    // Persist editable decimal text; the owning resolver validates its value.
    if (/^[0-9]{0,10}$/.test(memberCapacity)) changeView((previous) => ({ ...previous, memberCapacity }));
  }, [changeView]);
  const [confirmation, setConfirmation] = useState<Confirmation>();
  const [observing, setObserving] = useState(false);
  const [verified, setVerified] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [watch, setWatch] = useState(0);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  const actionGeneration = useRef(0);
  const clientGeneration = useRef(0);
  const scope = identityKey(identity);
  const publicHash = hex(identity.publicHash);
  // The immutable capture also protects a late asynchronous continuation if a
  // caller reuses/mutates an input record instead of replacing the keyed panel.
  const [boundIdentity] = useState<core.IdentityReference>(() => ({
    id: { incarnation: identity.id.incarnation.slice(0), sequence: identity.id.sequence }, publicHash: identity.publicHash.slice(0),
  }));
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useLayoutEffect(() => {
    // A replacement client can belong to the same immutable identity. Keep its
    // drafts and exact saved input, but retire all facts and waiters from the
    // previous native client before it can publish into this view again.
    clientGeneration.current++;
    actionGeneration.current++;
    inFlight.current = false;
    setBusy(false); setVerified(false); setObserving(false); setInputChecked(false); setInputBlocked(false);
    setUnread({});
    setConfirmation(undefined); setError(undefined);
    return () => { clientGeneration.current++; };
  }, [client]);
  const accepted = useCallback((operation: groups.GroupOperation, submitted?: { group: string | undefined; text: string }) => {
    if (!mounted.current || operation.state.tag === groups.GroupOperationState_Tags.Rejected) return;
    const intent = operation.request.intent;
    if (intent.tag === groups.GroupIntent_Tags.Create) {
      setTitle((value) => value === intent.inner.title ? "" : value);
      if (operation.state.tag === groups.GroupOperationState_Tags.Completed && operation.state.inner[0].tag === groups.GroupOutcome_Tags.Created) {
        setSelected(hex(operation.state.inner[0].inner[0]));
      }
    }
    if (intent.tag === groups.GroupIntent_Tags.Publish && submitted !== undefined) {
      const key = hex(intent.inner[0].group);
      const candidate = submitted;
      if (candidate.group === key && candidate.text) {
        try {
          if (encodeIntent(new groups.GroupIntent.Publish(groups.textPublication(intent.inner[0].group, candidate.text))) === encodeIntent(intent)) {
            // The draft store compares the latest same-key text again. A newer
            // edit or a different selected group cannot be erased by this reply.
            void clearMatching(key, candidate.text).catch(() => { /* Hook retains and reports the unsaved draft. */ });
          }
        } catch { /* A new edit is not the exact submitted publication. */ }
      }
    }
  }, [setSelected, setTitle, clearMatching]);
  useEffect(() => {
    if (!active || !view.ready) return;
    setVerified(false); setObserving(false);
    const abort = new AbortController();
    const clientTicket = clientGeneration.current;
    const current = () => !abort.signal.aborted && clientTicket === clientGeneration.current;
    let subscription: groups.GroupObservationLike | undefined;
    const snapshot = async () => {
      const actionTicket = actionGeneration.current;
      try {
        const saved = await loadInput(boundIdentity);
        if (!current()) return;
        let unresolved = saved;
        if (saved !== undefined) {
          const known = await client.operation(inputId(saved), { signal: abort.signal });
          if (!current()) return;
          if (known !== undefined) {
            await acknowledgeInput(saved, known);
            unresolved = undefined;
            if (current()) accepted(known);
          }
          // Missing/pruned is deliberately not proof that no effect occurred.
        }
        if (!current()) return;
        if (actionTicket === actionGeneration.current && !inFlight.current) {
          setPending(unresolved); setInputBlocked(false); setInputChecked(true);
        }
      } catch (cause) {
        if (current() && actionTicket === actionGeneration.current && !inFlight.current) {
          setInputBlocked(true); setInputChecked(true); setError(errorMessage(cause));
        }
      }
      // Unreadable pending input fences fresh actions but not healthy history.
      if (current()) setRevision((value) => value + 1);
    };
    void (async () => {
      try {
        if (hex(await client.identity({ signal: abort.signal })) !== publicHash) throw new Error("The group client does not belong to this identity.");
        if (!current()) return;
        setVerified(true);
        subscription = await client.subscribe({ signal: abort.signal });
        if (!current()) return;
        setObserving(true); // All page/detail hooks remain gated until subscribed.
        await snapshot(); // Subscribe before snapshot: no missing-change window.
        while (current()) { await subscription.next({ signal: abort.signal }); if (current()) await snapshot(); }
      } catch (cause) { if (current()) setError(errorMessage(cause)); }
      finally {
        if (current()) setObserving(false);
        if (subscription !== undefined && "uniffiDestroy" in subscription && typeof subscription.uniffiDestroy === "function") subscription.uniffiDestroy();
      }
    })();
    return () => { abort.abort(); setObserving(false); };
  }, [client, boundIdentity, scope, publicHash, active, view.ready, watch, accepted]);

  const readConversations = useCallback(async (after: ArrayBuffer | undefined, signal: AbortSignal) => {
    const page = await client.conversations(after, CATALOG_PAGE, { signal });
    return { items: page.items, next: page.next };
  }, [client]);
  const readInvitations = useCallback(async (after: ArrayBuffer | undefined, signal: AbortSignal) => {
    const page = await client.invitations(BigInt(Math.floor(Date.now() / 1000)), after, CATALOG_PAGE, { signal });
    return { items: page.items, next: page.next };
  }, [client]);
  const readOperations = useCallback(async (after: ArrayBuffer | undefined, signal: AbortSignal) => {
    const items = await client.operations(after, OPERATION_PAGE, { signal });
    // This owning API returns Vec, not an explicit-next page. A short nonempty
    // page may be byte-limited; only an empty successful page proves the end.
    return { items, next: items.at(-1)?.request.id };
  }, [client]);
  const reading = active && verified && observing && view.ready;
  const listReading = reading && screen === "list";
  const contextReading = reading && (screen === "list" || screen === "activity");
  const catalog = usePaged({ owner: client, scope, active: contextReading, revision, read: readConversations });
  const offers = usePaged({ owner: client, scope, active: contextReading, revision, read: readInvitations });
  const activity = usePaged({ owner: client, scope, active: reading && screen === "activity", revision, read: readOperations });
  const selectedConversation = useConversation(client, selected, reading && (screen === "conversation" || screen === "invite" || screen === "details" || screen === "delivery"), revision);
  const selectedInvitation = useInvitation(client, invitation, reading && screen === "invitation", revision);
  const conversation = selectedConversation.value;
  const conversations = catalog.rows, invitations = offers.rows, operations = activity.rows;
  const refreshDetail = useCallback(() => setRevision((value) => value + 1), []);

  const run = async (intent: groups.GroupIntent, retained?: SavedInput) => {
    if (!active || !verified || inFlight.current) return;
    const ticket = clientGeneration.current;
    const current = () => mounted.current && ticket === clientGeneration.current;
    inFlight.current = true;
    actionGeneration.current++;
    setBusy(true); setError(undefined); setConfirmation(undefined); setExpired(undefined);
    let captured: SavedInput | undefined;
    // Recovery cannot claim a current draft, even when its text is identical.
    const submittedDraft = retained === undefined ? { ...draftNow.current } : undefined;
    try {
      captured = retained ?? captureInput(boundIdentity, intent);
      // Keep this exact input even when saving it or receiving its result fails.
      setPending(captured);
      const result = await submitInput(client, boundIdentity, captured, { clock: deviceClock() });
      if (current()) { setPending(undefined); accepted(result, submittedDraft); setWatch((value) => value + 1); }
    } catch (cause) {
      if (current()) {
        if (cause instanceof PendingInputConflict) setPending(cause.pending);
        if (cause instanceof InputRejected) setPending(undefined);
        if (cause instanceof InputExpired) setExpired(cause);
        setError(errorMessage(cause));
      }
    } finally { if (current()) { inFlight.current = false; setBusy(false); } }
  };
  const submit = (intent: groups.GroupIntent) => { if (pending === undefined && inputChecked && !inputBlocked && observing) void run(intent); };
  const resume = (operation: groups.GroupOperation) => {
    if (pending === undefined) void run(operation.request.intent, captureInput(boundIdentity, operation.request.intent, operation.request.id));
  };
  const releaseExpired = async () => {
    if (!active || !verified || inFlight.current || expired === undefined || pending === undefined || !sameInput(pending, expired.input)) return;
    const ticket = clientGeneration.current;
    const current = () => mounted.current && ticket === clientGeneration.current;
    inFlight.current = true; actionGeneration.current++;
    setBusy(true); setError(undefined);
    try {
      await releaseExpiredInput(client, boundIdentity, expired);
      if (current()) { setPending(undefined); setExpired(undefined); setWatch((value) => value + 1); }
    } catch (cause) {
      if (current()) {
        if (cause instanceof PendingInputConflict) { setPending(cause.pending); setExpired(undefined); }
        setError(errorMessage(cause));
      }
    } finally { if (current()) { inFlight.current = false; setBusy(false); } }
  };
  const dismiss = async (operation: groups.GroupOperation) => {
    if (inFlight.current || !active || !verified) return;
    const ticket = clientGeneration.current;
    const current = () => mounted.current && ticket === clientGeneration.current;
    inFlight.current = true; setBusy(true); setError(undefined);
    try { await client.dismissOperation(operation.request.id); if (current()) setWatch((value) => value + 1); }
    catch (cause) { if (current()) setError(errorMessage(cause)); }
    finally { if (current()) { inFlight.current = false; setBusy(false); } }
  };
  const changeViewed = viewed.change;
  const onViewed = useCallback((heads: readonly groups.GroupChangeToken[]) => {
    if (viewed.ready) changeViewed((previous) => markViewed(previous, heads));
  }, [changeViewed, viewed.ready]);
  useEffect(() => {
    if (!listReading || !viewed.ready) return;
    const abort = new AbortController();
    const ticket = clientGeneration.current;
    const current = () => !abort.signal.aborted && ticket === clientGeneration.current;
    // Keep known badges while refreshing, bounded to this visible page.
    setUnread((previous) => {
      const retained: Record<string, Unread> = {};
      for (const row of conversations) {
        const key = hex(row.group);
        if (previous[key] !== undefined) retained[key] = previous[key];
      }
      return retained;
    });
    void (async () => {
      const refreshed: Record<string, Unread> = {};
      for (const row of conversations) {
        if (!current()) return;
        try { refreshed[hex(row.group)] = await unreadStatus(client, row.group, viewed.value, abort.signal); }
        catch { if (!current()) return; refreshed[hex(row.group)] = "unchecked"; }
      }
      if (current()) setUnread(refreshed);
    })();
    return () => { abort.abort(); };
  }, [client, conversations, viewed.value, viewed.ready, listReading]);
  const unavailable = !active || !view.ready || !verified || !observing || !inputChecked || inputBlocked || busy || pending !== undefined;
  // Native submission rechecks permissions. A background read is not an action.
  const groupUnavailable = unavailable || conversation === undefined || selectedConversation.error !== undefined;
  const openInvitation = (id: string) => onOpenInvitation?.(id);
  const openInvite = (id: string) => onInviteGroup?.(id);
  const openAction = (operation: groups.GroupOperation) => {
    const destination = actionDestination(operation);
    if (destination.screen === "conversation") setSelected(destination.group);
    else if (destination.screen === "invite") openInvite(destination.group);
    else if (destination.screen === "invitation") openInvitation(destination.invitation);
    else onOpenCreate?.();
  };
  const ask = (intent: groups.GroupIntent, label: string, consequence: string) => setConfirmation({ intent, title: label, consequence });
  return <View style={styles.section}>
    {screen === "list" && <PageHeader title="Conversations" help={{ label: "About conversations", content: <Text>Create a conversation or accept an invitation. Your message history stays on this phone until you delete it.</Text> }}
      trailing={<ActionMenu label="Conversation options" items={[{ title: "Action history", onPress: () => onOpenActivity?.() }]} />}>
      <Button tone="primary" size="compact" icon="add" title="New" accessibilityLabel="New conversation" disabled={!active} onPress={onOpenCreate} />
    </PageHeader>}
    {viewed.error !== undefined && <View><Text accessibilityRole="alert" style={styles.error}>{viewed.error}</Text>
      <Button title="Retry saving or loading viewed messages" onPress={viewed.retry} />
      <Button title="Reset local viewed markers" onPress={viewed.discard} /></View>}
    {view.error !== undefined && <View><Text accessibilityRole="alert" style={styles.error}>{view.error}</Text>
      <Text>Your current edits remain in this view. Retry before leaving to keep them on this device.</Text>
      <Button title="Retry saving or loading conversation options" onPress={view.retry} />
      <Button title="Discard saved view…" onPress={() => Alert.alert("Discard this identity’s saved view?",
        "This clears the new conversation title and member-capacity choice. Conversation drafts, history and accepted actions stay intact.",
        [{ text: "Keep view", style: "cancel" }, { text: "Discard view", style: "destructive", onPress: view.discard }])} /></View>}
    {error !== undefined && <View style={styles.section}>
      <Text accessibilityRole="alert" style={styles.error}>{error}</Text>
      {(!observing || inputBlocked) && <Button title="Retry conversations" disabled={!active || busy}
        onPress={() => { setError(undefined); setWatch((value) => value + 1); }} />}
    </View>}
    {pending !== undefined && <View style={styles.notice}>
      <Text style={styles.title}>{actionName(inputIntent(pending))} needs a status check</Text>
      <Text>Check the result before trying this request again.</Text>
      <Button tone="primary" title="Check result" disabled={!active || !verified || busy} onPress={() => { void run(inputIntent(pending), pending); }} />
      {expired !== undefined && sameInput(pending, expired.input) && <View>
        <Text>This action was rejected as expired. It may already have completed. Check your conversations before repeating it.</Text>
        <Button title="Continue without the result" disabled={!active || !verified || busy} onPress={() => { void releaseExpired(); }} />
      </View>}
    </View>}
    {screen === "activity" && <View style={styles.section}>
      <PageHeader title="Action history" subtitle="Requests from this phone and their results" />
      {operations.map((operation) => <View key={hex(operation.request.id)} style={styles.card}>
        <Text style={styles.title}>{actionName(operation.request.intent)}</Text>
        <Text>{actionContext(operation, conversations, invitations, contacts)}</Text>
        <Badge warning={!terminal(operation)}>{operationStatus(operation)}</Badge>
        <Button tone="quiet" size="compact" title={actionDestination(operation).screen === "invitation" ? "Review invitation" : actionDestination(operation).screen === "create" ? "Review conversation creation" : "Open related conversation"}
          onPress={() => openAction(operation)} />
        {terminal(operation) ? <Button tone="quiet" size="compact" title="Remove from history" disabled={unavailable || !activity.loaded || activity.error !== undefined} onPress={() => { if (activity.loaded && activity.error === undefined) void dismiss(operation); }} />
          : <Button size="compact" title="Check status" disabled={unavailable || !activity.loaded || activity.error !== undefined} onPress={() => { if (activity.loaded && activity.error === undefined) resume(operation); }} />}
      </View>)}
      <Paging label="Activity" active={reading && !busy} page={activity} />
      <Text>Removing a result from this list keeps your messages and does not undo the request.</Text>
    </View>}
    {recipient !== undefined && selectedContact === undefined && <Text accessibilityRole="alert">The selected contact is not available in this identity’s address book. Return to Contacts and choose a saved contact.</Text>}
    {screen === "create" && <View style={styles.section}>
      <PageHeader title="New conversation" />
      {selectedContact !== undefined && <Text>Conversation for {selectedContact.alias || selectedContact.claims.displayName}. After creating it, invite this contact to join.</Text>}
      <TextInput accessibilityLabel="New conversation title" placeholder="Conversation name" maxLength={8192} value={title} onChangeText={setTitle} editable={!unavailable} style={styles.input} />
      <Disclosure label="Conversation options">
        <Text>Maximum members</Text>
        <Text>Choose room for the people you expect to invite. Larger groups need more space on everyone’s device.</Text>
        <TextInput accessibilityLabel="Group member capacity" keyboardType="number-pad" maxLength={10} value={memberCapacity} onChangeText={setMemberCapacity} editable={!unavailable} style={styles.input} />
      </Disclosure>
      <Button tone="primary" title="Create conversation" disabled={unavailable || !title.trim() || (recipient !== undefined && selectedContact === undefined)} onPress={() => {
        try {
          if (!/^[1-9][0-9]*$/.test(memberCapacity) || !Number.isSafeInteger(Number(memberCapacity)) || Number(memberCapacity) > 0xffffffff) throw new Error("Enter a whole member capacity supported by this device.");
          const profile = app.applicationGroupProfile(Number(memberCapacity));
          submit(new groups.GroupIntent.Create({ title, profile }));
        } catch (cause) { setError(errorMessage(cause)); }
      }} />
    </View>}
    {screen === "list" && <View style={styles.listSection}>
    <Text accessibilityRole="header" style={styles.title}>Your conversations</Text>
    {catalog.loaded && conversations.length === 0 && <Text>{catalog.hasNext ? "No visible conversations on this page. Continue to the next page." : "No conversations on this page. Create one or accept an invitation."}</Text>}
    {conversations.map((row) => <ListRow key={hex(row.group)} title={row.title} subtitle={`${row.memberCount} members`}
      accessibilityLabel={`${row.title} · ${row.memberCount} members · ${groups.GroupLifecycle[row.lifecycle]}`}
      badge={<View style={styles.row}><Badge warning={row.lifecycle !== groups.GroupLifecycle.Active}>{groups.GroupLifecycle[row.lifecycle]}</Badge>
        {unread[hex(row.group)] === "unread" && <Badge>Unread messages</Badge>}
        {(unread[hex(row.group)] === "unchecked" || viewed.error !== undefined) && <Text>Unread status not checked</Text>}</View>}
      onPress={() => { setSelected(hex(row.group)); setConfirmation(undefined); }} />)}
    <Paging label="Conversations" active={reading && !busy} page={catalog} />
    <View style={styles.listSection}>
      <Text accessibilityRole="header" style={styles.title}>Invitations</Text>
      {offers.loaded && invitations.length === 0 && <Text>{offers.hasNext ? "No visible invitations on this page. Continue to the next page." : "No invitations on this page."}</Text>}
      {invitations.map((invitation) => <InvitationRow key={hex(invitation.id)} invitation={invitation} contacts={contacts} onOpen={() => openInvitation(hex(invitation.id))} />)}
      <Paging label="Invitations" active={reading && !busy} page={offers} />
    </View>
    </View>}
    {(screen === "conversation" || screen === "invite" || screen === "details" || screen === "delivery") && selectedConversation.error !== undefined && <View style={styles.section}>
      <Text accessibilityRole="alert">{selectedConversation.error}</Text>
      <Button title="Retry conversation" disabled={!reading} onPress={refreshDetail} />
    </View>}
    {(screen === "conversation" || screen === "invite" || screen === "details" || screen === "delivery") && selectedConversation.busy && conversation === undefined && selectedConversation.error === undefined && <ActivityIndicator accessibilityLabel="Loading selected conversation" />}
    {conversation !== undefined && (screen === "conversation") && <View style={styles.section}>
      <PageHeader title={conversation.title} subtitle={`${conversation.memberCount} members`}
        trailing={<ActionMenu label="Conversation options" items={[
          ...(conversation.capabilities.canInvite ? [{ title: "Invite people", disabled: groupUnavailable, onPress: () => openInvite(hex(conversation.group)) }] : []),
          { title: "Conversation info", onPress: () => onOpenDetails?.(hex(conversation.group)) },
          { title: "Connection activity", onPress: () => onOpenDelivery?.(hex(conversation.group)) },
          ...(onOpenActivity === undefined ? [] : [{ title: "Action history", onPress: onOpenActivity }]),
        ]} />} />
      {conversation.lifecycle !== groups.GroupLifecycle.Active && <Badge warning>{groups.GroupLifecycle[conversation.lifecycle]}</Badge>}
      <History key={hex(conversation.group)} client={client} conversation={conversation} contacts={contacts} revision={`${conversation.revision}:${hex(conversation.control.era)}:${hex(conversation.control.head)}`} active={active} onViewed={onViewed} onOpenMessage={onOpenMessage} />
      {draft.error !== undefined && <View><Text accessibilityRole="alert">{draft.error}</Text>
        <Text>Your unsent text is kept. Retry saving before leaving this conversation.</Text>
        <Button title="Retry saving message draft" onPress={draft.retry} /></View>}
      <View style={styles.composer}><TextInput accessibilityLabel="Message" placeholder="Write a message" maxLength={8192} multiline value={draft.text} style={[styles.input, { flex: 1, minWidth: 0 }]}
        editable={!unavailable && draft.ready && conversation.capabilities.canSend} onChangeText={draft.setText} />
      <Button tone="primary" title="Send" accessibilityLabel="Send message" disabled={groupUnavailable || !conversation.capabilities.canSend || !draft.ready || draft.pending || draft.error !== undefined || !draft.text.trim()} onPress={() => {
        try { submit(new groups.GroupIntent.Publish(groups.textPublication(conversation.group, draft.text))); }
        catch (cause) { setError(errorMessage(cause)); }
      }} />
      </View>
      {!conversation.capabilities.canSend && <Text>You can’t send messages in this conversation right now.</Text>}
    </View>}
    {conversation !== undefined && screen === "details" && <View style={styles.section}>
      <PageHeader title="Conversation info" subtitle={conversation.title}>
        {conversation.capabilities.canInvite && <Button tone="primary" size="compact" icon="add" title="Invite people" disabled={groupUnavailable} onPress={() => openInvite(hex(conversation.group))} />}
      </PageHeader>
      <Panel><Text accessibilityRole="header" style={styles.title}>Members</Text>
        <Members client={client} conversation={conversation} contacts={contacts} active={reading}
          revision={revision} disabled={groupUnavailable} refreshDetail={refreshDetail}
          remove={(member) => ask(new groups.GroupIntent.RemoveMember({ group: conversation.group, member }), "Remove member", "This owner action removes this membership from future group messages. It does not erase copies already held by that person.")} />
      </Panel>
      <Disclosure label="Conversation identifiers">
        {conversation.profile !== undefined && <Text>Member capacity: {conversation.profile.maxMembers}</Text>}
        <Text selectable style={styles.fingerprint}>Group: {hex(conversation.group)}</Text>
      </Disclosure>
      <Disclosure label="Manage conversation">
        {conversation.capabilities.canLeave && <Button tone="destructive" title="Leave group" disabled={groupUnavailable} onPress={() => ask(new groups.GroupIntent.Leave({ group: conversation.group }), "Leave group", "Request departure from this group. Your existing local history stays until you delete it; departure may need the owner to finish.")} />}
        {conversation.capabilities.canClose && <Button tone="destructive" title="Close group for everyone" disabled={groupUnavailable} onPress={() => ask(new groups.GroupIntent.Close({ group: conversation.group }), "Close group for everyone", "This owner action permanently closes the group for everyone. It keeps existing local history and does not erase anyone’s copies.")} />}
        {conversation.capabilities.canDeleteLocal && <Button tone="destructive" title="Delete local history" disabled={groupUnavailable} onPress={() => ask(new groups.GroupIntent.DeleteLocal({ group: conversation.group }), "Delete local history", "Delete this group’s history from this device. This does not revoke membership, erase other devices, or guarantee secure erasure. Saved departure and closure notices may still finish sending.")} />}
        {!conversation.capabilities.canDeleteLocal && conversation.capabilities.canClose && <Text>Close this group before deleting its local history.</Text>}
      </Disclosure>
    </View>}
    {screen === "message" && group !== undefined && incarnation !== undefined && messageId !== undefined && <MessageDetails
      client={client} group={group} incarnation={incarnation} messageId={messageId} contacts={contacts} active={reading} revision={revision} localIdentity={identity.publicHash}
      {...(conversation === undefined ? {} : { conversationTitle: conversation.title })} />}
    {conversation !== undefined && screen === "delivery" && <View style={styles.section}>
      <PageHeader title="Connection activity" subtitle={conversation.title} />
      <DeliveryDetails client={client} conversation={conversation} contacts={contacts} active={active} heading={false} />
    </View>}
    {screen === "invite" && conversation !== undefined && <Invite client={client} conversation={conversation} contacts={contacts} active={reading}
      disabled={groupUnavailable} revision={revision} initialContact={recipient} invite={(recipient) => submit(new groups.GroupIntent.Invite({ group: conversation.group, recipient }))} />}
    {screen === "invitation" && <>
      {selectedInvitation.busy && !selectedInvitation.loaded && <ActivityIndicator accessibilityLabel="Loading invitation" />}
      {selectedInvitation.error !== undefined && <View><Text accessibilityRole="alert">{selectedInvitation.error}</Text><Button title="Retry invitation" onPress={refreshDetail} /></View>}
      {selectedInvitation.loaded && selectedInvitation.value === undefined && <Text>This invitation is no longer available.</Text>}
      {selectedInvitation.value !== undefined && <InvitationDetails invitation={selectedInvitation.value} contacts={contacts} disabled={unavailable}
        respond={(accept) => submit(new groups.GroupIntent.Respond({ invitation: selectedInvitation.value!.id, accept }))}
        cancel={() => submit(new groups.GroupIntent.CancelInvitation({ invitation: selectedInvitation.value!.id }))}
        dismiss={() => submit(new groups.GroupIntent.DismissInvitation({ invitation: selectedInvitation.value!.id }))}
        openGroup={() => setSelected(hex(selectedInvitation.value!.group))} />}
    </>}
    {confirmation !== undefined && <View accessibilityRole="alert" style={styles.notice}>
      <Text style={styles.title}>{confirmation.title}</Text><Text>{confirmation.consequence}</Text>
      <Button tone="destructive" title={`Confirm: ${confirmation.title}`} disabled={groupUnavailable} onPress={() => { if (!groupUnavailable) submit(confirmation.intent); }} />
      <Button title="Keep group unchanged" onPress={() => setConfirmation(undefined)} />
    </View>}
  </View>;
}
function useStyles() {
  const palette = useAppPalette();
  return StyleSheet.create({
  listSection: { gap: 12, borderTopWidth: 1, borderTopColor: palette.border, paddingTop: 16 },
  composer: { flexDirection: "row", alignItems: "flex-end", gap: 8 },
  section: { gap: 12 }, row: { flexDirection: "row", alignItems: "center", gap: 8 }, heading: { fontSize: 24, fontWeight: "700" }, title: { fontSize: 17, fontWeight: "600" },
  card: { borderWidth: 1, borderColor: palette.border, borderRadius: 10, padding: 12, gap: 8 },
  notice: { borderWidth: 1, borderColor: palette.warning, backgroundColor: palette.warningSurface, borderRadius: 10, padding: 12, gap: 8 },
  input: { borderWidth: 1, borderColor: palette.border, borderRadius: 8, padding: 12, fontSize: 16 },
  error: { color: palette.destructive }, fingerprint: { fontFamily: "monospace", fontSize: 12 },
  });
}
