// route-id: help.index

import { CatalogRouteGuard } from "@/features/placeholder-screen";
import { HelpScreen } from "@/features/settings/help-screen";

export default function HelpRoute() {
  return (
    <CatalogRouteGuard screenId="help.index">
      <HelpScreen />
    </CatalogRouteGuard>
  );
}
