import type { prns_host_uniffi as host } from "@prns/app-bindings";
import { hex } from "../bytes";

export function sameGuard(left: host.ConnectionGuard, right: host.ConnectionGuard): boolean {
  return left.revision === right.revision && hex(left.incarnation) === hex(right.incarnation);
}

/** Compare canonical data, including enum payloads, byte IDs and bigint revisions.
 * This is only evidence for a guarded UI save; it never retries a native write.
 */
export function sameValue(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  const leftBytes = left instanceof ArrayBuffer || left instanceof Uint8Array;
  const rightBytes = right instanceof ArrayBuffer || right instanceof Uint8Array;
  if (leftBytes || rightBytes) return leftBytes && rightBytes && hex(left) === hex(right);
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right) && left.length === right.length && left.every((value, index) => sameValue(value, right[index]));
  }
  if (left === null || right === null || typeof left !== "object" || typeof right !== "object") return false;
  const a = left as Record<string, unknown>;
  const b = right as Record<string, unknown>;
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  return Array.from(keys).every((key) => sameValue(a[key], b[key]));
}

export function sameProfileId(left: host.ConnectionProfile["id"], right: host.ConnectionProfile["id"]): boolean { return hex(left) === hex(right); }
export function errorText(cause: unknown): string { return cause instanceof Error ? cause.message : String(cause); }
