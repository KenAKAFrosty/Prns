import { useEffect, useMemo, useState } from "react";
import { prns_identities_core as core, prns_service_runtime as runtime } from "@prns/app-bindings";
import type { prns_identities as identities } from "@prns/app-bindings";
import { identityKey } from "../identity-reference";
import { useRouteActive } from "../navigation";
import { Link } from "expo-router";
import { ActivityIndicator } from "react-native";
import { useInstallation } from "../runtime/InstallationProvider";
import { Button, Heading, Panel, Screen, Text } from "../ui/primitives";

export function InstallationScreen() {
  const installation = useInstallation();
  const { mode, busy, owner, error, status } = installation;
  const active = useRouteActive();
  const record = installation.records.find((entry) => identityKey(entry) === installation.selected);
  const selected = record === undefined ? undefined : identityKey(record);
  const identity = useMemo(() => owner === undefined || record === undefined ? undefined
    : owner.identities().identity({ id: record.id, publicHash: record.publicHash }), [owner, selected]);
  const [observed, setObserved] = useState<{ client: identities.IdentityClientLike; snapshot?: identities.IdentitySnapshot; failed?: boolean }>();
  useEffect(() => {
    if (identity === undefined || !active) return;
    const abort = new AbortController();
    let reading = false;
    const read = async () => {
      if (reading) return;
      reading = true;
      try {
        const snapshot = await identity.snapshot({ signal: abort.signal });
        if (!abort.signal.aborted) setObserved({ client: identity, snapshot });
      } catch { if (!abort.signal.aborted) setObserved({ client: identity, failed: true }); }
      finally { reading = false; }
    };
    void read();
    const timer = setInterval(() => { void read(); }, 1500);
    return () => { abort.abort(); clearInterval(timer); };
  }, [identity, active]);
  const snapshot = observed?.client === identity ? observed?.snapshot : undefined;
  const current = snapshot?.identity ?? record;
  const configured = current?.services.includes(0x67727073) === true;
  const enabled = current?.desired === core.IdentityDesiredState.Enabled;
  const running = snapshot?.runtime === core.IdentityRuntimeStatus.Running && snapshot.lastError === undefined;
  const online = installation.network?.phase === runtime.ServiceRuntimeNetworkPhase.Ready;
  const identityRoute = { pathname: "/more/identities/[reference]" as const, params: { reference: selected ?? "" } };
  return <Screen>
    <Heading help={{ label: "About this phone", content: <><Text>Keep separate messaging identities for different parts of your life. Conversations and contacts stay on this device.</Text><Text>A radio board is optional. Connect using your phone’s local network, a TCP server or Bluetooth.</Text></> }}>Welcome to PRNS Chat</Heading>
    <Text>Connect nearby. Share a contact. Start a conversation.</Text>
    {error !== undefined && <Text accessibilityRole="alert">{error}</Text>}
    {mode === "open" && error !== undefined && <Panel><Text>Saved data could not be opened or networking could not finish starting. Existing data has been preserved. Retry Open, or review local reset in Settings if you want to start fresh.</Text>
      <Link href="/more/settings/reset" asChild><Button title="Review local reset options" /></Link></Panel>}
    {status?.failure !== undefined && <Text accessibilityRole="alert">{String(status.failure)}</Text>}
    {mode === undefined && <ActivityIndicator accessibilityLabel="Checking saved installation" />}
    {mode === undefined && error !== undefined && <Button title="Retry checking saved installation" onPress={installation.retryInitialization} />}
    {mode === undefined && error !== undefined && <Link href="/more/settings/reset" asChild><Button title="Review local reset options" /></Link>}
    {busy !== undefined && <ActivityIndicator accessibilityLabel={busy === "start" ? "Opening your installation" : "Stopping connections"} />}
    {(busy === "start" || (status !== undefined && !status.cleanupComplete)) && <Button
      title={busy === "start" ? "Cancel startup" : "Stop networking"} disabled={busy === "stop"}
      onPress={() => { void installation.stop(); }} />}
    {owner === undefined && mode !== undefined && <Button tone="primary" title={mode === "create" ? "Set up this phone" : "Open saved installation"}
      disabled={busy !== undefined} onPress={() => { void installation.start(mode, installation.bluetooth); }} />}
    {owner === undefined && mode === "create" && busy === undefined && <Link href="/setup/import" asChild><Button title="Set up using an identity file" /></Link>}
    {owner !== undefined && <>
      <Text>Saved data is open on this phone.</Text>
      <Panel><Text accessibilityRole="header">Next steps</Text>
        <Text>1. {record === undefined ? "Create or choose a messaging identity." : `Identity selected: ${record.metadata.localLabel || record.metadata.publishedName || "Unnamed identity"}.`}</Text>
        {record === undefined && <Link href="/more/identities" asChild><Button title="Choose a messaging identity" tone="primary" /></Link>}
        {record !== undefined && <>
          <Text>2. {!configured ? "Set up group messaging for this identity." : !enabled ? "Start messaging for this identity." : !running ? "Messaging is not connected yet." : "Messaging is active for this identity."}</Text>
          {(!configured || !enabled || !running) && <Link href={identityRoute} asChild><Button
            title={!configured ? "Set up messaging" : !enabled ? "Start messaging" : "Check messaging status"} tone="primary" /></Link>}
          {observed?.client === identity && observed?.failed === true && <Text>Messaging status is unavailable. Open the identity to retry.</Text>}
        </>}
        <Text>3. {online ? "Networking is running. Check your connections to reach someone." : "Networking is stopped or unavailable. Saved conversations and contacts remain on this phone."}</Text>
        <Link href="/more/interfaces" asChild><Button title="Set up connections" tone={configured && enabled && running && !online ? "primary" : "secondary"} /></Link>
        {configured && enabled && running && selected !== undefined && <>
          <Text>4. Add a contact or share your card, then create a conversation and invite them.</Text>
          <Link href={{ pathname: "/contacts", params: { owner: selected } }} asChild><Button title="Open contacts" tone="primary" /></Link>
          <Link href={{ pathname: "/conversations", params: { owner: selected } }} asChild><Button title="Open conversations" /></Link>
          {!online && <Text>Messages can be sent when networking is available again.</Text>}
        </>}
      </Panel>
      {record !== undefined && <Link href="/more/identities" asChild><Button title="Choose another identity" /></Link>}
    </>}
    <Text>Keep PRNS Chat open for reliable delivery during this alpha. Use test data; saved messages are not yet encrypted at rest.</Text>
  </Screen>;
}
