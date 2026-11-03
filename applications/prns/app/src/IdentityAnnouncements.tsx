import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { View } from "react-native";
import { prns_identities_runtime as runtime } from "@prns/app-bindings";
import type { prns_identities as identities, prns_identities_core as core, prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { Disclosure } from "./ui/Disclosure";
import { Actions, Button, Text } from "./ui/primitives";
import { useStyles } from "./ui";

const choices = [undefined, 15, 30, 60, 180, 360] as const;
function intervalLabel(interval: bigint | undefined): string {
  if (interval === undefined) return "Manual only";
  return `Every ${interval / 60_000n} minutes`;
}

/** Preferences and admission come from the shared durable identity scheduler. */
export function IdentityAnnouncements({ owner, reference, active, compact = false }: {
  owner: owners.ServiceRuntimeLike; reference: core.IdentityReference; active: boolean; compact?: boolean;
}) {
  const styles = useStyles();
  const [retry, setRetry] = useState(0);
  const access = useMemo(() => {
    try { return { client: owner.announcements(reference) }; }
    catch (cause) { return { error: String(cause) }; }
  }, [owner, reference, retry]);
  const [settings, setSettings] = useState<identities.IdentityAnnounceSettings>();
  const [error, setError] = useState<string>();
  const [result, setResult] = useState<string>();
  const [busy, setBusy] = useState(false);
  const epoch = useRef(0);
  const pending = useRef(false);
  const generation = useRef(0);
  const activeNow = useRef(active); activeNow.current = active;
  useLayoutEffect(() => {
    generation.current++; epoch.current++; pending.current = false;
    setBusy(false); setSettings(undefined); setError(undefined); setResult(undefined);
    return () => { generation.current++; epoch.current++; };
  }, [access]);
  useEffect(() => {
    const ticket = ++epoch.current;
    if (!active || access.client === undefined) return;
    void access.client.settings().then((value) => {
      if (ticket === epoch.current) { setSettings(value); setError(undefined); }
    }).catch((cause) => { if (ticket === epoch.current) setError(String(cause)); });
    return () => { epoch.current++; };
  }, [active, access, retry]);
  const run = async (action: (client: identities.IdentityAnnounceClientLike) => Promise<{
    settings?: identities.IdentityAnnounceSettings; result?: string;
  }>) => {
    if (!activeNow.current || pending.current || access.client === undefined) return;
    const clientGeneration = generation.current;
    const current = () => clientGeneration === generation.current;
    epoch.current++;
    pending.current = true; setBusy(true); setError(undefined); setResult(undefined);
    try {
      const outcome = await action(access.client);
      if (current()) {
        epoch.current++;
        if (activeNow.current) {
          if (outcome.settings !== undefined) setSettings(outcome.settings);
          if (outcome.result !== undefined) setResult(outcome.result);
        }
      }
    } catch (cause) {
      if (current() && activeNow.current) {
        epoch.current++; setError(String(cause));
        // A preference may have committed before the reply failed. Reread it.
        try {
          const value = await access.client.settings();
          if (current() && activeNow.current) { epoch.current++; setSettings(value); }
        } catch { /* Keep the original error. */ }
      }
    } finally {
      // Hiding the panel does not cancel its accepted native operation.
      if (current()) { pending.current = false; setBusy(false); }
    }
  };
  const announceNow = () => { void run(async (client) => {
    const value = await client.announce();
    return { result: value.tag === runtime.IdentityManualAnnounce_Tags.Queued ? "Announcement queued."
      : value.tag === runtime.IdentityManualAnnounce_Tags.Throttled ? `Announcement queued. It can be sent in about ${(value.inner.retryAfterMs + 999n) / 1000n} seconds when connected.`
      : "This identity is not active on the network. Start networking and messaging before announcing." };
  }); };
  const schedule = <View style={styles.section}>
      {settings !== undefined && <Text>Schedule: {intervalLabel(settings.intervalMillis)}</Text>}
      {compact && <Text>Choose how often to announce while connected. This schedule is saved for this identity.</Text>}
      <View style={styles.row}>{choices.map((minutes) => {
        const intervalMillis = minutes === undefined ? undefined : BigInt(minutes) * 60_000n;
        return <Button key={minutes ?? "manual"} title={minutes === undefined ? "Manual only" : `${minutes} minutes`}
          size={compact ? "compact" : "normal"}
          accessibilityState={{ selected: settings !== undefined && settings.intervalMillis === intervalMillis }}
          disabled={!active || busy || settings === undefined} onPress={() => { void run(async (client) => {
            return { settings: await client.setSettings({ intervalMillis }) };
          }); }} />;
      })}</View>
    </View>;
  return <View style={compact ? styles.section : styles.card}>
    {!compact && <><Text accessibilityRole="header" style={styles.title}>Identity announcements</Text>
      <Text>Announcements help others find you. They are spaced out across this phone’s identities to avoid unnecessary traffic. Your schedule is saved.</Text></>}
    <Actions>
      <Button title={compact ? "Announce yourself" : "Announce this identity now"} tone="primary" size={compact ? "compact" : "normal"}
        disabled={!active || busy || settings === undefined} onPress={announceNow} />
    </Actions>
    {(error ?? access.error) !== undefined && <Text accessibilityRole="alert">{error ?? access.error}</Text>}
    {result !== undefined && <Text accessibilityLiveRegion="polite">{result}</Text>}
    {compact ? <Disclosure label="Announcement schedule">{schedule}</Disclosure> : schedule}
    {(error ?? access.error) !== undefined && <Button title="Retry loading announcement settings" disabled={!active || busy} onPress={() => setRetry((value) => value + 1)} />}
  </View>;
}
