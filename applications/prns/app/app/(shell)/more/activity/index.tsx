// route-id: activity.index

import { useLocalSearchParams } from "expo-router";

import { NetworkScreen, type NetworkTab } from "@/features/network/network-screen";
import { NotFoundScreen } from "@/features/placeholder-screen";
import { type RawRouteParams, routeParamsAreValid, screenById } from "@/navigation/catalog";

export default function ActivityRoute() {
  const entry = screenById("activity.index");
  const params: RawRouteParams = useLocalSearchParams();
  if (!routeParamsAreValid(entry, params)) return <NotFoundScreen backPath={entry.backPath} />;
  return <NetworkScreen initialTab={(params.filter ?? "connections") as NetworkTab} />;
}
