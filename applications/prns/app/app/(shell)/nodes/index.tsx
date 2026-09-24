// route-id: nodes.index

import { NodesScreen } from "@/features/nodes/nodes-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function NodesRoute() {
  return (
    <CatalogRouteGuard screenId="nodes.index">
      <NodesScreen />
    </CatalogRouteGuard>
  );
}
