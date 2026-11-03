import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ActivityIndicator, View } from "react-native";
import { prns_groups_service as service } from "@prns/app-bindings";
import type { prns_groups as groups, prns_groups_core as protocol } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useStyles } from "../ui";
import { InfoDisclosure } from "../ui/InfoDisclosure";
import { Button, Text } from "../ui/primitives";

const PAGE_SIZE = 50;
const PAGES_PER_LOAD = 4;
const MAX_POSITION = (1n << 64n) - 1n;
type Event = { cursor: bigint; kind: service.GroupChangeKind };
type Snapshot = { rows: Event[]; cursor: bigint; head: bigint; loaded: boolean; pruned: boolean; busy: boolean; error?: string | undefined };
export type MessageEventsProps = {
  client: groups.ClientLike; group: string; incarnation: string; messageId: string; position: bigint;
  active: boolean; revision: number; deviceName?: (endpoint: protocol.GroupEndpoint) => string;
};

function bytes(value: string, length: number): ArrayBuffer {
  if (!new RegExp(`^[0-9a-f]{${length * 2}}$`, "i").test(value)) throw new Error("Invalid activity selection.");
  return Uint8Array.from(value.match(/../g)!, (part) => Number.parseInt(part, 16)).buffer;
}
function validPosition(value: bigint): boolean {
  return typeof value === "bigint" && value >= 0n && value <= MAX_POSITION;
}
function validatePage(page: groups.GroupRecordChanges, group: string, incarnation: string, cursor: bigint, head: bigint): void {
  const scoped = (token: groups.GroupChangeToken) => hex(token.group) === group && hex(token.incarnation) === incarnation && validPosition(token.position);
  if (!scoped(page.next) || !scoped(page.head) || page.changes.length > PAGE_SIZE || page.head.position < head
    || page.next.position < cursor || page.next.position > page.head.position
    || (page.next.position === cursor && page.head.position > cursor)) throw new Error("Invalid activity page.");
  let previous = cursor;
  for (const change of page.changes) {
    if (!validPosition(change.cursor) || change.cursor <= previous || change.cursor > page.next.position) throw new Error("Invalid activity order.");
    if ((change.kind.tag === service.GroupChangeKind_Tags.Receipt || change.kind.tag === service.GroupChangeKind_Tags.ReceiptQueued)
      && hex(change.kind.inner[0].group) !== group) throw new Error("Invalid receipt scope.");
    previous = change.cursor;
  }
}
function matches(kind: service.GroupChangeKind, messageId: string): boolean {
  switch (kind.tag) {
    case service.GroupChangeKind_Tags.Published:
    case service.GroupChangeKind_Tags.Received: return hex(kind.inner[0]) === messageId;
    case service.GroupChangeKind_Tags.Receipt:
    case service.GroupChangeKind_Tags.ReceiptQueued: return hex(kind.inner[0].record) === messageId;
    // Rejections identify an envelope, not a message. Never infer a match.
    case service.GroupChangeKind_Tags.Rejected: return false;
  }
}
function label(kind: service.GroupChangeKind, deviceName: MessageEventsProps["deviceName"]): string {
  switch (kind.tag) {
    case service.GroupChangeKind_Tags.Published: return "Saved on this phone";
    case service.GroupChangeKind_Tags.Received: return "Received and saved on this phone";
    case service.GroupChangeKind_Tags.ReceiptQueued: return "Receipt queued for sender";
    case service.GroupChangeKind_Tags.Receipt: {
      const endpoint = kind.inner[0].target;
      return `Receipt confirmed from ${deviceName?.(endpoint) || `device ${hex(endpoint.id).slice(0, 8)}`}`;
    }
    case service.GroupChangeKind_Tags.Rejected: return "";
  }
}

/** A bounded view of retained local events, never the authority for message status. */
export function MessageEvents({ client, group, incarnation, messageId, position, active, revision, deviceName }: MessageEventsProps) {
  const styles = useStyles();
  const groupKey = group.toLowerCase(), incarnationKey = incarnation.toLowerCase(), messageKey = messageId.toLowerCase();
  const cache = useMemo(() => {
    const cursor = position > 0n ? position - 1n : 0n;
    return { view: { rows: [], cursor, head: cursor, loaded: false, pruned: false, busy: false } as Snapshot,
      request: undefined as AbortController | undefined, mounted: false };
  }, [client, groupKey, incarnationKey, messageKey, position]);
  const current = useRef({ cache, active });
  current.current = { cache, active };
  const [rendered, setRendered] = useState({ cache, view: cache.view });
  const load = useCallback(async () => {
    if (!cache.mounted || !current.current.active || current.current.cache !== cache || cache.request !== undefined) return;
    const request = new AbortController();
    cache.request = request;
    const live = () => cache.mounted && current.current.cache === cache && current.current.active && cache.request === request && !request.signal.aborted;
    const publish = (view: Snapshot) => { cache.view = view; setRendered({ cache, view }); };
    publish({ ...cache.view, busy: true, error: undefined });
    let next = cache.view;
    try {
      const selectedGroup = bytes(groupKey, 32), selectedIncarnation = bytes(incarnationKey, 24);
      bytes(messageKey, 32);
      if (!validPosition(position)) throw new Error("Invalid activity selection.");
      for (let count = 0; count < PAGES_PER_LOAD; count += 1) {
        const page = await client.changes(selectedGroup, { group: selectedGroup, incarnation: selectedIncarnation, position: next.cursor }, PAGE_SIZE, { signal: request.signal });
        if (!live()) return;
        validatePage(page, groupKey, incarnationKey, next.cursor, next.head);
        next = { ...next, rows: [...next.rows, ...page.changes.filter((change) => matches(change.kind, messageKey))],
          cursor: page.next.position, head: page.head.position, loaded: true, pruned: next.pruned || page.historyPruned };
        if (next.cursor === next.head) break;
      }
      if (live()) publish({ ...next, busy: false });
    } catch {
      if (live()) publish({ ...next, busy: false, error: "Recorded activity could not be read. Message and receipt details are still shown separately." });
    } finally {
      if (cache.request === request) cache.request = undefined;
    }
  }, [cache, client, groupKey, incarnationKey, messageKey, position]);
  useEffect(() => {
    cache.mounted = true;
    if (active) void load();
    return () => {
      cache.mounted = false;
      cache.request?.abort();
      cache.request = undefined;
    };
  }, [cache, active, revision, load]);
  if (!active) return null;
  const value = rendered.cache === cache ? rendered.view : cache.view;
  return <View style={styles.section}>
    <View style={styles.row}>
      <Text accessibilityRole="header" style={styles.title}>Recorded activity</Text>
      <InfoDisclosure label="About recorded activity"><Text>These retained events are shown in this phone’s local order. They have no recorded event times and may not form a complete history. Message and receipt details above are the current facts; this list does not establish delivery status or whether someone read a message.</Text></InfoDisclosure>
    </View>
    {value.pruned && <Text>Some earlier activity is no longer retained. This list may be incomplete.</Text>}
    {!value.loaded && value.busy && <ActivityIndicator accessibilityLabel="Loading recorded activity" />}
    {value.rows.map((event) => <Text key={event.cursor.toString()}>{label(event.kind, deviceName)}</Text>)}
    {value.loaded && value.rows.length === 0 && <Text>No retained events for this message have been found.</Text>}
    {value.error !== undefined && <>
      <Text accessibilityRole="alert">{value.error}</Text>
      <Button title="Retry recorded activity" disabled={value.busy} onPress={() => { void load(); }} />
    </>}
    {value.cursor < value.head && <Button title="Load more events" disabled={value.busy} onPress={() => { void load(); }} />}
  </View>;
}
