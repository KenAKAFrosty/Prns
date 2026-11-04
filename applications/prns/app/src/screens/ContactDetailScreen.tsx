import { useEffect, useRef, useState } from "react";
import { Alert } from "react-native";
import { useLocalSearchParams, useRouter } from "expo-router";
import { contactKey, loadContacts, removeContact, saveContact, setContactPinned } from "../contacts";
import type { SavedContact } from "../contacts";
import { identityKey } from "../identity-reference";
import { hex } from "../bytes";
import { scalar, useRouteActive } from "../navigation";
import { useInstallation } from "../runtime/InstallationProvider";
import { ActionMenu, Actions, Badge, Button, PageHeader, Panel, Screen, Text, TextInput } from "../ui/primitives";
import { useViewState } from "../view-state";
import { contactAliasDefault, decodeContactAlias } from "../contact-forms";

export function ContactDetailScreen() {
  const installation = useInstallation();
  const params = useLocalSearchParams<{ owner?: string; contact?: string }>();
  const reference = scalar(params.owner);
  const key = scalar(params.contact);
  const record = installation.records.find((item) => identityKey(item) === reference);
  const resolvedReference = record === undefined ? undefined : identityKey(record);
  const active = useRouteActive();
  const router = useRouter();
  const [contact, setContact] = useState<SavedContact>();
  const draft = useViewState(`prns.ui.contact-name.${resolvedReference ?? "unavailable"}.${key ?? "unavailable"}.v1`, contactAliasDefault, decodeContactAlias);
  const alias = draft.value.alias ?? contact?.alias ?? "";
  const setAlias = (alias: string) => draft.change(() => ({ version: 1, alias }));
  const [showIdentity, setShowIdentity] = useState(false);
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const generation = useRef(0);
  const mutating = useRef(false);
  useEffect(() => {
    const ticket = ++generation.current;
    setContact(undefined); setShowIdentity(false); setEditing(false); setError(undefined); setBusy(false); mutating.current = false;
    if (record === undefined || !active) return;
    void loadContacts(record).then((rows) => {
      if (ticket !== generation.current) return;
      const selected = rows.find((item) => contactKey(item) === key);
      if (selected === undefined) { setError("This contact is no longer saved."); return; }
      setContact(selected);
    }).catch((cause: unknown) => { if (ticket === generation.current) setError(String(cause)); });
    return () => { generation.current++; };
  }, [resolvedReference, key, active]);
  const save = async () => {
    if (!active || mutating.current || !draft.ready || draft.error !== undefined || record === undefined || contact === undefined) return;
    const ticket = generation.current;
    mutating.current = true; setBusy(true); setError(undefined);
    try {
      const rows = await saveContact(record, contact.uri, alias);
      installation.contactSaved();
      if (ticket === generation.current) { setContact(rows.find((item) => contactKey(item) === key)); setEditing(false); }
    } catch (cause) { if (ticket === generation.current) setError(String(cause)); }
    finally { if (ticket === generation.current) { mutating.current = false; setBusy(false); } }
  };
  const pin = async () => {
    if (!active || mutating.current || record === undefined || contact === undefined) return;
    const ticket = generation.current;
    mutating.current = true; setBusy(true); setError(undefined);
    try {
      const rows = await setContactPinned(record, contact, contact.pinned !== true);
      installation.contactSaved();
      if (ticket === generation.current) setContact(rows.find((item) => contactKey(item) === key));
    } catch (cause) { if (ticket === generation.current) setError(String(cause)); }
    finally { if (ticket === generation.current) { mutating.current = false; setBusy(false); } }
  };
  const remove = () => {
    if (record === undefined || contact === undefined || !active || mutating.current) return;
    const captured = contact, owner = record, ticket = generation.current;
    Alert.alert("Remove contact?", "This removes the saved contact from this identity. Conversations and group membership stay unchanged.", [
      { text: "Keep", style: "cancel" }, { text: "Remove", style: "destructive", onPress: () => {
        if (!active || mutating.current || ticket !== generation.current) return;
        mutating.current = true; setBusy(true);
        void removeContact(owner, captured).then(() => { installation.contactSaved(); if (ticket === generation.current) router.replace({ pathname: "/contacts", params: { owner: reference! } }); })
          .catch((cause: unknown) => { if (ticket === generation.current) setError(String(cause)); })
          .finally(() => { if (ticket === generation.current) { mutating.current = false; setBusy(false); } });
      } },
    ]);
  };
  return <Screen>
    <PageHeader title={contact?.alias || contact?.claims.displayName || "Contact details"}
      subtitle={contact?.alias && contact.alias !== contact.claims.displayName ? contact.claims.displayName : undefined}
      trailing={contact !== undefined && <ActionMenu label="Contact options" disabled={busy || !active} items={[
        { title: contact.pinned ? "Unpin contact" : "Pin contact", onPress: () => { void pin(); } },
        { title: "Edit private name", onPress: () => setEditing(true) },
        { title: showIdentity ? "Hide contact identity" : "Show contact identity", onPress: () => setShowIdentity((value) => !value) },
        { title: "Remove saved contact", destructive: true, onPress: remove },
      ]} />} />
    {error !== undefined && <Text accessibilityRole="alert">{error}</Text>}
    {draft.error !== undefined && <><Text accessibilityRole="alert">{draft.error}</Text><Button title="Retry contact-name draft" onPress={draft.retry} /></>}
    {record === undefined && <Text>This contact route does not identify an available messaging identity.</Text>}
    {contact !== undefined && <>
      <Actions><Button title="New conversation" tone="primary" icon="add" disabled={busy || !active || record === undefined || !record.services.includes(0x67727073)} onPress={() => {
        if (active && record !== undefined && contact !== undefined) router.push({ pathname: "/conversations/new", params: { owner: identityKey(record), contact: contactKey(contact) } });
      }} />{contact.pinned && <Badge>Pinned</Badge>}</Actions>
      <Text>Create a conversation, then review an invitation to this contact.</Text>
      {showIdentity && <Panel><Text accessibilityRole="header">Contact identity</Text>
        <Text>Shared name: {contact.claims.displayName}</Text>
        <Text selectable>{hex(contact.claims.identityHash)}</Text><Text>Compare this identity address with the person through a channel you trust.</Text>
        <Button title="Hide contact identity" tone="quiet" onPress={() => setShowIdentity(false)} />
      </Panel>}
      {editing && <Panel><Text accessibilityRole="header">Private contact name</Text>
        <TextInput accessibilityLabel="Private contact name" value={alias} onChangeText={setAlias} maxLength={1024} editable={draft.ready && !busy} />
        <Actions><Button tone="primary" title={busy ? "Saving…" : "Save private name"} disabled={busy || !active || !draft.ready || draft.error !== undefined} onPress={() => { void save(); }} />
          <Button title="Cancel editing" tone="quiet" disabled={busy} onPress={() => setEditing(false)} /></Actions>
      </Panel>}
    </>}
  </Screen>;
}
