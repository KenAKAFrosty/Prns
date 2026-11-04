import { Button, Text, TextInput } from "../ui/primitives";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Switch, View } from "react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { detail, Permissions } from "./presentation";

function subjectOf(snapshot: rc.RemoteControlControllerPairingSnapshot): rc.RemoteControlControllerPairingSubject | undefined {
  if (snapshot.tag === "AwaitingOffer") return new rc.RemoteControlControllerPairingSubject.AwaitingOffer({ context: snapshot.inner.context });
  if (snapshot.tag === "AwaitingApproval" || snapshot.tag === "AwaitingCompletion") {
    const { attemptId, context } = snapshot.inner.confirmation.confirmation;
    return new rc.RemoteControlControllerPairingSubject.Attempt({ attemptId, context });
  }
  if (snapshot.tag === "Persisting") return new rc.RemoteControlControllerPairingSubject.Attempt({ attemptId: snapshot.inner.attemptId, context: snapshot.inner.context });
  return undefined;
}
function sameSubject(expected: rc.RemoteControlControllerPairingSubject, actual: rc.RemoteControlControllerPairingSubject): boolean {
  return hex(expected.inner.context.endpoint.destinationHash.value) === hex(actual.inner.context.endpoint.destinationHash.value)
    && hex(expected.inner.context.linkId.value) === hex(actual.inner.context.linkId.value)
    && (expected.tag === "AwaitingOffer" || (actual.tag === "Attempt" && hex(expected.inner.attemptId.value) === hex(actual.inner.attemptId.value)));
}
function terminalKey(terminal: rc.RemoteControlControllerPairingTerminal): string {
  const subject = terminal.subject;
  return `${hex(subject.inner.context.endpoint.destinationHash.value)}:${hex(subject.inner.context.linkId.value)}:${subject.tag === "Attempt" ? hex(subject.inner.attemptId.value) : "offer"}:${terminal.outcome.tag}`;
}
function failedPairing(reason: rc.RemoteControlControllerPairingFailureReason): string {
  switch (reason) {
    case rc.RemoteControlControllerPairingFailureReason.LinkClosed: return "The connection closed before pairing was confirmed. This does not mean the node rejected you. Check the node before trying again.";
    case rc.RemoteControlControllerPairingFailureReason.RequestFailed: return "The pairing request did not complete. Check the node and your connection before trying again.";
    case rc.RemoteControlControllerPairingFailureReason.PersistenceFailed: return "Saving access could not be confirmed. The node may have granted access. Check saved nodes and the target before trying again.";
  }
}

/** Current ceremony and its terminal result come from one native capture. */
export function Pairing({ client, active, changed, onOpenPaired }: {
  client: rc.HostClientHandleLike; active: boolean; changed(): void;
  onOpenPaired?: ((target: rc.RemoteControlIdentityHash) => void) | undefined;
}) {
  const styles = useStyles();
  const scope = useMemo(() => ({ client, active }), [client, active]);
  const live = useRef(scope); live.current = scope;
  const mounted = useRef(false);
  const generation = useRef(0);
  const action = useRef(0);
  const observedSubject = useRef<rc.RemoteControlControllerPairingSubject | undefined>(undefined);
  const reconciled = useRef<string | undefined>(undefined);
  const suppressPriorTerminal = useRef(false);
  const notify = useRef(changed); notify.current = changed;
  const [statusClient, setStatusClient] = useState<rc.HostClientHandleLike>();
  const [storedStatus, setStatus] = useState<rc.RemoteControlControllerPairingStatus>();
  const [paired, setPaired] = useState<{ key: string; saved: boolean; error?: string }>();
  const [discovery, setDiscovery] = useState<rc.RemoteControlPairingDiscoverySnapshot>();
  const [selected, setSelected] = useState<rc.RemoteControlPairingAvailabilitySnapshot>();
  const [invitation, setInvitation] = useState("");
  const [discoveryError, setDiscoveryError] = useState<string>();
  const [verifiedAttempt, setVerifiedAttempt] = useState<string>();
  const submitting = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [actionError, setActionError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const status = active && statusClient === client ? storedStatus : undefined;
  const snapshot = status?.current;
  const terminal = status?.terminal;
  const current = (ticket: number, operation: number) => mounted.current && live.current === scope && active && generation.current === ticket && action.current === operation;
  useLayoutEffect(() => {
    mounted.current = true; generation.current++; action.current++;
    setStatus(undefined); setStatusClient(undefined); setPaired(undefined); setDiscovery(undefined); setSelected(undefined);
    setInvitation(""); setVerifiedAttempt(undefined); setError(undefined); setActionError(undefined); setDiscoveryError(undefined);
    observedSubject.current = undefined; reconciled.current = undefined; suppressPriorTerminal.current = false; submitting.current = false; setBusy(false);
    return () => { mounted.current = false; generation.current++; action.current++; };
  }, [client, active]);
  useEffect(() => {
    if (!active) return;
    const ticket = generation.current;
    const abort = new AbortController(); let reading = false;
    const read = async () => {
      if (reading || submitting.current) return;
      reading = true;
      const operation = action.current;
      const valid = () => !abort.signal.aborted && current(ticket, operation);
      try {
        const result = await client.remoteControlControllerPairingStatus({ signal: abort.signal });
        if (!valid()) return;
        if (result.tag === "Failed") throw result.inner.failure;
        const value = result.inner.value;
        const subject = subjectOf(value.current);
        if (subject !== undefined) { observedSubject.current = subject; suppressPriorTerminal.current = false; }
        const accepted = !suppressPriorTerminal.current && value.current.tag === "Idle" && value.terminal !== undefined
          && (observedSubject.current === undefined || sameSubject(observedSubject.current, value.terminal.subject)) ? value.terminal : undefined;
        setStatusClient(client); setStatus({ current: value.current, terminal: accepted }); setError(undefined);
        if (accepted?.outcome.tag === "Paired") {
          const key = terminalKey(accepted);
          const target = accepted.outcome.inner.target;
          try {
            const inventory = await client.remoteControlTargetInventory({ signal: abort.signal });
            if (!valid()) return;
            if (inventory.tag === "Failed") throw inventory.inner.failure;
            const saved = inventory.inner.value.targets.some((entry) => hex(entry.value) === hex(target.value));
            setPaired({ key, saved });
            if (saved && reconciled.current !== key) { reconciled.current = key; notify.current(); }
          } catch (cause) { if (valid()) setPaired({ key, saved: false, error: detail(cause) }); }
        } else setPaired(undefined);
      } catch (cause) { if (valid()) { setStatus(undefined); setPaired(undefined); setError(detail(cause)); } }
      finally {
        try {
          if (!valid()) return;
          const result = await client.remoteControlPairingDiscovery({ signal: abort.signal });
          if (valid()) {
            if (result.tag === "Failed") throw result.inner.failure;
            setDiscovery(result.inner.value); setDiscoveryError(undefined);
          }
        } catch (cause) { if (valid()) { setDiscovery(undefined); setDiscoveryError(detail(cause)); } }
        reading = false;
      }
    };
    void read(); const timer = setInterval(() => { void read(); }, 1500);
    return () => { abort.abort(); clearInterval(timer); };
  }, [active, client, revision]);
  const confirmation = snapshot?.tag === "AwaitingApproval" || snapshot?.tag === "AwaitingCompletion" ? snapshot.inner.confirmation.confirmation : undefined;
  const attempt = confirmation === undefined ? undefined : hex(confirmation.attemptId.value);
  async function mutate(operation: () => Promise<{ tag: string; inner: unknown }>, initiating = false) {
    if (!mounted.current || !active || live.current !== scope || submitting.current) return;
    const ticket = generation.current, serial = ++action.current;
    submitting.current = true; setBusy(true); setError(undefined); setActionError(undefined); setVerifiedAttempt(undefined);
    if (initiating) { suppressPriorTerminal.current = true; observedSubject.current = undefined; reconciled.current = undefined; setStatus(undefined); setPaired(undefined); }
    try {
      const result = await operation();
      if (!current(ticket, serial)) return;
      if (result.tag === "Failed") throw (result.inner as { failure: unknown }).failure;
      if (initiating) suppressPriorTerminal.current = false;
    } catch (cause) { if (current(ticket, serial)) setActionError(`Pairing action was not confirmed: ${detail(cause)}. Check the current result before trying again.`); }
    finally { if (current(ticket, serial)) { submitting.current = false; setBusy(false); setRevision((value) => value + 1); } }
  }
  async function decide(approve: boolean) {
    if (confirmation === undefined || snapshot?.tag !== "AwaitingApproval" || (approve && verifiedAttempt !== attempt)) return;
    const attemptId = confirmation.attemptId;
    await mutate(() => approve ? client.approveRemoteControlControllerPairing({ attemptId }) : client.rejectRemoteControlControllerPairing({ attemptId }));
  }
  async function cancel() {
    if (snapshot?.tag !== "AwaitingOffer" && snapshot?.tag !== "AwaitingApproval") return;
    const subject = subjectOf(snapshot)!;
    await mutate(() => client.cancelRemoteControlControllerPairing(subject));
  }
  async function initiate() {
    if (selected === undefined || discovery === undefined || snapshot?.tag !== "Idle" || !/^[0-9a-fA-F]{8}$/.test(invitation.trim()) || selected.expiresAt.value <= discovery.observedAt.value) return;
    const input = { endpoint: selected.endpoint, invitationCode: { value: Number.parseInt(invitation.trim(), 16) }, expiresAt: selected.expiresAt };
    setInvitation("");
    await mutate(() => client.initiateRemoteControlControllerPairing(input), true);
  }
  return <View style={styles.section}>
    <Text style={styles.heading}>Pair with a node</Text>
    <Text>Only approve the node and permissions you intended. Compare the complete six-digit code on both devices, including leading zeros.</Text>
    {terminal !== undefined && <View style={styles.card} accessibilityLiveRegion="polite">
      <Text style={styles.title}>Last pairing result</Text>
      <Text selectable style={styles.fingerprint}>{terminal.outcome.tag === "Paired" ? `Node: ${hex(terminal.outcome.inner.target.value)}` : `Pairing endpoint: ${hex(terminal.subject.inner.context.endpoint.destinationHash.value)}`}</Text>
      {terminal.outcome.tag === "Paired" && <>
        <Text>{paired?.key !== terminalKey(terminal) ? "Pairing reported complete. Checking saved access…" : paired.saved ? "Paired successfully. Access is saved on this phone." : paired.error !== undefined ? "Pairing reported complete, but saved access could not be checked. Check saved access before managing the node." : "Pairing reported complete, but this node is not in saved nodes. Check saved access before pairing again."}</Text>
        {paired?.key === terminalKey(terminal) && paired.saved && onOpenPaired !== undefined && <Button title="Manage paired node" tone="primary" onPress={() => {
          if (mounted.current && active && live.current === scope && terminal.outcome.tag === "Paired") onOpenPaired(terminal.outcome.inner.target);
        }} />}
      </>}
      {terminal.outcome.tag === "Rejected" && <Text>You rejected this pairing attempt. No access was saved by this attempt.</Text>}
      {terminal.outcome.tag === "Expired" && <Text>The pairing window expired. Open pairing on the node again to start a new attempt.</Text>}
      {terminal.outcome.tag === "Cancelled" && <Text>Pairing was cancelled before approval. Start again when both devices are ready.</Text>}
      {terminal.outcome.tag === "Failed" && <Text accessibilityRole="alert">{failedPairing(terminal.outcome.inner.reason)}</Text>}
    </View>}
    {(error !== undefined || discoveryError !== undefined || actionError !== undefined || terminal?.outcome.tag === "Failed" ||
      (terminal?.outcome.tag === "Paired" && paired?.saved === false)) && <Button
        title={error !== undefined || discoveryError !== undefined ? "Retry loading pairing status" : "Check pairing status"}
        onPress={() => setRevision((value) => value + 1)} disabled={busy || !active} />}
    {snapshot?.tag === rc.RemoteControlControllerPairingSnapshot_Tags.Idle && <Text>No pairing attempt is active. Choose an announced pairing endpoint or import a node-access card under Access.</Text>}
    {snapshot?.tag === rc.RemoteControlControllerPairingSnapshot_Tags.Idle && <View style={styles.card}>
      <Text style={styles.title}>Nearby pairing announcements</Text>
      <Text>Authenticated announcements identify an endpoint, not a trusted administrator. Verify the intended device and comparison code before granting access.</Text>
      {discovery === undefined ? <Text>Pairing discovery is unavailable.</Text> : <>
        <Text>{discovery.observations.length} observations · capacity {discovery.capacity} · {discovery.evicted.toString()} capacity omissions</Text>
        {discovery.observations.length === 0 && <Text>Open pairing on the target node. Announcements appear here automatically. A missing observation does not prove the node is offline.</Text>}
        {discovery.observations.map((item) => <View key={hex(item.endpoint.destinationHash.value)}>
          <Text selectable style={styles.fingerprint}>{hex(item.endpoint.destinationHash.value)}</Text>
          <Text>{item.hops.value} hops · {item.expiresAt.value > discovery.observedAt.value ? "Pairing window observed open" : "Observed window expired"}</Text>
          <Button title={`Choose pairing endpoint ${hex(item.endpoint.destinationHash.value).slice(0, 12)}`} disabled={busy || item.expiresAt.value <= discovery.observedAt.value} onPress={() => { setSelected(item); setInvitation(""); }} />
        </View>)}
      </>}
      {selected !== undefined && <>
        <Text selectable style={styles.fingerprint}>Selected endpoint: {hex(selected.endpoint.destinationHash.value)}</Text>
        <Text>Enter the target's eight hexadecimal invitation characters. The six-digit comparison code is shown in the next step.</Text>
        <TextInput accessibilityLabel="Pairing invitation code" value={invitation} onChangeText={setInvitation} autoCapitalize="none" autoCorrect={false} secureTextEntry maxLength={8} style={styles.input} editable={!busy} />
        <Button title="Begin pairing with selected endpoint" tone="primary" disabled={busy || !active || discovery === undefined || selected.expiresAt.value <= discovery.observedAt.value || !/^[0-9a-fA-F]{8}$/.test(invitation.trim())} onPress={() => { void initiate(); }} />
      </>}
      {discoveryError !== undefined && <Text accessibilityRole="alert">{discoveryError}</Text>}
    </View>}
    {snapshot?.tag === rc.RemoteControlControllerPairingSnapshot_Tags.AwaitingOffer && <View>
      <Text>Waiting for the target's offer.</Text><Text selectable style={styles.fingerprint}>Pairing endpoint: {hex(snapshot.inner.context.endpoint.destinationHash.value)}</Text>
      <Button title="Cancel pairing" disabled={busy || !active} onPress={() => { void cancel(); }} />
    </View>}
    {confirmation !== undefined && <View style={styles.card}>
      <Text style={styles.heading}>{rc.remoteControlPairingConfirmationCode(confirmation.attemptId)}</Text>
      <Text selectable style={styles.fingerprint}>Target: {hex(rc.remoteControlTargetIdentityHash(confirmation.target).value)}</Text>
      <Text selectable style={styles.fingerprint}>Controller: {hex(rc.remoteControlControllerIdentityHash(confirmation.controller).value)}</Text>
      <Text selectable style={styles.fingerprint}>Pairing endpoint: {hex(confirmation.context.endpoint.destinationHash.value)}</Text>
      <Permissions {...confirmation.permissions} />
      {snapshot?.tag === rc.RemoteControlControllerPairingSnapshot_Tags.AwaitingApproval ? <>
        <View style={styles.row}><Switch accessibilityLabel="Codes match and I approve these permissions" value={verifiedAttempt === attempt} onValueChange={(value) => setVerifiedAttempt(value ? attempt : undefined)} disabled={busy} /><Text>Codes match and I approve these exact permissions.</Text></View>
        <Button title="Approve this pairing attempt" tone="primary" disabled={busy || !active || verifiedAttempt !== attempt} onPress={() => { void decide(true); }} />
      </> : <Text>Approved locally. Waiting for the target to complete the same attempt.</Text>}
      {snapshot?.tag === "AwaitingApproval" && <>
        <Button title="Reject this pairing attempt" tone="destructive" disabled={busy || !active} onPress={() => { void decide(false); }} />
        <Button title="Cancel pairing" disabled={busy || !active} onPress={() => { void cancel(); }} />
      </>}
    </View>}
    {snapshot?.tag === rc.RemoteControlControllerPairingSnapshot_Tags.Persisting && <View>
      <Text>Saving completed access. Do not begin another pairing.</Text>
      <Text selectable>{hex(rc.remoteControlTargetIdentityHash(snapshot.inner.target).value)}</Text>
      <Permissions authority={snapshot.inner.authority} permittedRequests={snapshot.inner.permittedRequests} />
    </View>}
    {actionError !== undefined && <Text accessibilityRole="alert" style={styles.error}>{actionError}</Text>}
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
  </View>;
}
