// route-id: explore.index

import { ExploreScreen } from "@/features/explore/explore-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function ExploreRoute() {
  return (
    <CatalogRouteGuard screenId="explore.index">
      <ExploreScreen />
    </CatalogRouteGuard>
  );
}
