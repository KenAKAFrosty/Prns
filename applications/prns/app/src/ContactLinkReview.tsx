import { Button, Text, TextInput } from "./ui/primitives";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, View } from "react-native";
import type { prns_identities_core as core, prns_identities as identities, prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { reviewContactInput } from "./contact-input";
import { contactKey, loadContacts, saveContact } from "./contacts";
import { identityKey } from "./identity-reference";
import { hex } from "./bytes";
import type { ContactLink } from "./contact-links";
import type { SavedContact } from "./contacts";
import { useStyles } from "./ui";

type Destination = Readonly<{ reference: core.IdentityReference; label: string }>;
type SaveInput = Readonly<Destination & { uri: string; alias: string }>;
function message(cause: unknown) { return cause instanceof Error ? cause.message : String(cause); }
function destination(row: identities.IdentityRecord): Destination {
  return { reference: { id: { incarnation: row.id.incarnation.slice(0), sequence: row.id.sequence },
    publicHash: row.publicHash.slice(0) }, label: row.metadata.localLabel };
}

/** The caller keys this view by inbox entry, not selected identity. Selection is
 * always explicit and saves retain its complete immutable reference. */
export function ContactLinkReview({ link, owner, active, onDone, onSaved, catalogRevision }: {
  catalogRevision?: string;
  link: ContactLink; owner: owners.ServiceRuntimeLike | undefined; active: boolean;
  onDone: (id: number) => void; onSaved: (reference: core.IdentityReference, contacts: readonly SavedContact[]) => void;
}) {
  const styles = useStyles();
  const [review] = useState(() => {
    try { return { contact: reviewContactInput(link.url) }; }
    catch (cause) { return { error: message(cause) }; }
  });
  const [rows, setRows] = useState<readonly identities.IdentityRecord[]>([]);
  const [catalogReady, setCatalogReady] = useState(false);
  const [catalogError, setCatalogError] = useState<string>();
  const [reload, setReload] = useState(0);
  const [selected, setSelected] = useState<Destination>();
  const [alias, setAlias] = useState("");
  const [contactsReady, setContactsReady] = useState(false);
  const [contactRetry, setContactRetry] = useState(0);
  const [error, setError] = useState<string>();
  const [captured, setCaptured] = useState<SaveInput>();
  const [busy, setBusy] = useState(false);
  const mounted = useRef(true);
  const saving = useRef(false);
  const lockedInput = useRef<SaveInput | undefined>(undefined);
  const loadedOwner = useRef<owners.ServiceRuntimeLike | undefined>(undefined);
  const dirty = useRef(false);
  const foreground = useRef(active); foreground.current = active;
  const availableOwner = useRef(owner); availableOwner.current = owner;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    if (loadedOwner.current !== owner) { setCatalogReady(false); loadedOwner.current = undefined; }
    setCatalogError(undefined);
    if (owner === undefined || review.contact === undefined) return;
    let cancelled = false;
    const abort = new AbortController();
    void (async () => owner.identities().identities({ signal: abort.signal }))().then((value) => {
      if (!cancelled) { loadedOwner.current = owner; setRows(value); setCatalogReady(true); }
    }).catch((cause: unknown) => { if (!cancelled) { loadedOwner.current = undefined; setCatalogReady(false); setCatalogError(message(cause)); } });
    return () => { cancelled = true; abort.abort(); };
  }, [owner, reload, review, catalogRevision]);
  const selectedPresent = selected !== undefined && catalogReady && rows.some((row) => identityKey(row) === identityKey(selected.reference));
  useEffect(() => {
    setContactsReady(false);
    if (selected === undefined || !selectedPresent || review.contact === undefined) return;
    let cancelled = false;
    setError(undefined);
    void loadContacts(selected.reference).then((contacts) => {
      if (cancelled) return;
      const existing = contacts.find((entry) => contactKey(entry) === contactKey(review.contact!));
      if (!dirty.current && lockedInput.current === undefined) setAlias(existing?.alias ?? "");
      setContactsReady(true);
    }).catch((cause: unknown) => { if (!cancelled) setError(message(cause)); });
    return () => { cancelled = true; };
  }, [selected, selectedPresent, review, contactRetry]);
  const save = async () => {
    if (saving.current || !mounted.current || !foreground.current || availableOwner.current === undefined ||
      availableOwner.current !== loadedOwner.current || !selectedPresent || !contactsReady || selected === undefined || review.contact === undefined) return;
    saving.current = true; setBusy(true); setError(undefined);
    try {
      // Before any storage effect, validate the label with the existing helper.
      const checked = lockedInput.current ?? { ...selected, ...reviewContactInput(review.contact.uri, alias) };
      const input: SaveInput = { reference: checked.reference, label: checked.label, uri: checked.uri, alias: checked.alias };
      lockedInput.current = input; setCaptured(input);
      const contacts = await saveContact(input.reference, input.uri, input.alias);
      if (mounted.current) { onSaved(input.reference, contacts); onDone(link.id); }
    } catch (cause) { if (mounted.current) setError(message(cause)); }
    finally { saving.current = false; if (mounted.current) setBusy(false); }
  };
  return <View style={styles.card}>
    <Text accessibilityRole="header" style={styles.heading}>Incoming contact link</Text>
    {review.error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{review.error}</Text>}
    {review.contact !== undefined && <>
      <Text style={styles.title}>{review.contact.claims.displayName}</Text>
      <Text>Verified signed card · group messaging</Text>
      <Text>Confirm this identity with the person. Saving only adds a contact; it never accepts an invitation or joins a group.</Text>
      <Text selectable style={styles.fingerprint}>{hex(review.contact.claims.identityHash)}</Text>
      {owner === undefined ? <Text>Open or set up this phone, then choose which messaging identity should keep this contact.</Text> : <>
        {!catalogReady && catalogError === undefined && <ActivityIndicator accessibilityLabel="Loading contact destination identities" />}
        {catalogError !== undefined && <Text accessibilityRole="alert" style={styles.error}>{catalogError}</Text>}
        {catalogError !== undefined && <Button title="Retry loading destination identities" disabled={!active || busy} onPress={() => setReload((value) => value + 1)} />}
        {catalogReady && rows.length === 0 && <Text>Create an identity below. It will appear here automatically; this contact link will stay open.</Text>}
        {catalogReady && <Text>Choose where to save this contact:</Text>}
        {catalogReady && rows.map((row) => <Button key={identityKey(row)} title={`Save contact under ${row.metadata.localLabel}`}
          disabled={!active || busy || captured !== undefined} accessibilityState={{ selected: selected !== undefined && identityKey(row) === identityKey(selected.reference) }}
          onPress={() => { if (saving.current || lockedInput.current !== undefined || !foreground.current) return; dirty.current = false; setAlias(""); setSelected(destination(row)); }} />)}
      </>}
      {selected !== undefined && <View style={styles.section}>
        <Text>Destination: {selected.label}</Text>
        <Text selectable style={styles.fingerprint}>{hex(selected.reference.publicHash)}</Text>
        {catalogReady && !selectedPresent && <Text accessibilityRole="alert">The identity you chose is no longer available. This contact has not been saved under another identity.</Text>}
        {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
        {!contactsReady && selectedPresent && error !== undefined && <Button title="Retry loading destination contacts" disabled={!active || busy} onPress={() => setContactRetry((value) => value + 1)} />}
        <TextInput accessibilityLabel="Incoming contact private label" placeholder="Private label (optional)" value={alias}
          editable={active && contactsReady && !busy && captured === undefined}
          onChangeText={(value) => { dirty.current = true; setAlias(value); }} style={styles.input} />
        {captured !== undefined && <Text>Retry will save the same contact and label under the identity you chose. Dismissing this review does not undo a save that already finished.</Text>}
        <Button title={busy ? "Saving contact…" : captured === undefined ? "Save reviewed incoming contact" : "Retry saving contact"}
          disabled={!active || owner === undefined || busy || !selectedPresent || !contactsReady} onPress={() => { void save(); }} />
      </View>}
    </>}
    <Button title="Dismiss incoming contact link" disabled={!active || busy} onPress={() => onDone(link.id)} />
  </View>;
}
