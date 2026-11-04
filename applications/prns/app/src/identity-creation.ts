import AsyncStorage from "@react-native-async-storage/async-storage";
import { presentationEpoch, requirePresentationEpoch, serializePresentation } from "./runtime/presentation-storage.ts";
import { getRandomBytesAsync } from "expo-crypto";
import { prns_identities_core as core } from "@prns/app-bindings";
import type { prns_identities as identities } from "@prns/app-bindings";
import { hex } from "./bytes.ts";
export { hex } from "./bytes.ts";

// Keep the recovery slot key: encountering the earlier nonempty-service input
// must fail closed, not silently reuse its nonce with the new creation intent.
const key = "prns.app.v1.identity-creation";

/** Product form input and its caller-owned recovery ID, never native progress.
 * Version 2 creates keys/identity only. Messaging schema selection is a separate
 * explicit retained AddService operation, not implicit initialization on Enable.
 */
export type CreationDraft = Readonly<{
  version: 2;
  incarnation: string;
  epoch: string;
  nonce: string;
  localLabel: string;
  publishedName: string;
}>;

/** A different exact request already owns the saved recovery slot. */
export class CreationDraftConflict extends Error {
  readonly pending: CreationDraft;
  constructor(pending: CreationDraft) {
    super("Another saved identity request needs to finish first.");
    this.name = "CreationDraftConflict";
    this.pending = pending;
  }
}

/** Only emitted after definite native metadata rejection and safe draft cleanup. */
export class CreationMetadataRejected extends Error {
  readonly draft: CreationDraft;
  constructor(draft: CreationDraft, cause: unknown) {
    super("Update the identity names before trying again.", { cause });
    this.name = "CreationMetadataRejected";
    this.draft = draft;
  }
}

// React instances can overlap during close/reopen. Serialize only this key's
// read/check/write sections; never hold the section while awaiting native work.
function withSavedDraft<T>(operation: () => Promise<T>, ticket = presentationEpoch()): Promise<T> {
  return serializePresentation(key, operation, ticket);
}

function bytes(hexValue: string): ArrayBuffer {
  return Uint8Array.from(hexValue.match(/../g) ?? [], (byte) => Number.parseInt(byte, 16)).buffer;
}

export function decodeCreationDraft(encoded: string): CreationDraft {
  const value: unknown = JSON.parse(encoded);
  if (typeof value !== "object" || value === null ||
      !("version" in value) || value.version !== 2 ||
      !("incarnation" in value) || typeof value.incarnation !== "string" || !/^[0-9a-f]{32}$/.test(value.incarnation) ||
      !("epoch" in value) || typeof value.epoch !== "string" || !/^(0|[1-9][0-9]*)$/.test(value.epoch) || BigInt(value.epoch) > 0xffffffffffffffffn ||
      !("nonce" in value) || typeof value.nonce !== "string" || !/^[0-9a-f]{64}$/.test(value.nonce) ||
      !("localLabel" in value) || typeof value.localLabel !== "string" ||
      !("publishedName" in value) || typeof value.publishedName !== "string") {
    throw new Error("The saved identity request cannot be read. It has been kept for recovery.");
  }
  return Object.freeze({ version: 2, incarnation: value.incarnation, epoch: value.epoch, nonce: value.nonce,
    localLabel: value.localLabel, publishedName: value.publishedName });
}

async function readSavedDraft(): Promise<CreationDraft | undefined> {
  const encoded = await AsyncStorage.getItem(key);
  return encoded === null ? undefined : decodeCreationDraft(encoded);
}

export function loadCreationDraft(): Promise<CreationDraft | undefined> {
  return withSavedDraft(readSavedDraft);
}

function matches(left: CreationDraft, right: CreationDraft): boolean {
  return left.version === right.version && left.incarnation === right.incarnation &&
    left.epoch === right.epoch && left.nonce === right.nonce &&
    left.localLabel === right.localLabel && left.publishedName === right.publishedName;
}

function saveDraft(draft: CreationDraft, ticket: number): Promise<void> {
  return withSavedDraft(async () => {
    const pending = await readSavedDraft();
    if (pending !== undefined) {
      if (!matches(pending, draft)) throw new CreationDraftConflict(pending);
      return;
    }
    await AsyncStorage.setItem(key, JSON.stringify(draft));
  }, ticket);
}

function clearDraft(draft: CreationDraft, ticket: number): Promise<void> {
  return withSavedDraft(async () => {
    const pending = await readSavedDraft();
    if (pending === undefined) return;
    if (!matches(pending, draft)) throw new CreationDraftConflict(pending);
    await AsyncStorage.removeItem(key);
  }, ticket);
}

export async function prepareCreationDraft(
  catalog: identities.IdentityCatalogClientLike,
  metadata: core.IdentityMetadata,
): Promise<CreationDraft> {
  const { localLabel, publishedName } = metadata;
  const scope = await catalog.creationScope();
  const nonce = await getRandomBytesAsync(32);
  return Object.freeze({ version: 2, incarnation: hex(scope.incarnation), epoch: scope.epoch.toString(),
    nonce: hex(Uint8Array.from(nonce).buffer), localLabel, publishedName });
}

export async function submitCreationDraft(
  catalog: identities.IdentityCatalogClientLike,
  draft: CreationDraft,
): Promise<identities.IdentityCreateOutcome> {
  // Copy before the first await: callers cannot mutate the input used for
  // either native execution or exact cleanup while storage admission is pending.
  const ticket = presentationEpoch();
  requirePresentationEpoch(ticket);
  const exact = decodeCreationDraft(JSON.stringify(draft));
  // Save BEFORE polling native work. Failed or uncertain storage never submits,
  // and a matching earlier write may already have succeeded despite reply loss.
  await saveDraft(exact, ticket);
  requirePresentationEpoch(ticket);
  let outcome: identities.IdentityCreateOutcome;
  try {
    outcome = await catalog.createOrRecover(
      { scope: { incarnation: bytes(exact.incarnation), epoch: BigInt(exact.epoch) }, nonce: bytes(exact.nonce) },
      { source: new core.IdentityKeySource.Generate(), metadata: { localLabel: exact.localLabel,
        publishedName: exact.publishedName }, services: [] },
      undefined,
    );
  } catch (cause) {
    // The owning checked converter rejects metadata before native admission.
    // No UI copy of its byte limits or Unicode policy is maintained here.
    if (typeof cause === "object" && cause !== null &&
        core.IdentityApiError.Authority.instanceOf(cause) &&
        cause.inner.reason === core.IdentityFault.InvalidMetadata) {
      // A read/remove failure is still uncertain storage: do not unlock the
      // form or replace the ID until matching cleanup has actually succeeded.
      await clearDraft(exact, ticket);
      throw new CreationMetadataRejected(exact, cause);
    }
    throw cause;
  }
  await clearDraft(exact, ticket);
  return outcome;
}
