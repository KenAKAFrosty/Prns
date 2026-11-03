import { useCallback, useMemo, useRef, useState } from "react";
import { ActivityIndicator, View } from "react-native";
import { prns_groups as groups, prns_groups_core as protocol } from "@prns/app-bindings";
import type { SavedContact } from "../contacts";
import { hex } from "../bytes";
import { Button, PageHeader, Panel, Text } from "../ui/primitives";
import { Disclosure } from "../ui/Disclosure";
import { InfoDisclosure } from "../ui/InfoDisclosure";
import { deviceClock } from "./clock";
import { contactName, receiptText, timestampLabel } from "./presentation";
import { useDetail } from "./useDetail";
import { usePaged } from "./usePaged";
import { Paging } from "./Paging";
import { MessageEvents } from "./MessageEvents";

const TARGET_PAGE = { maxRows: 32, maxBytes: 16n * 1024n };
function bytes(value: string, size: number): ArrayBuffer | undefined {
  if (value.length !== size * 2 || !/^[0-9a-f]+$/.test(value)) return undefined;
  return Uint8Array.from(value.match(/../g)!, (part) => Number.parseInt(part, 16)).buffer;
}
function endpointKey(endpoint: protocol.GroupEndpoint): string {
  return hex(endpoint.id) + ":" + endpoint.incarnation;
}
function Field({ label, children }: { label: string; children: string }) {
  return <View style={{ gap: 3 }}><Text style={{ fontSize: 13 }}>{label}</Text><Text selectable>{children}</Text></View>;
}

export function MessageDetails({ client, group, incarnation, messageId, contacts, active, revision, conversationTitle, localIdentity }: {
  client: groups.ClientLike; group: string; incarnation: string; messageId: string;
  contacts: readonly SavedContact[]; active: boolean; revision: number; conversationTitle?: string | undefined;
  localIdentity?: ArrayBuffer | undefined;
}) {
  const selection = group + ":" + incarnation + ":" + messageId;
  const reference = useMemo(() => {
    const groupBytes = bytes(group, 32), store = bytes(incarnation, 24), id = bytes(messageId, 32);
    return groupBytes === undefined || store === undefined || id === undefined ? undefined : { group: groupBytes, incarnation: store, id };
  }, [selection]);
  const [retry, setRetry] = useState(0);
  const read = useCallback(async (signal: AbortSignal) => {
    if (reference === undefined) return undefined;
    const clock = deviceClock();
    const [message, record] = await Promise.all([
      client.message(reference.group, reference.incarnation, reference.id, clock, { signal }),
      client.record(reference.group, reference.incarnation, reference.id, clock, { signal }),
    ]);
    if (message === undefined || record === undefined) return undefined;
    if (hex(message.id) !== messageId || hex(record.record.metadata.record) !== messageId ||
      hex(message.position.group) !== group || hex(record.position.group) !== group ||
      hex(message.position.incarnation) !== incarnation || hex(record.position.incarnation) !== incarnation ||
      hex(record.record.metadata.group) !== group || hex(message.author) !== hex(record.authorIdentity) ||
      message.outgoing !== record.outgoing || message.position.position !== record.position.position) {
      throw new Error("The saved message did not match this detail view.");
    }
    return { message, record };
  }, [client, reference, group, incarnation, messageId]);
  const detail = useDetail(client, reference === undefined ? undefined : selection, active, revision + retry, read);
  const value = detail.value;
  const originalTargets = value?.record.record.targets.map(endpointKey).join("|") ?? "";
  const targetSet = useMemo(() => new Set(originalTargets ? originalTargets.split("|") : []), [originalTargets]);
  const readTargets = useCallback(async (after: protocol.GroupEndpoint | undefined, signal: AbortSignal) => {
    if (reference === undefined) return { items: [], next: undefined };
    const page = await client.deliveryTargets(reference.group, reference.incarnation, reference.id, after, TARGET_PAGE, { signal });
    if (page === undefined) throw new Error("Recipient details are no longer available on this phone.");
    if (page.targets.some((target) => !targetSet.has(endpointKey(target.endpoint)))) {
      throw new Error("Recipient details did not match this message.");
    }
    return { items: page.targets, next: page.next };
  }, [client, reference, targetSet]);
  const targets = usePaged<groups.GroupDeliveryTarget, protocol.GroupEndpoint>({
    owner: client, scope: selection, active: active && value !== undefined, revision, read: readTargets,
  });
  const names = useRef({ client, selection, identities: new Map<string, ArrayBuffer>() });
  if (names.current.client !== client || names.current.selection !== selection) {
    names.current = { client, selection, identities: new Map() };
  }
  for (const target of targets.rows) if (target.identity !== undefined) names.current.identities.set(endpointKey(target.endpoint), target.identity);
  const deviceName = (endpoint: protocol.GroupEndpoint) => {
    const identity = names.current.identities.get(endpointKey(endpoint));
    const device = "device " + hex(endpoint.id).slice(0, 8);
    if (identity === undefined) return "Device " + hex(endpoint.id).slice(0, 8);
    const name = localIdentity !== undefined && hex(identity) === hex(localIdentity) ? "Your identity" : contactName(identity, contacts);
    return name + " · " + device;
  };
  const summary = value?.record.progress.summary;
  return <View style={{ gap: 16 }}>
    <PageHeader title="Message details" subtitle={conversationTitle} help={{ label: "About message details", content: <Text>Device confirmations mean a device saved this message. They do not mean someone read it. The sender’s timestamp comes from the sender’s clock. Current connection activity is separate from this message’s recorded activity.</Text> }} />
    {reference === undefined && <Text>Message unavailable. Open its details from a conversation.</Text>}
    {detail.error !== undefined && <View style={{ gap: 8 }}><Text accessibilityRole="alert">{detail.error}</Text>
      <Button title="Retry message details" disabled={!active} onPress={() => setRetry((old) => old + 1)} /></View>}
    {reference !== undefined && !detail.loaded && detail.error === undefined && <ActivityIndicator accessibilityLabel="Loading message details" />}
    {detail.loaded && value === undefined && detail.error === undefined && <Text>This message is no longer available on this phone. It may have been deleted.</Text>}
    {value !== undefined && summary !== undefined && <>
      <Panel>
        <Field label="Sender">{value.message.outgoing ? "You" : contactName(value.record.authorIdentity, contacts)}</Field>
        <Text selectable>{value.message.text}</Text>
        <Field label="Sender’s timestamp">{timestampLabel(value.message.sentAtMillis)}</Field>
        <Text>{receiptText({ ...value.message, receivedCount: summary.received, recipientCount: summary.originalTargets, membershipChanged: value.record.progress.membershipChanged })}</Text>
      </Panel>
      <Panel>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
          <Text accessibilityRole="header" style={{ flex: 1, fontSize: 20, fontWeight: "600" }}>Device confirmations</Text>
          <InfoDisclosure label="About device confirmations"><Text>These are the original recipient devices, with durable confirmations kept on this phone. A device can remain confirmed after leaving the conversation. Missing event history does not erase a saved confirmation.</Text></InfoDisclosure>
        </View>
        <Text>{summary.received.toString() + " of " + summary.originalTargets.toString() + " recipient devices confirmed saving this message."}</Text>
        {summary.pending > 0n && <Text>{"Waiting for confirmation: " + summary.pending}</Text>}
        {summary.removed > 0n && <Text>{"No longer in the conversation: " + summary.removed}</Text>}
        {summary.expired > 0n && <Text>{"Delivery window ended without confirmation: " + summary.expired}</Text>}
        {summary.timeUncertain > 0n && <Text>{"Delivery window cannot be checked because of the device clock: " + summary.timeUncertain}</Text>}
        {value.record.progress.membershipChanged && <Text>Membership changed before every original recipient confirmed saving this message.</Text>}
        {targets.rows.map((target) => {
          const received = target.receipt === protocol.GroupReceiptStatus.Received;
          const removed = target.removal === protocol.GroupRemovalStatus.Removed;
          const state = received ? "Save confirmed" : removed ? "No confirmation recorded"
            : summary.expired > 0n ? "Delivery window ended" : summary.timeUncertain > 0n ? "Delivery window cannot be checked" : "Awaiting confirmation";
          return <View key={endpointKey(target.endpoint)} style={{ gap: 4, paddingVertical: 8 }}>
            <Text style={{ fontWeight: "600" }}>{deviceName(target.endpoint)}</Text><Text>{state}</Text>
            {removed && <Text>No longer in the conversation</Text>}
          </View>;
        })}
        {targets.loaded && targets.rows.length === 0 && summary.originalTargets === 0n && <Text>This message has no recipient devices. Its saved copy remains on this phone.</Text>}
        <Paging label="Recipient devices" active={active} page={targets} />
      </Panel>
      <MessageEvents client={client} group={group} incarnation={incarnation} messageId={messageId}
        position={value.record.position.position} active={active} revision={revision} deviceName={deviceName} />
      <Disclosure label="Technical details">
        <Field label="Message ID">{messageId}</Field>
        <Field label="Sender identity address">{hex(value.record.authorIdentity)}</Field>
        <Field label="Sender device">{endpointKey(value.record.record.metadata.author)}</Field>
        <Field label="Conversation ID">{group}</Field>
        <Field label="Local membership reference">{incarnation}</Field>
        <Field label="Original membership">{hex(value.record.record.metadata.originalControl.era) + ":" + hex(value.record.record.metadata.originalControl.head)}</Field>
        <Field label="Local record position">{value.record.position.position.toString()}</Field>
        <Field label="Delivery deadline">{timestampLabel(value.record.record.metadata.validity.deadline * 1000n)}</Field>
        <Text>The delivery deadline does not delete your saved message history.</Text>
        {value.record.clientRequestId !== undefined && <Field label="Local send request ID">{hex(value.record.clientRequestId)}</Field>}
      </Disclosure>
    </>}
  </View>;
}
