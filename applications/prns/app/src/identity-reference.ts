import type { prns_identities_core as core } from "@prns/app-bindings";
import { hex } from "./bytes";

export function identityKey(reference: core.IdentityReference): string {
  return `${hex(reference.id.incarnation)}:${reference.id.sequence}:${hex(reference.publicHash)}`;
}
