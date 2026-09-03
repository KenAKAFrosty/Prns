// route-id: inbox.conversation

import { useLocalSearchParams } from "expo-router";

import { parseDestinationHash } from "@/features/contacts/format";
import { ConversationScreen } from "@/features/inbox/inbox-screen";
import { NotFoundScreen } from "@/features/placeholder-screen";

export default function ConversationRoute() {
  const params = useLocalSearchParams<{ readonly destination?: string | string[] }>();
  const destination =
    typeof params.destination === "string" ? parseDestinationHash(params.destination) : null;
  if (destination === null) {
    return <NotFoundScreen backPath="/inbox" />;
  }
  return <ConversationScreen destination={destination} />;
}
