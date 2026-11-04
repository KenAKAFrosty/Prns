import { prns_contact_card as cards } from "@prns/app-bindings";
import { hex } from "../bytes";
import type { HeardAnnouncement } from "./network-activity";
import type { SavedContact } from "../contacts";

export type DiscoveredContact = Readonly<{ contact: SavedContact; observation: HeardAnnouncement }>;
export const discoveryLifetimeMillis = 24 * 60 * 60 * 1000;
export const discoveryCapacity = 200;

/** A raw diagnostic is only a candidate. The signed claim must also match the
 * exact identity accepted by the native host for this service destination. */
export async function discoverContact(observation: HeardAnnouncement,
  acceptedKey: (destination: Uint8Array) => Promise<Uint8Array | undefined>): Promise<DiscoveredContact | undefined> {
  const { appData, destination } = observation.event.inner;
  if (appData.length < 148 || appData.length > 276) return undefined;
  let contact: SavedContact;
  try {
    const wire = new Uint8Array(appData).buffer;
    const claims = cards.verifyContactCard(wire);
    if (hex(claims.serviceNameHash) !== hex(cards.serviceNameHash("prns.groups.enrollment")) ||
        hex(claims.destinationHash) !== hex(destination)) return undefined;
    contact = Object.freeze({ uri: cards.encodeContactCardUri(wire), alias: "", claims });
  } catch { return undefined; } // Unrelated or malformed advertisements are ordinary network traffic.
  const key = await acceptedKey(destination);
  if (key === undefined || hex(key) !== hex(contact.claims.publicIdentity)) return undefined;
  return { contact, observation };
}

export function retainDiscoveries(previous: readonly DiscoveredContact[], incoming: readonly DiscoveredContact[], now: number): readonly DiscoveredContact[] {
  const seen = new Set<string>();
  return [...incoming, ...previous].filter((item) => {
    const key = `${hex(item.contact.claims.identityHash)}:${hex(item.contact.claims.serviceNameHash)}`;
    if (now - item.observation.observedAt >= discoveryLifetimeMillis || seen.has(key)) return false;
    seen.add(key); return true;
  }).slice(0, discoveryCapacity);
}
