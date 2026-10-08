// route-id: installation.onboarding

import { OnboardingScreen } from "@/features/onboarding/onboarding-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function OnboardingRoute() {
  return (
    <CatalogRouteGuard screenId="installation.onboarding">
      <OnboardingScreen step="welcome" />
    </CatalogRouteGuard>
  );
}
