import { useEffect, useState } from "react";
import { View } from "react-native";
import { prns_groups_core as protocol } from "@prns/app-bindings";
import type { prns_groups as groups } from "@prns/app-bindings";
import type { SavedContact } from "../contacts";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { Text } from "../ui/primitives";
import { contactName } from "./presentation";

function controlKey(value: groups.GroupConversation["control"]): string {
  return `${hex(value.era)}:${hex(value.head)}`;
}
function age(milliseconds: bigint): string {
  if (milliseconds < 1_000n) return "just now";
  if (milliseconds < 60_000n) return `${milliseconds / 1_000n}s ago`;
  if (milliseconds < 3_600_000n) return `${milliseconds / 60_000n}m ago`;
  return `${milliseconds / 3_600_000n}h ago`;
}
function phase(peer: groups.PeerSyncProgress): string {
  switch (peer.phase) {
    case protocol.GroupSyncPhase.Unknown: return "No activity observed yet";
    case protocol.GroupSyncPhase.Waiting: return peer.pending === true ? "Saved work waiting for a connection" : "Waiting for the next connection check";
    case protocol.GroupSyncPhase.Connecting: return "Connecting";
    case protocol.GroupSyncPhase.Synchronizing: return "Exchanging messages and confirmations";
    case protocol.GroupSyncPhase.RetryScheduled: return peer.retryAfterMillis === undefined ? "Waiting before another connection attempt" : `Next connection attempt in about ${(peer.retryAfterMillis + 999n) / 1_000n}s`;
    case protocol.GroupSyncPhase.StorageBlocked: return "Waiting for storage";
    case protocol.GroupSyncPhase.MembershipCatchUp: return "Catching up on group membership";
  }
}
function issue(value: protocol.GroupSyncIssue | undefined): string | undefined {
  switch (value) {
    case undefined: return undefined;
    case protocol.GroupSyncIssue.Transport: return "The last connection attempt was not confirmed.";
    case protocol.GroupSyncIssue.StorageBusy: return "Storage was busy on the last attempt.";
    case protocol.GroupSyncIssue.StorageFull: return "Storage was full on the last attempt.";
    case protocol.GroupSyncIssue.StorageUnavailable: return "Storage was unavailable on the last attempt.";
    case protocol.GroupSyncIssue.MembershipChanged: return "Group membership changed.";
    case protocol.GroupSyncIssue.TimeUncertain: return "The device clock needs attention.";
    case protocol.GroupSyncIssue.Rejected: return "The last exchange was rejected.";
    case protocol.GroupSyncIssue.UnsupportedPeer: return "The other device does not support this exchange.";
    case protocol.GroupSyncIssue.Configuration: return "Group setup needs attention.";
  }
}

type Observed = { client: groups.ClientLike; scope: string; value?: groups.GroupSyncProgress; failed?: boolean };
/** Disposable activity only; authenticated message receipts remain in History. */
export function DeliveryDetails({ client, conversation, contacts, active, heading = true }: {
  client: groups.ClientLike; conversation: groups.GroupConversation;
  contacts: readonly SavedContact[]; active: boolean; heading?: boolean;
}) {
  const styles = useStyles();
  const control = controlKey(conversation.control);
  const group = hex(conversation.group);
  const scope = `${group}:${control}`;
  const [observed, setObserved] = useState<Observed>();
  useEffect(() => {
    if (!active) return;
    // Capture the value, not the decoder's replaceable buffer identity.
    const selected = conversation.group.slice(0);
    const read = () => {
      try { setObserved({ client, scope, value: client.synchronizationProgress(selected) }); }
      catch { setObserved({ client, scope, failed: true }); }
    };
    read();
    // Runner activity is disposable and need not commit a database revision.
    // Poll only the visible diagnostics; never clear known rows between reads.
    const timer = setInterval(read, 2_000);
    return () => clearInterval(timer);
  }, [client, group, control, active]);
  if (!active) return null;
  const current = observed?.client === client && observed.scope === scope ? observed : undefined;
  const value = current?.value;
  const matches = value?.control !== undefined && controlKey(value.control) === control;
  return <View style={styles.section}>
    {heading && <Text accessibilityRole="header" style={styles.title}>Connection activity</Text>}
    <Text>Recent connection activity for this conversation, not a history of any one message. Open a message to see which devices confirmed saving it. Neither confirms someone read it.</Text>
    {current?.failed ? <Text>Connection activity is unavailable. Checking again automatically.</Text>
      : value === undefined ? <Text>Checking connection activity…</Text>
      : !value.running ? <Text>No live connection activity is available. Networking or messaging may be paused.</Text>
      : !matches ? <Text>Waiting for activity from this group’s current membership.</Text>
      : <>
        {!value.observed && <Text>Only some device activity is available. Missing observations do not mean delivery failed.</Text>}
        {value.peers.length === 0 && <Text>No device activity has been observed yet.</Text>}
        {value.peers.map((peer) => <View key={`${hex(peer.endpoint.id)}:${peer.endpoint.incarnation}`} style={styles.card}>
          <Text>{contactName(peer.identity, contacts)} · {hex(peer.identity).slice(0, 8)}</Text>
          <Text>{phase(peer)}</Text>
          {issue(peer.lastIssue) !== undefined && <Text>{issue(peer.lastIssue)}</Text>}
          <Text>Last connection update {age(peer.updatedAgoMillis)}</Text>
        </View>)}
      </>}
  </View>;
}
