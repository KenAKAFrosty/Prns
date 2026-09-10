// route-id: more.index

import { MoreScreen } from "@/features/more/more-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function MoreRoute() {
  return (
    <CatalogRouteGuard screenId="more.index">
      <MoreScreen />
    </CatalogRouteGuard>
  );
}
