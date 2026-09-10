// route-id: nodes.pair

import { useLocalSearchParams } from "expo-router";

import { PairNodeScreen } from "@/features/nodes/pair-node-screen";
import { NotFoundScreen } from "@/features/placeholder-screen";
import { type RawRouteParams, routeParamsAreValid, screenById } from "@/navigation/catalog";

export default function PairNodeRoute() {
  const entry = screenById("nodes.pair");
  const params: RawRouteParams = useLocalSearchParams();
  if (!routeParamsAreValid(entry, params)) {
    return <NotFoundScreen backPath="/nodes" />;
  }
  return (
    <PairNodeScreen
      selectedCandidateId={typeof params.candidateId === "string" ? params.candidateId : undefined}
    />
  );
}
