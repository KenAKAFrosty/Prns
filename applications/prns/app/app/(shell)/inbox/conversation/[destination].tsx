// route-id: inbox.conversation

import { useLocalSearchParams } from "expo-router";

import { parseDestinationHash } from "@/features/contacts/format";
import { ConversationScreen } from "@/features/inbox/inbox-screen";
import { NotFoundScreen } from "@/features/placeholder-screen";
import { type RawRouteParams, routeParamsAreValid, screenById } from "@/navigation/catalog";

export default function ConversationRoute() {
  const entry = screenById("inbox.conversation");
  const params: RawRouteParams = useLocalSearchParams();
  const destination =
    typeof params.destination === "string" ? parseDestinationHash(params.destination) : null;
  if (!routeParamsAreValid(entry, params) || destination === null) {
    return <NotFoundScreen backPath={entry.backPath} />;
  }
  return <ConversationScreen destination={destination} />;
}
