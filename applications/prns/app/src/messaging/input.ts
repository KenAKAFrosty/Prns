import AsyncStorage from "@react-native-async-storage/async-storage";
import { presentationEpoch, requirePresentationEpoch, serializePresentation } from "../runtime/presentation-storage";
import { prns_groups as groups, prns_groups_core as protocolValues } from "@prns/app-bindings";
import type { prns_groups_core as protocol, prns_identities_core as core } from "@prns/app-bindings";
import { identityKey } from "../identity-reference";
import { hex } from "../bytes";

/** One immutable form awaiting native acknowledgement, never an action/result journal. */
export type SavedInput = Readonly<{ version: 2; identity: string; id: string; intent: string }>;
const converter = groups.default.converters.FfiConverterTypeGroupIntent;
// Keep the storage location stable so an earlier encoded input is rejected by
// its envelope version, rather than accidentally treated as an absent request.
const prefix = "prns.app.v1.group-input.";
const unreadable = () => new Error("The saved group action cannot be read. It has been kept unchanged; do not repeat it with a new request.");

function serialized<T>(identity: string, run: () => Promise<T>, ticket = presentationEpoch()): Promise<T> {
  return serializePresentation(prefix + identity, run, ticket);
}
function bytes(value: string): ArrayBuffer {
  if (!/^(?:[0-9a-f]{2})+$/.test(value)) throw unreadable();
  return Uint8Array.from(value.match(/../g)!, (part) => Number.parseInt(part, 16)).buffer;
}
export function encodeIntent(intent: groups.GroupIntent): string {
  // The generated public converter is the only model codec. This app envelope
  // is versioned with its binding schema; upgrades must not silently reinterpret
  // a pending input or replace its ID. It is not a protocol/storage wire format.
  const encoded = converter.lower(intent, (size) => new Uint8Array(size));
  return hex(encoded.slice().buffer as ArrayBuffer);
}
export function inputIntent(input: SavedInput): groups.GroupIntent {
  return converter.lift(new Uint8Array(bytes(input.intent)));
}
export function inputId(input: SavedInput): ArrayBuffer { return bytes(input.id); }
export function captureInput(identity: core.IdentityReference, intent: groups.GroupIntent, id = groups.createRequestId()): SavedInput {
  if (id.byteLength !== 32) throw unreadable();
  const input = Object.freeze({ version: 2 as const, identity: identityKey(identity), id: hex(id), intent: encodeIntent(intent) });
  if (JSON.stringify(input).length > 262144) throw new Error("This form is too large to save safely.");
  return input;
}
export function sameInput(left: SavedInput, right: SavedInput): boolean {
  return left.version === right.version && left.identity === right.identity && left.id === right.id && left.intent === right.intent;
}
async function read(identity: string): Promise<SavedInput | undefined> {
  const value = await AsyncStorage.getItem(prefix + identity);
  if (value === null) return undefined;
  try {
    // Bound parsing of damaged local input before the generated decoder runs.
    if (value.length > 262144) throw unreadable();
    const data: unknown = JSON.parse(value);
    if (typeof data !== "object" || data === null || !("version" in data) || data.version !== 2 ||
        !("identity" in data) || data.identity !== identity || !("id" in data) || typeof data.id !== "string" ||
        !/^[0-9a-f]{64}$/.test(data.id) || !("intent" in data) || typeof data.intent !== "string") throw unreadable();
    const result: SavedInput = Object.freeze({ version: 2, identity, id: data.id, intent: data.intent });
    // Reject trailing bytes and noncanonical/malformed encodings; preserve raw storage.
    if (encodeIntent(inputIntent(result)) !== result.intent) throw unreadable();
    return result;
  } catch { throw unreadable(); }
}
export class PendingInputConflict extends Error {
  constructor(readonly pending: SavedInput) { super("Finish the saved group action before starting another action."); }
}
export function loadInput(identity: core.IdentityReference): Promise<SavedInput | undefined> {
  const key = identityKey(identity);
  return serialized(key, () => read(key));
}
async function save(input: SavedInput, ticket: number): Promise<void> {
  await serialized(input.identity, async () => {
    const current = await read(input.identity);
    if (current !== undefined && !sameInput(current, input)) throw new PendingInputConflict(current);
    await AsyncStorage.setItem(prefix + input.identity, JSON.stringify(input));
  }, ticket);
}
function removeMatching(input: SavedInput, ticket = presentationEpoch()): Promise<void> {
  return serialized(input.identity, async () => {
    const current = await read(input.identity);
    if (current !== undefined && sameInput(current, input)) await AsyncStorage.removeItem(prefix + input.identity);
  }, ticket);
}
export class InputRejected extends Error {
  constructor(readonly input: SavedInput, cause: unknown) { super("Update this action before trying again. Its input was rejected before it could be applied.", { cause }); }
}
export class InputExpired extends Error {
  constructor(readonly input: SavedInput, cause: unknown) {
    super("This saved action has expired. Its earlier result may be unknown.", { cause });
  }
}
export async function releaseExpiredInput(client: groups.ClientLike, identity: core.IdentityReference, expired: InputExpired): Promise<void> {
  const ticket = presentationEpoch();
  requirePresentationEpoch(ticket);
  const input = expired.input;
  if (input.identity !== identityKey(identity) || hex(await client.identity()) !== hex(identity.publicHash)) {
    throw new Error("This action belongs to a different identity. The saved action has been kept.");
  }
  if (await client.operation(inputId(input)) !== undefined) {
    throw new Error("A saved result is available. Check this action before continuing.");
  }
  // Absence does not establish non-execution. This is only the user's explicit
  // release of a matching local form, never cancellation or native retirement.
  await serialized(input.identity, async () => {
    const current = await read(input.identity);
    if (current === undefined) return;
    if (!sameInput(current, input)) throw new PendingInputConflict(current);
    await AsyncStorage.removeItem(prefix + input.identity);
  }, ticket);
}
export function acknowledgeInput(input: SavedInput, operation: groups.GroupOperation, ticket = presentationEpoch()): Promise<void> {
  if (hex(operation.request.id) !== input.id || encodeIntent(operation.request.intent) !== input.intent) {
    return Promise.reject(new Error("Native action evidence does not match the saved input. The input has been kept."));
  }
  return removeMatching(input, ticket);
}
export async function submitInput(client: groups.ClientLike, identity: core.IdentityReference, input: SavedInput,
  context: groups.GroupExecutionContext): Promise<groups.GroupOperation> {
  const ticket = presentationEpoch();
  requirePresentationEpoch(ticket);
  if (input.identity !== identityKey(identity) || hex(await client.identity()) !== hex(identity.publicHash)) {
    throw new Error("This group client belongs to a different identity. No action was submitted.");
  }
  await save(input, ticket);
  requirePresentationEpoch(ticket); // Never hold the storage section across a native wait.
  let result: groups.GroupOperation;
  try { result = await client.submit(inputId(input), inputIntent(input), context); }
  catch (cause) {
    if (cause !== null && typeof cause === "object" && protocolValues.GroupApiError.Retry.instanceOf(cause) &&
        cause.inner.reason === protocolValues.GroupRetryError.Expired) {
      throw new InputExpired(input, cause);
    }
    if (cause !== null && typeof cause === "object" && protocolValues.GroupApiError.InvalidInput.instanceOf(cause)) {
      await removeMatching(input, ticket); // If cleanup is uncertain, keep the form locked.
      throw new InputRejected(input, cause);
    }
    throw cause;
  }
  await acknowledgeInput(input, result, ticket);
  return result;
}
export { deviceClock } from "./clock";
