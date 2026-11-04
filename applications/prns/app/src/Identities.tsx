import { Button, Text, TextInput } from "./ui/primitives";
import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, View } from "react-native";
import { prns_identities_core as core } from "@prns/app-bindings";
import type { prns_service_runtime_uniffi as owners, prns_identities as identities } from "@prns/app-bindings";
import { CreationDraftConflict, CreationMetadataRejected, hex, loadCreationDraft, prepareCreationDraft, submitCreationDraft } from "./identity-creation";
import type { CreationDraft } from "./identity-creation";
import { CatalogActivity } from "./CatalogActivity";
import { identityKey } from "./identity-reference";
import { useStyles } from "./ui";
import { presentationEpoch, requirePresentationEpoch } from "./runtime/presentation-storage";

function message(error: unknown): string { return error instanceof Error ? error.message : String(error); }

export function Identities({ owner, active: visible = true, onSelect }: {
  owner: owners.ServiceRuntimeLike; active?: boolean; onSelect: (identity: string) => void;
}) {
  const styles = useStyles();
  const [catalog] = useState(() => owner.identities());
  const [rows, setRows] = useState<identities.IdentityRecord[]>([]);
  const [draft, setDraft] = useState<CreationDraft>();
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [localLabel, setLocalLabel] = useState("");
  const [publishedName, setPublishedName] = useState("");
  const active = useRef(true);
  const inFlight = useRef(false);
  const loading = useRef(false);
  const loadGeneration = useRef(0);
  const refresh = useCallback(async () => {
    const result = await catalog.identities();
    if (active.current) setRows(result);
    return result;
  }, [catalog]);
  const reload = useCallback(async () => {
    if (inFlight.current) return;
    const ticket = ++loadGeneration.current;
    loading.current = true; setBusy(true); setError(undefined);
    try {
      const [pending, snapshot] = await Promise.all([loadCreationDraft(), catalog.identities()]);
      if (active.current && ticket === loadGeneration.current) {
        setDraft(pending); setRows(snapshot); setLoaded(true);
      }
    } catch (cause) {
      if (active.current && ticket === loadGeneration.current) { setError(message(cause)); setLoaded(false); }
    } finally {
      if (ticket === loadGeneration.current) { loading.current = false; if (active.current) setBusy(false); }
    }
  }, [catalog]);
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; loadGeneration.current++; };
  }, [reload]);
  useEffect(() => { if (visible) void reload(); }, [visible, reload]);

  const create = async () => {
    if (!active.current || !loaded || loading.current || inFlight.current) return;
    const storageEpoch = presentationEpoch();
    inFlight.current = true;
    setBusy(true);
    setError(undefined);
    try {
      requirePresentationEpoch(storageEpoch);
      const pending = draft ?? await prepareCreationDraft(catalog, { localLabel, publishedName });
      requirePresentationEpoch(storageEpoch);
      // Retain the ID even if writing the saved form has an uncertain outcome.
      if (active.current) setDraft(pending);
      const outcome = await submitCreationDraft(catalog, pending);
      if (active.current) { setDraft(undefined); setLocalLabel(""); setPublishedName(""); }
      const refreshed = await refresh();
      const result = outcome?.result;
      if (active.current && result !== undefined &&
          (result.tag === core.IdentityOperationResult_Tags.Created || result.tag === core.IdentityOperationResult_Tags.Existing)) {
        const identity = result.inner.identity;
        const created = refreshed.find((record) => record.id.sequence === identity.sequence && hex(record.id.incarnation) === hex(identity.incarnation));
        if (created !== undefined) onSelect(identityKey(created));
      }
    } catch (cause) {
      if (active.current) {
        if (cause instanceof CreationDraftConflict) setDraft(cause.pending);
        if (cause instanceof CreationMetadataRejected) {
          setDraft(undefined);
          setLocalLabel(cause.draft.localLabel);
          setPublishedName(cause.draft.publishedName);
        }
        setError(message(cause));
      }
    }
    finally { inFlight.current = false; if (active.current) setBusy(false); }
  };

  return <View style={styles.section}>
    <Text>Each identity has its own groups and message history. History stays until you delete it.</Text>
    <CatalogActivity catalog={catalog} active={visible} />
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
    {!loaded && error === undefined && <ActivityIndicator accessibilityLabel="Loading identities" />}
    {rows.map((row) => <View key={`${hex(row.id.incarnation)}:${row.id.sequence}:${hex(row.publicHash)}`} style={styles.card}>
      <Text style={styles.title}>{row.metadata.localLabel}</Text>
      <Text>{row.metadata.publishedName}</Text>
      <Text selectable style={styles.fingerprint}>{hex(row.publicHash)}</Text>
      <Text>{core.IdentityDesiredState[row.desired]}</Text>
      <Button title={`Open ${row.metadata.localLabel}`} onPress={() => {
        const key = identityKey(row);
        onSelect(key);
      }} />
    </View>)}
    {loaded && rows.length === 0 && <Text>No identities yet. Create one, then set up messaging in its identity settings.</Text>}
    {draft === undefined ? <View style={styles.section}>
      <TextInput accessibilityLabel="Private identity label" placeholder="Private label, e.g. Personal" value={localLabel}
        onChangeText={setLocalLabel} editable={loaded && !busy} style={styles.input} />
      <TextInput accessibilityLabel="Shared identity name" placeholder="Name shared with contacts" value={publishedName}
        onChangeText={setPublishedName} editable={loaded && !busy} style={styles.input} />
      <Button tone="primary" title={busy ? "Creating…" : "Create identity"}
        disabled={!loaded || busy || localLabel.trim().length === 0 || publishedName.trim().length === 0}
        onPress={() => { void create(); }} />
    </View> : <View style={styles.card}>
      <Text>Finish creating “{draft.localLabel}”. Retrying uses the saved request and cannot create a second identity.</Text>
      <Button title={busy ? "Checking…" : "Resume saved request"} disabled={!loaded || busy} onPress={() => { void create(); }} />
    </View>}
    {!loaded && error !== undefined && <Button title="Retry loading identities" disabled={busy} onPress={() => { void reload(); }} />}
  </View>;
}
