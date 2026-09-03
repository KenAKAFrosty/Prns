// route-id: contacts.entry

import { useLocalSearchParams } from "expo-router";

import { ContactDetailScreen } from "@/features/contacts/contacts-screen";
import { parseDestinationHash } from "@/features/contacts/format";
import { NotFoundScreen } from "@/features/placeholder-screen";

export default function ContactRoute() {
  const params = useLocalSearchParams<{ readonly destination?: string | string[] }>();
  const destination =
    typeof params.destination === "string" ? parseDestinationHash(params.destination) : null;
  if (destination === null) {
    return <NotFoundScreen backPath="/contacts" />;
  }
  return <ContactDetailScreen destination={destination} />;
}
