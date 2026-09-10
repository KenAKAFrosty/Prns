// route-id: inbox.compose

import { useLocalSearchParams } from "expo-router";

import { ComposeScreen } from "@/features/inbox/inbox-screen";
import { NotFoundScreen } from "@/features/placeholder-screen";
import { type RawRouteParams, routeParamsAreValid, screenById } from "@/navigation/catalog";

export default function ComposeRoute() {
  const entry = screenById("inbox.compose");
  const params: RawRouteParams = useLocalSearchParams();
  if (!routeParamsAreValid(entry, params)) {
    return <NotFoundScreen backPath={entry.backPath} />;
  }
  const destination = typeof params.destination === "string" ? params.destination : null;
  return <ComposeScreen initialDestination={destination} />;
}
