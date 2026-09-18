// route-id: settings.about

import { CatalogRouteGuard } from "@/features/placeholder-screen";
import { AboutScreen } from "@/features/settings/about-screen";

export default function AboutRoute() {
  return (
    <CatalogRouteGuard screenId="settings.about">
      <AboutScreen />
    </CatalogRouteGuard>
  );
}
