import type { prns_groups as groups } from "@prns/app-bindings";
import { hex } from "../bytes";
import { deviceClock } from "./clock";

export type ViewedState = { version: 1; heads: Readonly<Record<string, string>> };
export const viewedDefault: ViewedState = { version: 1, heads: {} };
// Explicit saved-presentation byte budget; there is no conversation-count cap.
// Valid keys/positions and the enclosing JSON are entirely ASCII.
export const viewedByteBudget = 4 * 1024 * 1024;
function key(token: groups.GroupChangeToken) { return `${hex(token.group)}:${hex(token.incarnation)}`; }
export function decodeViewed(value: unknown): ViewedState {
  if (value === null || typeof value !== "object" || !("version" in value) || value.version !== 1 || !("heads" in value) ||
    value.heads === null || typeof value.heads !== "object" || Array.isArray(value.heads)) throw new Error("Unsupported viewed-message state; saved data preserved.");
  const heads = Object.entries(value.heads);
  if (heads.some(([key, position]) => !/^[0-9a-f]{64}:[0-9a-f]{48}$/.test(key) ||
    typeof position !== "string" || !/^(0|[1-9][0-9]{0,19})$/.test(position) || BigInt(position) > 18446744073709551615n)) throw new Error("Invalid viewed-message positions; saved data preserved.");
  const result: ViewedState = { version: 1, heads: Object.fromEntries(heads) as Record<string, string> };
  if (JSON.stringify(result).length > viewedByteBudget) throw new Error("Viewed-message positions exceed the saved byte budget; original data preserved.");
  return result;
}
export function markViewed(previous: ViewedState, heads: readonly groups.GroupChangeToken[]): ViewedState {
  const next = { ...previous.heads };
  let changed = false;
  for (const head of heads) {
    const name = key(head);
    if (next[name] === undefined || BigInt(next[name]) < head.position) { next[name] = head.position.toString(); changed = true; }
  }
  return changed ? { version: 1, heads: next } : previous;
}
export type Unread = "unread" | "viewed" | "unchecked";
/** Local viewing evidence, never a network read receipt. A bounded traversal
 * can say unknown; it must never call unread records read because paging stopped.
 * Namespace positions are comparable only within their original incarnation. */
export async function unreadStatus(client: groups.ClientLike, group: ArrayBuffer, viewed: ViewedState, signal: AbortSignal): Promise<Unread> {
  let cursor: groups.GroupRecordCursor | undefined;
  let incarnation: string | undefined;
  const clock = deviceClock();
  for (let pageNumber = 0; pageNumber < 8; pageNumber++) {
    const page = await client.messages(group, cursor, 100, clock, { signal });
    if (signal.aborted) return "unchecked";
    const current = hex(page.currentIncarnation);
    if (incarnation !== undefined && incarnation !== current) return "unchecked";
    incarnation = current;
    if (page.messages.some((message) => !message.outgoing && message.position.position > BigInt(viewed.heads[key(message.position)] ?? "0"))) return "unread";
    cursor = page.nextBefore;
    if (cursor === undefined) return "viewed";
    // Having traversed below a viewed boundary proves older rows in this one
    // namespace viewed. Skip it, but still inspect older membership namespaces.
    const head = viewed.heads[`${hex(cursor.group)}:${hex(cursor.incarnation)}`];
    if (head !== undefined && cursor.before !== undefined && cursor.before <= BigInt(head)) cursor = { ...cursor, before: 0n };
  }
  return "unchecked";
}
