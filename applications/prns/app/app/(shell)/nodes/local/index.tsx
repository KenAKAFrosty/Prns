// route-id: nodes.local

import { LocalNodeScreen } from "@/features/nodes/nodes-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function LocalNodeRoute() {
  return (
    <CatalogRouteGuard screenId="nodes.local">
      <LocalNodeScreen />
    </CatalogRouteGuard>
  );
}
