// route-id: contacts.entry

import { useLocalSearchParams } from "expo-router";

import { ContactDetailScreen } from "@/features/contacts/contacts-screen";
import { parseDestinationHash } from "@/features/contacts/format";
import { NotFoundScreen } from "@/features/placeholder-screen";
import { type RawRouteParams, routeParamsAreValid, screenById } from "@/navigation/catalog";

export default function ContactRoute() {
  const entry = screenById("contacts.entry");
  const params: RawRouteParams = useLocalSearchParams();
  const destination =
    typeof params.destination === "string" ? parseDestinationHash(params.destination) : null;
  if (!routeParamsAreValid(entry, params) || destination === null) {
    return <NotFoundScreen backPath={entry.backPath} />;
  }
  return <ContactDetailScreen destination={destination} />;
}
