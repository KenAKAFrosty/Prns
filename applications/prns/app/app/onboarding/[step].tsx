// route-id: installation.onboarding

import { useLocalSearchParams } from "expo-router";

import { isOnboardingStep, OnboardingScreen } from "@/features/onboarding/onboarding-screen";
import { NotFoundScreen } from "@/features/placeholder-screen";
import { type RawRouteParams, routeParamsAreValid, screenById } from "@/navigation/catalog";

export default function OnboardingStepRoute() {
  const entry = screenById("installation.onboarding");
  const params: RawRouteParams = useLocalSearchParams();
  const step = params.step;

  if (!routeParamsAreValid(entry, params) || typeof step !== "string" || !isOnboardingStep(step)) {
    return <NotFoundScreen backPath={entry.backPath} />;
  }

  return <OnboardingScreen step={step} />;
}
