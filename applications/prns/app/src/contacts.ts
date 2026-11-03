import AsyncStorage from "@react-native-async-storage/async-storage";
import { serializePresentation as serialized } from "./runtime/presentation-storage.ts";
import { prns_contact_card as cards } from "@prns/app-bindings";
import type { prns_identities_core as core } from "@prns/app-bindings";
import { hex } from "./bytes";
import { identityKey } from "./identity-reference";
export { identityKey } from "./identity-reference";

/** Product address-book choices. Claims are always re-derived by the core verifier. */
export type SavedContact = Readonly<{ uri: string; alias: string; pinned?: boolean; claims: cards.ContactCardClaims }>;
const capacity = 256;

export function contactKey(contact: SavedContact): string {
  return `${hex(contact.claims.identityHash)}:${hex(contact.claims.serviceNameHash)}`;
}
function storageKey(reference: core.IdentityReference): string {
  return `prns.app.v1.contacts.${identityKey(reference)}`;
}
export function verifyMessagingContact(uri: string, alias = ""): SavedContact {
  const wire = cards.decodeContactCardUri(uri.trim());
  const claims = cards.verifyContactCard(wire);
  if (hex(claims.serviceNameHash) !== hex(cards.serviceNameHash("prns.groups.enrollment"))) {
    throw new Error("This contact card does not offer group messaging.");
  }
  const label = alias.trim();
  if (Array.from(label).length > 128 || /[\u0000-\u001f\u007f]/u.test(label)) {
    throw new Error("Use a contact label of up to 128 characters without control characters.");
  }
  return Object.freeze({ uri: cards.encodeContactCardUri(wire), alias: label, claims });
}

async function read(key: string): Promise<readonly SavedContact[]> {
  const encoded = await AsyncStorage.getItem(key);
  if (encoded === null) return [];
  const data: unknown = JSON.parse(encoded);
  if (typeof data !== "object" || data === null || !("version" in data) || data.version !== 1 ||
      !("contacts" in data) || !Array.isArray(data.contacts) || data.contacts.length > capacity) {
    throw new Error("The saved address book cannot be read. It has been kept unchanged.");
  }
  const contacts = data.contacts.map((entry: unknown) => {
    if (typeof entry !== "object" || entry === null || !("uri" in entry) || typeof entry.uri !== "string" ||
        !("alias" in entry) || typeof entry.alias !== "string") {
      throw new Error("A saved contact cannot be read. The address book has been kept unchanged.");
    }
    if ("pinned" in entry && typeof entry.pinned !== "boolean") {
      throw new Error("A saved contact preference cannot be read. The address book has been kept unchanged.");
    }
    return Object.freeze({ ...verifyMessagingContact(entry.uri, entry.alias), pinned: "pinned" in entry ? entry.pinned as boolean : false });
  });
  if (new Set(contacts.map(contactKey)).size !== contacts.length) {
    throw new Error("The saved address book has duplicate contacts. It has been kept unchanged.");
  }
  return pinnedFirst(contacts);
}

/** Stable within each section; pinning never changes the signed card. */
export function pinnedFirst(contacts: readonly SavedContact[]): readonly SavedContact[] {
  return [...contacts.filter((contact) => contact.pinned === true), ...contacts.filter((contact) => contact.pinned !== true)];
}
async function write(key: string, contacts: readonly SavedContact[]): Promise<readonly SavedContact[]> {
  const ordered = pinnedFirst(contacts);
  await AsyncStorage.setItem(key, JSON.stringify({ version: 1,
    contacts: ordered.map(({ uri, alias, pinned }) => ({ uri, alias, pinned: pinned === true })) }));
  return ordered;
}

export function loadContacts(reference: core.IdentityReference): Promise<readonly SavedContact[]> {
  const key = storageKey(reference);
  return serialized(key, () => read(key));
}
// A retry replaces metadata for the same verified identity/service, including any
// intervening alias/card edit. Re-saving the desired metadata resolves a conflict.
export function saveContact(reference: core.IdentityReference, uri: string, alias: string): Promise<readonly SavedContact[]> {
  const key = storageKey(reference);
  const contact = verifyMessagingContact(uri, alias);
  return serialized(key, async () => {
    const current = await read(key);
    const index = current.findIndex((entry) => contactKey(entry) === contactKey(contact));
    if (index < 0 && current.length >= capacity) throw new Error("This address book is full (256 contacts). Remove a contact before adding another.");
    const next = [...current];
    if (index < 0) next.push(contact); else next[index] = Object.freeze({ ...contact, pinned: current[index]!.pinned === true });
    return write(key, next);
  });
}
export function removeContact(reference: core.IdentityReference, expected: SavedContact): Promise<readonly SavedContact[]> {
  const key = storageKey(reference);
  const id = contactKey(expected);
  const { uri, alias } = expected;
  return serialized(key, async () => {
    const current = await read(key);
    const existing = current.find((entry) => contactKey(entry) === id);
    if (existing !== undefined && (existing.uri !== uri || existing.alias !== alias)) {
      throw new Error("This contact changed. Refresh and review it before removing it.");
    }
    const next = current.filter((entry) => contactKey(entry) !== id);
    return write(key, next);
  });
}

/** Exact desired preference makes an explicit retry safe after a lost write reply. */
export function setContactPinned(reference: core.IdentityReference, expected: SavedContact, pinned: boolean): Promise<readonly SavedContact[]> {
  const key = storageKey(reference);
  return serialized(key, async () => {
    const current = await read(key);
    const index = current.findIndex((contact) => contactKey(contact) === contactKey(expected));
    const existing = current[index];
    if (existing === undefined || existing.uri !== expected.uri || existing.alias !== expected.alias) {
      throw new Error("This contact changed. Open it again before changing its pin.");
    }
    if ((existing.pinned === true) === pinned) return current;
    const next = [...current];
    next[index] = Object.freeze({ ...existing, pinned });
    return write(key, next);
  });
}
