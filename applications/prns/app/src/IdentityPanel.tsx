import { Button, Text, TextInput } from "./ui/primitives";
import { useCallback, useEffect, useRef, useState } from "react";
import { View } from "react-native";
import { getRandomBytesAsync } from "expo-crypto";
import { prns_identities_core as core, prns_identities as identities } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners, prns_groups as groups } from "@prns/app-bindings";
import { ContactList } from "./ContactList";
import { IdentityAnnouncements } from "./IdentityAnnouncements";
import { identityKey, loadContacts } from "./contacts";
import type { SavedContact } from "./contacts";
import { Messaging, type MessagingScreen } from "./messaging/Messaging";
import { useStyles } from "./ui";

const groupService = 0x67727073;

function operationLabel(record: identities.IdentityOperationRecord): string {
  const target = identities.IdentityCommand.Transition.instanceOf(record.command) ? record.command.inner[0].target.tag : "Create";
  const action = target === "Enable" ? "Start messaging" : target === "Pause" ? "Pause messaging" : target === "AddService" ? "Set up messaging" : target;
  return `${action}: ${record.state.tag === core.IdentityOperationState_Tags.Reserved ? "waiting for confirmation" : "finishing"}`;
}

function metadataForm(record: identities.IdentityRecord) {
  return { localLabel: record.metadata.localLabel, publishedName: record.metadata.publishedName, dirty: false,
    guard: { id: record.id, publicHash: record.publicHash, metadataRevision: record.metadataRevision } };
}

export type IdentitySection = "Conversations" | "Contacts" | "Identity";

export function IdentityPanel({ owner, record, active, onChanged, contactsRevision = 0, onContactsChanged, section, onSectionChange, group, recipient, onOpenGroup, onOpenContact, messagingScreen, invitation, onOpenInvitation, onInviteGroup, onOpenActivity, onOpenCreate, onOpenDetails, onOpenDelivery, incarnation, messageId, onOpenMessage }: {
  owner: owners.ServiceRuntimeLike; record: identities.IdentityRecord; active: boolean; contactsRevision?: number; onContactsChanged?: (() => void) | undefined; onChanged: () => Promise<void>;
  section: IdentitySection; onSectionChange?: ((section: IdentitySection) => void) | undefined;
  group?: string | undefined; recipient?: string | undefined; onOpenGroup?: ((group: string) => void) | undefined;
  onOpenContact?: ((contact: SavedContact) => void) | undefined;
  messagingScreen?: MessagingScreen | undefined;
  invitation?: string | undefined; onOpenInvitation?: ((id: string) => void) | undefined; onInviteGroup?: ((group: string) => void) | undefined;
  onOpenActivity?: (() => void) | undefined; onOpenCreate?: (() => void) | undefined;
  onOpenDetails?: ((group: string) => void) | undefined; onOpenDelivery?: ((group: string) => void) | undefined;
  incarnation?: string | undefined; messageId?: string | undefined;
  onOpenMessage?: ((group: string, incarnation: string, messageId: string) => void) | undefined;
}) {
  const styles = useStyles();
  const [boundOwner] = useState(owner);
  const [catalog] = useState(() => owner.identities());
  const [identity] = useState(() => catalog.identity({ id: record.id, publicHash: record.publicHash }));
  const [reference] = useState(() => identity.reference());
  const [groupAccess, setGroupAccess] = useState<{ client?: groups.ClientLike; error?: string }>({});
  const [groupRetry, setGroupRetry] = useState(0);
  const openSection = (section: IdentitySection) => onSectionChange?.(section);
  const [snapshot, setSnapshot] = useState<identities.IdentitySnapshot>();
  const [operations, setOperations] = useState<identities.IdentityOperationRecord[]>([]);
  const [contacts, setContacts] = useState<readonly SavedContact[]>([]);
  const [contactsReady, setContactsReady] = useState(false);
  const [contactsLoading, setContactsLoading] = useState(false);
  const [contactError, setContactError] = useState<string>();
  const [error, setError] = useState<string>();
  const [snapshotError, setSnapshotError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [retirement, setRetirement] = useState<identities.IdentityPreparedTransition>();
  const [form, setForm] = useState(() => metadataForm(record));
  const mounted = useRef(true);
  const inFlight = useRef(false);
  const readGeneration = useRef(0);
  const contactGeneration = useRef(0);
  const key = identityKey(reference);
  const current = snapshot?.identity ?? record;
  const configured = current.services.includes(groupService);
  useEffect(() => {
    if (!configured) { setGroupAccess({}); return; }
    // A service enters the catalog only after retained AddService has durably
    // prepared its schema. Ordinary client construction/reads never initialize.
    try { setGroupAccess({ client: boundOwner.groups(reference) }); }
    catch (cause) { setGroupAccess({ error: cause instanceof Error ? cause.message : String(cause) }); }
  }, [boundOwner, configured, reference, groupRetry]);
  const refreshContacts = useCallback(async () => {
    const ticket = ++contactGeneration.current;
    setContactsLoading(true);
    setContactError(undefined);
    try {
      const rows = await loadContacts(reference);
      if (mounted.current && ticket === contactGeneration.current) { setContacts(rows); setContactsReady(true); }
    } catch (cause) {
      if (mounted.current && ticket === contactGeneration.current) setContactError(cause instanceof Error ? cause.message : String(cause));
    } finally { if (mounted.current && ticket === contactGeneration.current) setContactsLoading(false); }
  }, [reference]);
  useEffect(() => {
    mounted.current = true;
    void refreshContacts();
    return () => { mounted.current = false; contactGeneration.current++; readGeneration.current++; };
  }, [refreshContacts]);
  const previousContactsRevision = useRef(contactsRevision);
  useEffect(() => {
    if (previousContactsRevision.current === contactsRevision) return;
    previousContactsRevision.current = contactsRevision;
    void refreshContacts();
  }, [contactsRevision, refreshContacts]);
  const refresh = useCallback(async () => {
    const ticket = ++readGeneration.current;
    try {
      const [current, retained] = await Promise.all([identity.snapshot(), catalog.operations()]);
      if (mounted.current && ticket === readGeneration.current) {
        setSnapshot(current);
        setOperations(retained.filter((item) => identities.IdentityCommand.Transition.instanceOf(item.command) &&
          identityKey(item.command.inner[0].identity) === key && item.state.tag !== core.IdentityOperationState_Tags.Completed));
        // A dirty form keeps the exact guard that produced it. Polling must not
        // bless stale edits with the latest revision and bypass native CAS.
        setForm((previous) => previous.dirty ? previous : metadataForm(current.identity));
      }
    } catch (cause) {
      if (mounted.current && ticket === readGeneration.current) throw cause;
    }
  }, [catalog, identity, key]);
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    let reading = false;
    const poll = async () => {
      if (reading || inFlight.current) return;
      reading = true;
      try {
        await refresh();
        if (!cancelled) setSnapshotError(undefined);
      } catch (cause) { if (!cancelled) setSnapshotError(cause instanceof Error ? cause.message : String(cause)); }
      finally { reading = false; }
    };
    void poll();
    const timer = setInterval(() => { void poll(); }, 1500);
    return () => { cancelled = true; readGeneration.current++; clearInterval(timer); };
  }, [active, refresh]);

  const run = async (operation: () => Promise<void>) => {
    if (inFlight.current || !mounted.current || !active) return;
    readGeneration.current++; // Invalidate every snapshot begun before this mutation.
    inFlight.current = true; setBusy(true); setError(undefined);
    try { await operation(); await refresh(); await onChanged(); }
    catch (cause) {
      if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause));
      // Re-read native evidence after uncertainty; never replace the command.
      try { await refresh(); } catch { /* Keep the original failure visible. */ }
    } finally { inFlight.current = false; if (mounted.current) setBusy(false); }
  };
  const transition = async (target: core.IdentityTransitionTarget, stop = false) => {
    const digest = Uint8Array.from(await getRandomBytesAsync(32)).buffer;
    const prepared = await identity.prepare(target);
    if (stop) { await identity.applyStop(prepared, digest); }
    else {
      // Reservation durably stores this exact input in the native catalog.
      // If its reply or this JS instance is lost, the operations view below
      // recovers that record; it never reconstructs a new command from a label.
      const token = await catalog.reserve(prepared, digest);
      await identity.begin(token, prepared);
    }
  };
  const paused = current.desired !== core.IdentityDesiredState.Enabled;
  const pending = operations.length > 0 || current.pending !== undefined || current.recoveryCleanup !== undefined;
  const metadataConflict = form.dirty && current.metadataRevision !== form.guard.metadataRevision;
  const status = snapshot === undefined ? "Checking messaging status…"
    : current.desired === core.IdentityDesiredState.Retired ? "This identity is retired. Existing history and contacts remain available."
    : !configured ? pending ? "Messaging setup has saved work to finish. Review the identity operation below." : "Messaging is not set up for this identity."
    : snapshot.runtime === core.IdentityRuntimeStatus.Stopping || current.recoveryCleanup !== undefined ? "Stopping messaging. Finishing accepted work; history and contacts remain available."
    : snapshot.runtime === core.IdentityRuntimeStatus.Starting ? "Messaging is starting."
    : snapshot.runtime === core.IdentityRuntimeStatus.Running ? "Messaging is active"
    : paused && current.pending === undefined ? "Messaging is paused. Saved history and contacts remain available."
    : "Messaging is not connected. Saved history and contacts remain available.";
  return <View style={styles.section}>
    {section === "Identity" && <Text accessibilityRole="header" style={styles.heading}>{current.metadata.localLabel}</Text>}
    {(section === "Identity" || snapshot?.runtime !== core.IdentityRuntimeStatus.Running || !configured) && <Text>{status}</Text>}
    {!configured && <View style={styles.card}>
      <Text>Set up messaging for this identity, then choose Start messaging to connect.</Text>
      <Button tone="primary" title="Set up messaging" disabled={!active || busy || snapshot === undefined || pending || current.desired === core.IdentityDesiredState.Retired}
        onPress={() => { void run(async () => { await transition(new core.IdentityTransitionTarget.AddService(groupService)); openSection("Identity"); }); }} />
      {pending && <Button title="Review saved setup" onPress={() => openSection("Identity")} />}
    </View>}
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
    {snapshotError !== undefined && <Text accessibilityRole="alert">Messaging status is temporarily unavailable. Checking again automatically.</Text>}
    {section === "Conversations" && <View style={styles.section}>
      {configured && groupAccess.client !== undefined && <Messaging identity={reference} client={groupAccess.client} contacts={contacts} active={active} group={group} recipient={recipient} onOpenGroup={onOpenGroup} screen={messagingScreen ?? "list"} invitation={invitation} onOpenInvitation={onOpenInvitation} onInviteGroup={onInviteGroup} onOpenActivity={onOpenActivity} onOpenCreate={onOpenCreate} onOpenDetails={onOpenDetails} onOpenDelivery={onOpenDelivery} incarnation={incarnation} messageId={messageId} onOpenMessage={onOpenMessage} />}
      {!configured && <Text>Your conversations will appear here after messaging setup.</Text>}
      {configured && groupAccess.error !== undefined && <View><Text accessibilityRole="alert" style={styles.error}>{groupAccess.error}</Text>
        <Button title="Retry opening conversations" onPress={() => setGroupRetry((value) => value + 1)} /></View>}
    </View>}
    {section === "Contacts" && <View style={styles.section}>
      {contactsReady && <ContactList identity={identity} displayName={current.metadata.publishedName} contacts={contacts} active={active}
        onOpenContact={onOpenContact} onChange={(rows) => { onContactsChanged?.(); if (mounted.current) { contactGeneration.current++; setContactsLoading(false); setContacts(rows); } }} />}
      {contactsLoading && !contactsReady && <Text>Loading contacts…</Text>}
      {contactError !== undefined && <Text accessibilityRole="alert" style={styles.error}>{contactError}</Text>}
      {contactError !== undefined && <Button title="Retry loading contacts" disabled={contactsLoading || !active}
        onPress={() => { void refreshContacts(); }} />}
    </View>}
    {section === "Identity" && <View style={styles.section}>
      {configured && <View style={styles.row}>
        <Button title="Open conversations" onPress={() => openSection("Conversations")} />
        <Button title="Share or add a contact" onPress={() => openSection("Contacts")} />
      </View>}
      <Text>Pausing stops new messaging work. It does not leave groups or delete local history.</Text>
      {configured && <Button tone={paused ? "primary" : "secondary"} title={paused ? "Start messaging" : "Pause messaging"} disabled={busy || snapshot === undefined || pending || current.desired === core.IdentityDesiredState.Retired}
        onPress={() => { void run(() => transition(paused ? new core.IdentityTransitionTarget.Enable() : new core.IdentityTransitionTarget.Pause(), !paused)); }} />}
      {configured && !paused && snapshot?.runtime === core.IdentityRuntimeStatus.Unavailable && !pending && <Button
        title="Resume messaging" disabled={!active || busy} onPress={() => { void run(async () => { await identity.recover(); }); }} />}
      {current.desired !== core.IdentityDesiredState.Retired && <Button title="Review identity retirement"
        disabled={!active || busy || snapshot === undefined || pending} onPress={() => { void run(async () => {
          const prepared = await identity.prepare(new core.IdentityTransitionTarget.Retire());
          if (mounted.current) setRetirement(prepared);
        }); }} />}
      {retirement !== undefined && current.desired !== core.IdentityDesiredState.Retired && <View style={styles.card}>
        <Text>Retirement permanently prevents new messaging and contact-card signing with this identity. It keeps existing local history and contacts, and does not erase copies held by other people.</Text>
        <Text>You may need to close groups you own or finish pending messages before retiring this identity.</Text>
        <Button title="Keep identity available" disabled={busy} onPress={() => setRetirement(undefined)} />
        <Button tone="destructive" title="Confirm identity retirement" disabled={!active || busy || pending} onPress={() => { const exact = retirement; void run(async () => {
          const digest = Uint8Array.from(await getRandomBytesAsync(32)).buffer;
          await identity.applyStop(exact, digest);
          if (mounted.current) setRetirement(undefined);
        }); }} />
      </View>}
      {snapshot?.lastError !== undefined && <Text accessibilityRole="alert" style={styles.error}>{String(snapshot.lastError)}</Text>}
      {operations.map((operation) => <View key={operation.token.sequence.toString()} style={styles.card}>
        <Text>{operationLabel(operation)}</Text>
        <Button title="Resume saved identity operation" disabled={busy} onPress={() => { void run(async () => {
          if (!identities.IdentityCommand.Transition.instanceOf(operation.command)) return;
          if (operation.state.tag === core.IdentityOperationState_Tags.Reserved) {
            await identity.begin(operation.token, operation.command.inner[0]);
          } else await identity.recover();
        }); }} />
        {operation.state.tag === core.IdentityOperationState_Tags.Reserved && <Button title="Cancel unstarted operation" disabled={busy}
          onPress={() => { void run(async () => { await catalog.cancelReserved(operation.token); }); }} />}
      </View>)}
      <TextInput accessibilityLabel="Edit private identity label" value={form.localLabel}
        onChangeText={(localLabel) => setForm((previous) => ({ ...previous, localLabel, dirty: true }))} editable={!busy} style={styles.input} />
      <TextInput accessibilityLabel="Edit shared identity name" value={form.publishedName}
        onChangeText={(publishedName) => setForm((previous) => ({ ...previous, publishedName, dirty: true }))} editable={!busy} style={styles.input} />
      {metadataConflict && <View style={styles.card}>
        <Text accessibilityRole="alert">The saved names changed since you started editing. Your draft has been kept. Reload the saved names before replacing them.</Text>
        <Button title="Discard name edits and reload" disabled={busy} onPress={() => setForm(metadataForm(current))} />
      </View>}
      <Button tone="primary" title="Save identity names" disabled={busy || snapshot === undefined || metadataConflict} onPress={() => { void run(async () => {
        const guard = await identity.updateMetadata(form.guard, { localLabel: form.localLabel, publishedName: form.publishedName });
        if (mounted.current) setForm({ ...form, guard, dirty: false });
      }); }} />

      <IdentityAnnouncements owner={boundOwner} reference={reference} active={active} />
    </View>}
  </View>;
}
