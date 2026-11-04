import { Disclosure } from "./ui/Disclosure";
import { Actions, Button, Panel, Text, TextInput } from "./ui/primitives";
import { useEffect, useRef, useState } from "react";
import { AppState, Share, View } from "react-native";
import { prns_contact_card as cards } from "@prns/app-bindings";
import type { prns_identities as identities } from "@prns/app-bindings";
import { saveContact } from "./contacts";
import type { SavedContact } from "./contacts";
import { hex } from "./bytes";
import { useStyles } from "./ui";
import { ContactQr } from "./ContactQr";
import { ContactScanner } from "./ContactScanner";
import { reviewContactInput } from "./contact-input";
import { useViewState } from "./view-state";
import { identityKey } from "./identity-reference";
import { contactFormDefault, decodeContactForm } from "./contact-forms";

export function ContactExchange({ identity, displayName, onChange, active: visible, mode, initialCard }: {
  identity: identities.IdentityClientLike; displayName: string;
  active: boolean; mode: "add" | "card"; initialCard?: string | undefined;
  onChange: (contacts: readonly SavedContact[]) => void;
}) {
  const styles = useStyles();
  const form = useViewState(`prns.ui.contact-import.${identityKey(identity.reference())}.v1`, contactFormDefault, decodeContactForm);
  const { uri, alias } = form.value;
  const setUri = (uri: string) => form.change((previous) => ({ ...previous, uri }));
  const setAlias = (alias: string) => form.change((previous) => ({ ...previous, alias }));
  const [preview, setPreview] = useState<SavedContact>();
  const [ownCard, setOwnCard] = useState<SavedContact>();
  const [scanning, setScanning] = useState(false);
  const [appActive, setAppActive] = useState(AppState.currentState === "active");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const initialReviewed = useRef(false);
  useEffect(() => {
    if (mode !== "add" || initialCard === undefined || initialReviewed.current || !form.ready || form.error !== undefined) return;
    initialReviewed.current = true;
    try {
      const reviewed = reviewContactInput(initialCard);
      // An incoming card starts a deliberate review, never inherits another
      // card's private label or writes to the address book automatically.
      setPreview(reviewed); setError(undefined);
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  }, [mode, initialCard, form.ready, form.error]);
  const active = useRef(true);
  const inFlight = useRef(false);
  const foreground = useRef(visible);
  foreground.current = visible;
  const appForeground = useRef(appActive);
  useEffect(() => {
    active.current = true;
    const listener = AppState.addEventListener("change", (state) => {
      const foreground = state === "active";
      appForeground.current = foreground;
      setAppActive(foreground);
      if (!foreground) setScanning(false);
    });
    return () => { active.current = false; listener.remove(); };
  }, []);
  useEffect(() => { if (!visible) setScanning(false); }, [visible]);
  const run = async (operation: () => Promise<void>) => {
    if (inFlight.current || !active.current || !foreground.current || !appForeground.current) return;
    inFlight.current = true; setBusy(true); setError(undefined);
    try { await operation(); }
    catch (cause) { if (active.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { inFlight.current = false; if (active.current) setBusy(false); }
  };
  const issueCard = async (share: boolean) => {
    const wire = await identity.contactCard(cards.serviceNameHash("prns.groups.enrollment"), displayName);
    const contact = reviewContactInput(cards.encodeContactCardUri(wire));
    if (hex(contact.claims.identityHash) !== hex(identity.reference().publicHash)) {
      throw new Error("This contact card does not belong to the selected identity. It has not been shared.");
    }
    if (active.current) setOwnCard(contact);
    if (share && active.current && foreground.current && appForeground.current) await Share.share({ message: contact.uri });
  };
  const scanned = (data: string) => {
    if (!active.current || !foreground.current || !appForeground.current || !form.ready || form.error !== undefined) return;
    setScanning(false);
    try {
      const reviewed = reviewContactInput(data, alias);
      setUri(reviewed.uri); setPreview(reviewed); setError(undefined);
    } catch (cause) {
      setPreview(undefined);
      setError(cause instanceof Error ? cause.message : "This QR code is not a valid signed group contact card.");
    }
  };
  return <View style={styles.section}>
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
    {mode === "add" && form.error !== undefined && <View><Text accessibilityRole="alert">{form.error}</Text>
      <Button title="Retry saving or loading contact draft" onPress={form.retry} /></View>}
    {mode === "add" && !form.ready && form.error === undefined && <Text>Loading contact draft…</Text>}
    {mode === "card" && <>
      <Actions>
        {ownCard === undefined ? <>
          <Button title="Show my QR code" icon="qr" tone="primary" disabled={busy || scanning || !visible || !appActive} onPress={() => { void run(() => issueCard(false)); }} />
          <Button title="Share my contact card" icon="share" tone="quiet" disabled={busy || scanning || !visible || !appActive} onPress={() => { void run(() => issueCard(true)); }} />
        </> : <>
          <Button title="Share contact card" accessibilityLabel="Share this exact contact link" icon="share" tone="primary" disabled={busy || !visible || !appActive} onPress={() => { const exact = ownCard.uri; void run(async () => { await Share.share({ message: exact }); }); }} />
          <Button title="Hide my QR code" tone="quiet" onPress={() => setOwnCard(undefined)} />
        </>}
      </Actions>
      {ownCard !== undefined && <Panel>
        <Text style={styles.title}>{ownCard.claims.displayName || "Your contact card"}</Text>
        <ContactQr uri={ownCard.uri} />
        <Text>Let the other person scan this code, then compare the identity shown in their review.</Text>
        <Disclosure label="Contact link and address">
          <Text>Identity address</Text>
          <Text selectable accessibilityLabel="Your identity address" style={styles.fingerprint}>{hex(ownCard.claims.identityHash)}</Text>
          <Text selectable accessibilityLabel="Your contact link" style={styles.fingerprint}>{ownCard.uri}</Text>
        </Disclosure>
      </Panel>}
    </>}
    {mode === "add" && <>
      {preview === undefined ? <>
        <Actions><Button title="Scan a contact QR code" icon="qr" tone="primary" disabled={!form.ready || form.error !== undefined || busy || scanning || !visible || !appActive} onPress={() => { setPreview(undefined); setError(undefined); setScanning(true); }} /></Actions>
        {scanning && visible && appActive && <ContactScanner onScan={scanned} onCancel={() => setScanning(false)}
          onFailure={(message) => { if (active.current && foreground.current) { setScanning(false); setError(message); } }} />}
        <Panel><Text accessibilityRole="header" style={styles.title}>Or paste a contact link</Text>
          <TextInput accessibilityLabel="Contact link" placeholder="Paste a contact link" multiline autoCapitalize="none" maxLength={4096}
            value={uri} onChangeText={(value) => { setUri(value); setPreview(undefined); }} editable={form.ready && !busy && !scanning} style={styles.input} />
          <TextInput accessibilityLabel="Private contact label" placeholder="Private label (optional)" value={alias}
            onChangeText={(value) => { setAlias(value); setPreview(undefined); }} editable={form.ready && !busy && !scanning} maxLength={1024} style={styles.input} />
          <Button title="Review contact" disabled={!form.ready || form.error !== undefined || busy || scanning || !visible || uri.trim().length === 0} onPress={() => {
            try { setPreview(reviewContactInput(uri, alias)); setError(undefined); }
            catch (cause) { setPreview(undefined); setError(cause instanceof Error ? cause.message : String(cause)); }
          }} />
        </Panel>
      </> : <Panel>
        <Text style={styles.title}>{preview.alias || preview.claims.displayName}</Text>
        <Text>Verified signed card · group messaging</Text>
        <Text>Shared name: {preview.claims.displayName}</Text>
        <Text>Confirm this identity with the person before saving. Saving does not accept an invitation.</Text>
        <Text>Identity address</Text>
        <Text selectable accessibilityLabel="Contact identity address" style={styles.fingerprint}>{hex(preview.claims.identityHash)}</Text>
        <Actions><Button title="Save this contact" tone="primary" disabled={!form.ready || form.error !== undefined || busy || !visible || !appActive} onPress={() => { const exact = preview; void run(async () => {
          const next = await saveContact(identity.reference(), exact.uri, exact.alias);
          onChange(next);
          if (active.current) {
            setPreview(undefined);
            if (initialCard === undefined || uri === exact.uri) { setUri(""); setAlias(""); }
          }
        }); }} />
          <Button title="Edit contact" tone="quiet" disabled={busy} onPress={() => {
            if (initialCard !== undefined) { setUri(preview.uri); setAlias(preview.alias); }
            setPreview(undefined);
          }} />
        </Actions>
      </Panel>}
    </>}
  </View>;
}
