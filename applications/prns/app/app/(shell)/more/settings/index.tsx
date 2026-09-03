// route-id: settings.index

import { CatalogRouteGuard } from "@/features/placeholder-screen";
import { SettingsScreen } from "@/features/settings/settings-screen";

export default function SettingsRoute() {
  return (
    <CatalogRouteGuard screenId="settings.index">
      <SettingsScreen />
    </CatalogRouteGuard>
  );
}
