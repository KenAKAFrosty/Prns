// route-id: installation.recovery

import { RecoveryScreen } from "@/features/onboarding/recovery-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function RecoveryRoute() {
  return (
    <CatalogRouteGuard screenId="installation.recovery">
      <RecoveryScreen />
    </CatalogRouteGuard>
  );
}
