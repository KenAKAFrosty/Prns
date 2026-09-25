// route-id: interfaces.index

import { ConnectionsScreen } from "@/features/connections/connections-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function InterfacesRoute() {
  return (
    <CatalogRouteGuard screenId="interfaces.index">
      <ConnectionsScreen />
    </CatalogRouteGuard>
  );
}
